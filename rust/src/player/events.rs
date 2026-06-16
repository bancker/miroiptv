use super::mpv_sys as sys;
use super::Event;
use std::ffi::CStr;

/// Map a raw mpv event to our higher-level Event enum.
/// Returns None when the event has no UI-relevant translation.
///
/// # Safety
/// `evt` must be a valid pointer returned by `mpv_wait_event` whose backing
/// memory is still owned by the mpv handle (i.e. not yet invalidated by a
/// subsequent `mpv_wait_event` call).
pub unsafe fn from_mpv(evt: *mut sys::mpv_event) -> Option<Event> {
    let id = (*evt).event_id;
    match id {
        sys::MPV_EVENT_FILE_LOADED => Some(Event::FileLoaded),
        sys::MPV_EVENT_PLAYBACK_RESTART => Some(Event::PlaybackStarted),
        sys::MPV_EVENT_END_FILE => Some(Event::EndOfFile {
            reason: end_file_reason(evt),
        }),
        _ => None,
    }
}

/// # Safety
/// Same constraints as [`from_mpv`]. Callers should only pass events whose
/// `event_id == MPV_EVENT_END_FILE`.
pub unsafe fn end_file_reason(evt: *mut sys::mpv_event) -> String {
    let data = (*evt).data;
    if data.is_null() {
        return "unknown".into();
    }
    let ef = data as *const sys::mpv_event_end_file;
    reason_label((*ef).reason)
}

/// Label for an `mpv_end_file_reason` enum value.
///
/// The values are NOT contiguous - mpv's public enum skips 1:
/// `EOF=0, STOP=2, QUIT=3, ERROR=4, REDIRECT=5` (mpv/client.h:1465).
/// The previous mapping assumed `0..=4` were contiguous, so STOP(2) -
/// the reason mpv reports for the previous stream on EVERY channel
/// switch (loadfile stops it) - was mislabelled "quit". That is the
/// "ended: quit" the user kept seeing.
fn reason_label(code: std::os::raw::c_int) -> String {
    match code {
        0 => "eof".into(),
        2 => "stop".into(),
        3 => "quit".into(),
        4 => "error".into(),
        5 => "redirect".into(),
        n => format!("reason={}", n),
    }
}

/// Parse an `MPV_EVENT_LOG_MESSAGE` payload into (prefix, level, text).
///
/// # Safety
/// Same constraints as [`from_mpv`]. Callers should only pass events whose
/// `event_id == MPV_EVENT_LOG_MESSAGE`.
pub unsafe fn log_message(evt: *mut sys::mpv_event) -> Option<(String, String, String)> {
    let data = (*evt).data as *const sys::mpv_event_log_message;
    if data.is_null() {
        return None;
    }
    let cstr_or = |p: *const std::os::raw::c_char| -> String {
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    let prefix = cstr_or((*data).prefix);
    let level = cstr_or((*data).level);
    let mut text = cstr_or((*data).text);
    // mpv log lines include trailing newline; strip for tidier formatting.
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    Some((prefix, level, text))
}

/// # Safety
/// Same constraints as [`from_mpv`].
pub unsafe fn property_change(evt: *mut sys::mpv_event) -> Option<(String, String)> {
    let data = (*evt).data as *mut sys::mpv_event_property;
    if data.is_null() {
        return None;
    }
    let name = CStr::from_ptr((*data).name).to_string_lossy().into_owned();
    let val = if (*data).format == sys::MPV_FORMAT_STRING && !(*data).data.is_null() {
        let p = *((*data).data as *const *const std::os::raw::c_char);
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    } else {
        String::new()
    };
    Some((name, val))
}

#[cfg(test)]
mod tests {
    use super::reason_label;

    // Values mirror mpv/client.h's mpv_end_file_reason enum, which is NOT
    // contiguous (1 is unused): EOF=0, STOP=2, QUIT=3, ERROR=4, REDIRECT=5.
    #[test]
    fn maps_each_mpv_reason_to_its_label() {
        assert_eq!(reason_label(0), "eof");
        assert_eq!(reason_label(2), "stop");
        assert_eq!(reason_label(3), "quit");
        assert_eq!(reason_label(4), "error");
        assert_eq!(reason_label(5), "redirect");
    }

    #[test]
    fn stop_is_not_mislabelled_quit() {
        // Regression guard for the off-by-one that made every channel switch
        // (mpv reports STOP=2 for the previous stream) surface as "ended: quit".
        assert_eq!(reason_label(2), "stop");
        assert_ne!(reason_label(2), "quit");
    }

    #[test]
    fn unknown_code_is_explicit_not_silently_dropped() {
        assert_eq!(reason_label(1), "reason=1"); // unused slot in mpv's enum
        assert_eq!(reason_label(7), "reason=7");
    }
}
