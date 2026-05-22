#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::os::raw::{c_char, c_int, c_void};

pub type mpv_handle = c_void;
pub type mpv_render_context = c_void;

pub const MPV_FORMAT_NONE: c_int = 0;
pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_OSD_STRING: c_int = 2;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;
pub const MPV_FORMAT_NODE: c_int = 6;

pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_LOG_MESSAGE: c_int = 2;
pub const MPV_EVENT_GET_PROPERTY_REPLY: c_int = 3;
pub const MPV_EVENT_SET_PROPERTY_REPLY: c_int = 4;
pub const MPV_EVENT_COMMAND_REPLY: c_int = 5;
pub const MPV_EVENT_START_FILE: c_int = 6;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_IDLE: c_int = 11;
pub const MPV_EVENT_TICK: c_int = 14;
pub const MPV_EVENT_CLIENT_MESSAGE: c_int = 16;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: c_int = 18;
pub const MPV_EVENT_SEEK: c_int = 20;
pub const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

pub const MPV_RENDER_API_TYPE_SW: &str = "sw";
pub const MPV_RENDER_PARAM_INVALID: c_int = 0;
pub const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const MPV_RENDER_PARAM_SW_SIZE: c_int = 17;
pub const MPV_RENDER_PARAM_SW_FORMAT: c_int = 18;
pub const MPV_RENDER_PARAM_SW_STRIDE: c_int = 19;
pub const MPV_RENDER_PARAM_SW_POINTER: c_int = 20;

#[repr(C)]
pub struct mpv_render_param {
    pub type_: c_int,
    pub data: *mut c_void,
}

pub const MPV_RENDER_UPDATE_FRAME: u64 = 1;

#[link(name = "mpv", kind = "dylib")]
extern "C" {
    pub fn mpv_client_api_version() -> std::os::raw::c_ulong;
    pub fn mpv_create() -> *mut mpv_handle;
    pub fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    pub fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    pub fn mpv_set_option_string(
        ctx: *mut mpv_handle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    pub fn mpv_set_property_string(
        ctx: *mut mpv_handle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    pub fn mpv_get_property_string(ctx: *mut mpv_handle, name: *const c_char) -> *mut c_char;
    pub fn mpv_get_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_observe_property(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
    pub fn mpv_command(ctx: *mut mpv_handle, args: *mut *const c_char) -> c_int;
    pub fn mpv_command_async(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        args: *mut *const c_char,
    ) -> c_int;
    pub fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
    pub fn mpv_wakeup(ctx: *mut mpv_handle);
    pub fn mpv_set_wakeup_callback(
        ctx: *mut mpv_handle,
        cb: extern "C" fn(*mut c_void),
        d: *mut c_void,
    );
    pub fn mpv_free(data: *mut c_void);
    pub fn mpv_error_string(error: c_int) -> *const c_char;
    pub fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;

    pub fn mpv_render_context_create(
        out: *mut *mut mpv_render_context,
        mpv: *mut mpv_handle,
        params: *mut mpv_render_param,
    ) -> c_int;
    pub fn mpv_render_context_render(
        ctx: *mut mpv_render_context,
        params: *mut mpv_render_param,
    ) -> c_int;
    pub fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;
    pub fn mpv_render_context_set_update_callback(
        ctx: *mut mpv_render_context,
        cb: extern "C" fn(*mut c_void),
        d: *mut c_void,
    );
    pub fn mpv_render_context_free(ctx: *mut mpv_render_context);
}
