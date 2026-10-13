use super::mpv::{check, Mpv, MpvError};
use super::mpv_sys as sys;
use parking_lot::Mutex;
use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct RgbaFrame {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>, // length = w*h*4, BGRA0 from mpv
    pub version: u64,
}

pub struct FrameBus {
    inner: Mutex<RgbaFrame>,
    on_new_frame: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Default for FrameBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBus {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RgbaFrame::default()),
            on_new_frame: Mutex::new(None),
        }
    }

    pub fn set_new_frame_callback(&self, cb: impl Fn() + Send + Sync + 'static) {
        *self.on_new_frame.lock() = Some(Box::new(cb));
    }

    pub fn read(&self) -> RgbaFrame {
        self.inner.lock().clone()
    }

    /// Cheap read of just the frame version counter, without cloning the
    /// ~3.6 MB pixel buffer. The stall watchdog polls this every frame, so
    /// it must not pay the full `read()` clone.
    pub fn version(&self) -> u64 {
        self.inner.lock().version
    }

    /// Publishes a frame. The render thread does; `pub(crate)` so that the app's
    /// tests can put a picture on the bus without mpv.
    pub(crate) fn write(&self, w: u32, h: u32, data: Vec<u8>) {
        {
            let mut g = self.inner.lock();
            g.w = w;
            g.h = h;
            g.data = data;
            g.version = g.version.wrapping_add(1);
        }
        if let Some(cb) = self.on_new_frame.lock().as_ref() {
            cb();
        }
    }
}

/// Which frames on the bus may show. A stopped stream's frames must not come
/// back, and the newest of them can still be on their way when the stop is
/// ordered: mpv renders one more while the command travels to its thread. So
/// from the stop on nothing shows, until the next load narrows the fence to
/// what is on the bus by then (the stopped stream's last frames); the first
/// newer frame lifts it.
#[derive(Debug, Default)]
pub struct FrameFence(Option<u64>);

impl FrameFence {
    pub fn stop(&mut self) {
        self.0 = Some(u64::MAX);
    }

    /// A stream loads; `latest` is the version of the newest frame on the bus.
    /// Without a stop there is no fence to narrow.
    pub fn load(&mut self, latest: u64) {
        if self.0.is_some() {
            self.0 = Some(latest);
        }
    }

    /// Whether the frame with this version may show.
    pub fn admit(&mut self, version: u64) -> bool {
        if self.0.is_some_and(|fence| version <= fence) {
            return false;
        }
        self.0 = None;
        true
    }
}

pub struct RenderCtx {
    raw: *mut sys::mpv_render_context,
    frames: Arc<FrameBus>,
    width: u32,
    height: u32,
    buf: Vec<u8>,
    // Keep CStrings alive for parameter strings passed to mpv.
    _api_type: CString,
}

unsafe impl Send for RenderCtx {}

impl RenderCtx {
    pub fn new(
        mpv: &Mpv,
        frames: Arc<FrameBus>,
        init_w: u32,
        init_h: u32,
    ) -> Result<Self, MpvError> {
        let api_type = CString::new(sys::MPV_RENDER_API_TYPE_SW).unwrap();

        let mut params: [sys::mpv_render_param; 2] = [
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut ctx: *mut sys::mpv_render_context = ptr::null_mut();
        let rc = unsafe {
            sys::mpv_render_context_create(&mut ctx as *mut _, mpv.raw(), params.as_mut_ptr())
        };
        if rc < 0 || ctx.is_null() {
            return Err(MpvError::Code(
                rc,
                "mpv_render_context_create failed".into(),
            ));
        }

        let buf = vec![0u8; (init_w as usize).max(1) * (init_h as usize).max(1) * 4];
        Ok(Self {
            raw: ctx,
            frames,
            width: init_w.max(1),
            height: init_h.max(1),
            buf,
            _api_type: api_type,
        })
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        let w = w.max(1);
        let h = h.max(1);
        if w == self.width && h == self.height {
            return;
        }
        self.width = w;
        self.height = h;
        self.buf = vec![0u8; (w as usize) * (h as usize) * 4];
    }

    /// Render the current frame into our buffer and publish to the FrameBus.
    /// Safe to call only when has_new_frame() returns true (otherwise mpv will return error).
    pub fn render_into_frames(&mut self) -> Result<(), MpvError> {
        let format = CString::new("rgb0").unwrap();
        let mut size_arr: [i32; 2] = [self.width as i32, self.height as i32];
        let mut stride: usize = (self.width as usize) * 4;

        let mut params: [sys::mpv_render_param; 5] = [
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_SW_SIZE,
                data: size_arr.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_SW_FORMAT,
                data: format.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_SW_STRIDE,
                data: &mut stride as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_SW_POINTER,
                data: self.buf.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        check(unsafe { sys::mpv_render_context_render(self.raw, params.as_mut_ptr()) })?;
        self.frames.write(self.width, self.height, self.buf.clone());
        Ok(())
    }

    pub fn has_new_frame(&self) -> bool {
        let flags = unsafe { sys::mpv_render_context_update(self.raw) };
        flags & sys::MPV_RENDER_UPDATE_FRAME != 0
    }
}

impl Drop for RenderCtx {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                sys::mpv_render_context_free(self.raw);
            }
            self.raw = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_streams_frames_never_come_back() {
        let mut fence = FrameFence::default();
        assert!(fence.admit(5), "no stop: every frame shows");
        fence.stop();
        // A frame mpv rendered while the stop was on its way is as stale as the
        // older ones, though it is newer than the bus was when the stop was ordered.
        assert!(!fence.admit(5));
        assert!(!fence.admit(6));
        // The next stream loads; what is on the bus then is the stopped one's.
        fence.load(6);
        assert!(!fence.admit(6));
        // Its first frame lifts the fence for good.
        assert!(fence.admit(7));
        assert!(fence.admit(7));
        assert!(fence.admit(8));
    }

    #[test]
    fn a_load_without_a_stop_leaves_the_frames_alone() {
        let mut fence = FrameFence::default();
        fence.load(10);
        assert!(fence.admit(10));
        assert!(fence.admit(11));
    }
}
