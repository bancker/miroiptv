use super::mpv_sys as sys;
use super::Event;
use std::ffi::CStr;

/// Map a raw mpv event to our higher-level Event enum.
/// Returns None when the event has no UI-relevant translation.
pub unsafe fn from_mpv(evt: *mut sys::mpv_event) -> Option<Event> {
    let id = (*evt).event_id;
    match id {
        sys::MPV_EVENT_FILE_LOADED       => Some(Event::FileLoaded),
        sys::MPV_EVENT_PLAYBACK_RESTART  => Some(Event::PlaybackStarted),
        sys::MPV_EVENT_END_FILE          => Some(Event::EndOfFile { reason: end_file_reason(evt) }),
        _ => None,
    }
}

unsafe fn end_file_reason(evt: *mut sys::mpv_event) -> String {
    let data = (*evt).data;
    if data.is_null() { return "unknown".into(); }
    let ef = data as *const sys::mpv_event_end_file;
    match (*ef).reason {
        0 => "eof".into(),
        1 => "stop".into(),
        2 => "quit".into(),
        3 => "error".into(),
        4 => "redirect".into(),
        n => format!("reason={}", n),
    }
}

pub unsafe fn property_change(evt: *mut sys::mpv_event) -> Option<(String, String)> {
    let data = (*evt).data as *mut sys::mpv_event_property;
    if data.is_null() { return None; }
    let name = CStr::from_ptr((*data).name).to_string_lossy().into_owned();
    let val = if (*data).format == sys::MPV_FORMAT_STRING && !(*data).data.is_null() {
        let p = *((*data).data as *const *const std::os::raw::c_char);
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    } else { String::new() };
    Some((name, val))
}
