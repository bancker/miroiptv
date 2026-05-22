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
        if h.is_null() { return Err(MpvError::NullHandle); }

        let me = Self { handle: h };

        me.set_option("config", "no")?;
        me.set_option("audio-display", "no")?;
        me.set_option("cache-secs", "10")?;
        me.set_option("hwdec", "auto-safe")?;
        me.set_option("vo", "libmpv")?;
        me.set_option("input-default-bindings", "no")?;
        me.set_option("input-vo-keyboard", "no")?;
        me.set_option("osc", "no")?;
        me.set_option("sid", "auto")?;
        me.set_option("sub-auto", "fuzzy")?;
        // Reduce stdout chatter; we route via mpv's log msg event.
        me.set_option("terminal", "no")?;

        check(unsafe { sys::mpv_initialize(h) })?;

        let lvl = CString::new("info").unwrap();
        unsafe { sys::mpv_request_log_messages(h, lvl.as_ptr()); }

        Ok(me)
    }

    pub fn raw(&self) -> *mut sys::mpv_handle { self.handle }

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
            if p.is_null() { return None; }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            sys::mpv_free(p as *mut c_void);
            Some(s)
        }
    }

    pub fn get_property_f64(&self, name: &str) -> Option<f64> {
        let n = CString::new(name).unwrap();
        let mut out: f64 = 0.0;
        let rc = unsafe {
            sys::mpv_get_property(self.handle, n.as_ptr(), sys::MPV_FORMAT_DOUBLE, &mut out as *mut _ as *mut c_void)
        };
        if rc < 0 { None } else { Some(out) }
    }

    pub fn command(&self, args: &[&str]) -> Result<(), MpvError> {
        let cstrs: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
        let mut ptrs: Vec<*const std::os::raw::c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        check(unsafe { sys::mpv_command(self.handle, ptrs.as_mut_ptr()) })
    }

    pub fn observe_property(&self, name: &str, format: i32, userdata: u64) -> Result<(), MpvError> {
        let n = CString::new(name).unwrap();
        check(unsafe { sys::mpv_observe_property(self.handle, userdata, n.as_ptr(), format) })
    }

    pub fn wait_event(&self, timeout_s: f64) -> *mut sys::mpv_event {
        unsafe { sys::mpv_wait_event(self.handle, timeout_s) }
    }

    pub fn set_wakeup_callback(&self, cb: extern "C" fn(*mut c_void), ud: *mut c_void) {
        unsafe { sys::mpv_set_wakeup_callback(self.handle, cb, ud); }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { sys::mpv_terminate_destroy(self.handle); }
            self.handle = ptr::null_mut();
        }
    }
}

pub fn check(rc: i32) -> Result<(), MpvError> {
    if rc >= 0 { return Ok(()); }
    let msg = unsafe {
        let p = sys::mpv_error_string(rc);
        if p.is_null() { "unknown".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    Err(MpvError::Code(rc, msg))
}
