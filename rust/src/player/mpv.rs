use super::mpv_sys as sys;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::ptr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MpvError {
    #[error("mpv error {0}: {1}")]
    Code(i32, String),
    #[error("mpv handle is null (create failed)")]
    NullHandle,
}

pub struct Mpv {
    handle: *mut sys::mpv_handle,
}

unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    pub fn new() -> Result<Self, MpvError> {
        let h = unsafe { sys::mpv_create() };
        if h.is_null() {
            return Err(MpvError::NullHandle);
        }

        let me = Self { handle: h };

        me.set_option("config", "no")?;
        me.set_option("audio-display", "no")?;
        me.set_option("cache-secs", "10")?;
        me.set_option("hwdec", "auto-safe")?;
        // Don't decode a frame ahead or wait for the first frame to be fully
        // rendered before starting: zap -> picture went 1.10 -> 0.82 s on
        // average. Frame dropping then trusts the stream's fps, which live TV
        // reports correctly.
        me.set_option("video-latency-hacks", "yes")?;
        me.set_option("vo", "libmpv")?;
        me.set_option("input-default-bindings", "no")?;
        me.set_option("input-vo-keyboard", "no")?;
        me.set_option("osc", "no")?;
        // mpv's other built-in Lua scripts: each a VM on its own thread
        // (~2.5 MB together) the app never uses, and ytdl_hook would launch
        // yt-dlp on a portal URL that failed to open - one more portal session,
        // seconds later. Best effort: an mpv without one of them is fine too.
        for script in [
            "ytdl",
            "load-stats-overlay",
            "load-console",
            "load-auto-profiles",
            "load-select",
            "load-positioning",
            "load-commands",
        ] {
            let _ = me.set_option(script, "no");
        }
        me.set_option("sid", "auto")?;
        me.set_option("sub-auto", "fuzzy")?;
        // Reduce stdout chatter; we route via mpv's log msg event.
        me.set_option("terminal", "no")?;

        check(unsafe { sys::mpv_initialize(h) })?;

        // Default to "info" so demux/HW-decode/HLS errors land in our log
        // file without spamming per-packet trace under normal playback.
        // Diagnose deeper issues with: set MPV_LOG_LEVEL=v (verbose) or
        // =debug before launching.
        let lvl_str = std::env::var("MPV_LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
        let lvl = CString::new(lvl_str).unwrap();
        unsafe {
            sys::mpv_request_log_messages(h, lvl.as_ptr());
        }

        Ok(me)
    }

    pub fn raw(&self) -> *mut sys::mpv_handle {
        self.handle
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<(), MpvError> {
        let n = CString::new(name).unwrap();
        let v = CString::new(value).unwrap();
        check(unsafe { sys::mpv_set_option_string(self.handle, n.as_ptr(), v.as_ptr()) })
    }

    pub fn set_property(&self, name: &str, value: &str) -> Result<(), MpvError> {
        let n = CString::new(name).unwrap();
        let v = CString::new(value).unwrap();
        check(unsafe { sys::mpv_set_property_string(self.handle, n.as_ptr(), v.as_ptr()) })
    }

    pub fn get_property_string(&self, name: &str) -> Option<String> {
        let n = CString::new(name).unwrap();
        unsafe {
            let p = sys::mpv_get_property_string(self.handle, n.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            sys::mpv_free(p as *mut c_void);
            Some(s)
        }
    }

    pub fn get_property_f64(&self, name: &str) -> Option<f64> {
        let n = CString::new(name).unwrap();
        let mut out: f64 = 0.0;
        let rc = unsafe {
            sys::mpv_get_property(
                self.handle,
                n.as_ptr(),
                sys::MPV_FORMAT_DOUBLE,
                &mut out as *mut _ as *mut c_void,
            )
        };
        if rc < 0 {
            None
        } else {
            Some(out)
        }
    }

    pub fn command(&self, args: &[&str]) -> Result<(), MpvError> {
        let cstrs = c_strings(args)?;
        let mut ptrs: Vec<*const std::os::raw::c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        check(unsafe { sys::mpv_command(self.handle, ptrs.as_mut_ptr()) })
    }

    pub fn observe_property(&self, name: &str, format: i32, userdata: u64) -> Result<(), MpvError> {
        let n = CString::new(name).unwrap();
        check(unsafe { sys::mpv_observe_property(self.handle, userdata, n.as_ptr(), format) })
    }

    /// Stops observing every property observed with `userdata`.
    pub fn unobserve_property(&self, userdata: u64) -> Result<(), MpvError> {
        check(unsafe { sys::mpv_unobserve_property(self.handle, userdata) })
    }

    pub fn wait_event(&self, timeout_s: f64) -> *mut sys::mpv_event {
        unsafe { sys::mpv_wait_event(self.handle, timeout_s) }
    }

    /// # Safety
    /// The caller must ensure `ud` outlives the mpv handle, and is safe to
    /// dereference from any thread (the callback fires from mpv's worker
    /// threads).
    pub unsafe fn set_wakeup_callback(&self, cb: extern "C" fn(*mut c_void), ud: *mut c_void) {
        sys::mpv_set_wakeup_callback(self.handle, cb, ud);
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                sys::mpv_terminate_destroy(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}

pub fn check(rc: i32) -> Result<(), MpvError> {
    if rc >= 0 {
        return Ok(());
    }
    let msg = unsafe {
        let p = sys::mpv_error_string(rc);
        if p.is_null() {
            "unknown".into()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    Err(MpvError::Code(rc, msg))
}

/// mpv's `MPV_ERROR_INVALID_PARAMETER`: its catch-all for a parameter that is
/// invalid (client.h: "used if there is no better error code").
const MPV_ERROR_INVALID_PARAMETER: i32 = -4;

/// The arguments of a command as C strings. An argument with a NUL byte in it
/// can be none, and that is an error, not a panic: a list's URL can hold a NUL
/// (a raw one, or `%00` in a header), and the release build aborts on a panic.
/// The error does not repeat the argument: it may hold credentials.
fn c_strings(args: &[&str]) -> Result<Vec<CString>, MpvError> {
    args.iter()
        .map(|arg| {
            CString::new(*arg).map_err(|_| {
                MpvError::Code(
                    MPV_ERROR_INVALID_PARAMETER,
                    "an argument holds a NUL byte".into(),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mpv's `MPV_ERROR_INVALID_PARAMETER`.
    const INVALID_PARAMETER: i32 = -4;

    /// An `Mpv` with nothing behind it. `command` has to refuse an argument
    /// with a NUL in it before it calls libmpv, so these tests never reach it.
    fn handleless() -> Mpv {
        Mpv {
            handle: ptr::null_mut(),
        }
    }

    #[test]
    fn a_nul_in_an_argument_is_an_error_not_a_panic() {
        // A raw NUL byte in a list's URL: from_utf8_lossy keeps it.
        let err = handleless()
            .command(&["loadfile", "http://user:secret@a.example/\0x"])
            .unwrap_err();
        assert!(
            matches!(err, MpvError::Code(INVALID_PARAMETER, _)),
            "{err:?}"
        );
        // The argument may hold credentials: the error does not repeat it.
        assert!(!err.to_string().contains("secret"), "{err}");
    }

    #[test]
    fn a_percent_00_in_a_lists_header_value_is_refused_cleanly() {
        // `%00` in a Kodi-style header decodes to a NUL byte in the option.
        for url in [
            "http://a.example/1|User-Agent=a%00b",
            "http://a.example/1|Referer=a%00b",
        ] {
            let args = crate::player::loadfile_args(url);
            assert!(args.iter().any(|a| a.contains('\0')), "{args:?}");
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let err = handleless().command(&args).unwrap_err();
            assert!(
                matches!(err, MpvError::Code(INVALID_PARAMETER, _)),
                "{err:?}"
            );
        }
    }

    #[test]
    fn arguments_without_a_nul_become_the_same_c_strings() {
        let c = c_strings(&["loadfile", "http://a.example/1", "user-agent=%2%é"]).unwrap();
        let bytes: Vec<&[u8]> = c.iter().map(|s| s.to_bytes()).collect();
        let expected: [&[u8]; 3] = [
            b"loadfile",
            b"http://a.example/1",
            "user-agent=%2%é".as_bytes(),
        ];
        assert_eq!(bytes, expected);
    }
}
