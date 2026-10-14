pub mod events;
pub mod mpv;
pub mod mpv_sys;
pub mod render;

pub use mpv::{Mpv, MpvError};
pub use render::{FrameBus, FrameFence, RenderCtx, RgbaFrame};

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub enum Cmd {
    LoadUrl(String),
    Pause(bool),
    /// Stop playing; nothing is loaded afterwards.
    Stop,
    SeekRelative(f64),
    CycleAudio,
    CycleSubtitle,
    SetWindowSize(u32, u32),
    Quit,
}

#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    PlaybackStarted,
    EndOfFile { reason: String },
    Error { msg: String },
    PropertyChanged { name: String, value: String },
}

pub struct PlayerHandle {
    pub cmd_tx: UnboundedSender<Cmd>,
    pub evt_rx: parking_lot::Mutex<UnboundedReceiver<Event>>,
    pub frames: Arc<FrameBus>,
}

pub fn spawn(initial_w: u32, initial_h: u32) -> anyhow::Result<PlayerHandle> {
    let (cmd_tx, mut cmd_rx) = unbounded_channel::<Cmd>();
    let (evt_tx, evt_rx) = unbounded_channel::<Event>();
    let frames = Arc::new(FrameBus::new());
    let frames_for_thread = frames.clone();

    thread::Builder::new()
        .name("mpv-player".into())
        .spawn(move || {
            let mpv = match Mpv::new() {
                Ok(m) => m,
                Err(e) => {
                    let _ = evt_tx.send(Event::Error {
                        msg: format!("mpv init: {}", e),
                    });
                    return;
                }
            };
            let mut rctx =
                match RenderCtx::new(&mpv, frames_for_thread, initial_w.max(1), initial_h.max(1)) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = evt_tx.send(Event::Error {
                            msg: format!("render ctx: {}", e),
                        });
                        return;
                    }
                };

            // Observe paused-for-cache as a STRING so the existing
            // property_change parser yields "yes"/"no" (it only handles
            // MPV_FORMAT_STRING). This is the freeze signal: mpv flips it to
            // "yes" when the picture stalls on a cache underrun and back to
            // "no" when playback resumes.
            if let Err(e) = mpv.observe_property("paused-for-cache", mpv_sys::MPV_FORMAT_STRING, 1) {
                warn!("observe paused-for-cache failed: {}", e);
            }

            info!("mpv player thread started");

            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::LoadUrl(url)) => {
                            let args = loadfile_args(&url);
                            let (_, headers) = crate::m3u::split_headers(&url);
                            let sent: Vec<&str> = [
                                ("user-agent", headers.user_agent.is_some()),
                                ("referrer", headers.referrer.is_some()),
                            ]
                            .into_iter()
                            .filter_map(|(name, on)| on.then_some(name))
                            .collect();
                            let address = crate::redact::redact(&args[1]);
                            if sent.is_empty() {
                                info!("loadfile {}", address);
                            } else {
                                info!("loadfile {} ({})", address, sent.join(", "));
                            }
                            let args: Vec<&str> = args.iter().map(String::as_str).collect();
                            if let Err(e) = mpv.command(&args) {
                                warn!("loadfile failed: {}", e);
                                let _ = evt_tx.send(Event::Error { msg: e.to_string() });
                            }
                        }
                        Ok(Cmd::Pause(p)) => {
                            let _ = mpv.set_property("pause", if p { "yes" } else { "no" });
                        }
                        Ok(Cmd::Stop) => {
                            info!("stop");
                            let _ = mpv.command(&["stop"]);
                        }
                        Ok(Cmd::SeekRelative(s)) => {
                            let _ = mpv.command(&["seek", &s.to_string(), "relative"]);
                        }
                        Ok(Cmd::CycleAudio) => {
                            let _ = mpv.command(&["cycle", "audio"]);
                        }
                        Ok(Cmd::CycleSubtitle) => {
                            let _ = mpv.command(&["cycle", "sub"]);
                        }
                        Ok(Cmd::SetWindowSize(w, h)) => {
                            rctx.resize(w, h);
                        }
                        Ok(Cmd::Quit) => {
                            info!("player thread quit");
                            return;
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                    }
                }

                let evt = mpv.wait_event(0.005);
                let id = unsafe { (*evt).event_id };
                match id {
                    mpv_sys::MPV_EVENT_NONE => {}
                    mpv_sys::MPV_EVENT_LOG_MESSAGE => {
                        if let Some((prefix, level, text)) = unsafe { events::log_message(evt) } {
                            let effective = effective_log_level(&prefix, &level, &text);
                            let line = format!("[mpv:{}] {}", prefix, crate::redact::redact(&text));
                            match effective {
                                "fatal" | "error" => error!("{}", line),
                                "warn" => warn!("{}", line),
                                "info" | "status" | "v" => info!("{}", line),
                                _ => debug!("{}", line),
                            }
                        }
                    }
                    mpv_sys::MPV_EVENT_FILE_LOADED => {
                        info!("mpv: file-loaded");
                        let _ = evt_tx.send(Event::FileLoaded);
                    }
                    mpv_sys::MPV_EVENT_PLAYBACK_RESTART => {
                        debug!("mpv: playback-restart");
                        let _ = evt_tx.send(Event::PlaybackStarted);
                    }
                    mpv_sys::MPV_EVENT_END_FILE => {
                        let reason = unsafe { events::end_file_reason(evt) };
                        warn!("mpv: end-file (reason={})", reason);
                        let _ = evt_tx.send(Event::EndOfFile { reason });
                    }
                    mpv_sys::MPV_EVENT_VIDEO_RECONFIG => {
                        let w = mpv
                            .get_property_string("dwidth")
                            .unwrap_or_else(|| "?".into());
                        let h = mpv
                            .get_property_string("dheight")
                            .unwrap_or_else(|| "?".into());
                        info!("mpv: video-reconfig {}x{}", w, h);
                    }
                    mpv_sys::MPV_EVENT_AUDIO_RECONFIG => {
                        info!("mpv: audio-reconfig");
                    }
                    mpv_sys::MPV_EVENT_PROPERTY_CHANGE => {
                        if let Some((n, v)) = unsafe { events::property_change(evt) } {
                            let _ = evt_tx.send(Event::PropertyChanged { name: n, value: v });
                        }
                    }
                    mpv_sys::MPV_EVENT_SHUTDOWN => {
                        warn!("mpv: shutdown event - exiting player thread");
                        return;
                    }
                    _ => {
                        debug!("mpv event id={}", id);
                    }
                }

                if rctx.has_new_frame() {
                    if let Err(e) = rctx.render_into_frames() {
                        error!("render failed: {}", e);
                        let _ = evt_tx.send(Event::Error {
                            msg: format!("render: {}", e),
                        });
                        thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    thread::sleep(Duration::from_micros(500));
                }
            }
        })?;

    Ok(PlayerHandle {
        cmd_tx,
        evt_rx: parking_lot::Mutex::new(evt_rx),
        frames,
    })
}

/// The `loadfile` command for `url`. A URL that carries headers
/// (`url|User-Agent=…`, see `m3u::split_headers`) hands them to mpv as options
/// of this one file: mpv sets them for its playback and puts the defaults back
/// after it, so the next load goes without. `-1` is the playlist index that
/// mpv 0.38 put before the options; it means none.
///
/// The referrer goes as a header field (`http-header-fields`), not as mpv's
/// `referrer` option. Measured on the vendored libmpv (git db73857, client API
/// 2.5): after a file, a per-file `referrer` comes back as an empty string and
/// every later load sends an empty `Referer:`, while a per-file header list
/// comes back empty and the next load sends none. (The player sets no
/// `http-header-fields` of its own, so empty is what it was.)
pub fn loadfile_args(url: &str) -> Vec<String> {
    let (target, headers) = crate::m3u::split_headers(url);
    let mut options = Vec::new();
    if let Some(agent) = &headers.user_agent {
        options.push(format!("user-agent={}", option_value(agent)));
    }
    if let Some(referrer) = &headers.referrer {
        options.push(format!(
            "http-header-fields={}",
            option_value(&referrer_field(referrer))
        ));
    }
    let mut args = vec!["loadfile".to_owned(), target.to_owned()];
    if !options.is_empty() {
        args.extend(["replace".to_owned(), "-1".to_owned(), options.join(",")]);
    }
    args
}

/// An option value as mpv's `key=value,…` lists take any text, commas and
/// colons included: `%<bytes>%<value>`.
fn option_value(value: &str) -> String {
    format!("%{}%{}", value.len(), value)
}

/// A referrer as the one entry it is in mpv's `http-header-fields` list. mpv
/// splits that list at commas, and a backslash in front of a comma keeps it in
/// the entry. A backslash means nothing anywhere else there (measured on the
/// vendored libmpv: `\\` stays two backslashes), so only the commas get one.
fn referrer_field(referrer: &str) -> String {
    format!("Referer: {}", referrer.replace(',', "\\,"))
}

/// Downgrade the effective log level for known-benign mpv messages so the
/// ERROR view stays signal-only.
///
/// Entries today (all share the pattern "ffmpeg emits scary text but mpv's
/// upper layer handles it without disruption"):
///
/// 1. HLS manifest HTTP-EOF: Xtream-style portals close the HLS playlist
///    response without `Content-Length` / chunked encoding -> ffmpeg flags
///    "Error reading HTTP response: End of file" but mpv reads the bytes
///    fine and continues playback.
///
/// 2. H.264 startup PPS/SPS misses: when joining a live HLS stream
///    mid-segment the demuxer sees B/P frames that reference
///    Picture/Sequence Parameter Sets it hasn't seen yet (those are
///    codec config blobs that only appear at I-frames). Until the first
///    keyframe arrives (~0.5s after zap) we get a flood of
///    "non-existing PPS 0", "non-existing SPS 0", "no frame!". Decoding
///    then starts cleanly. The flood was making the log unreadable on
///    every channel switch.
fn effective_log_level<'a>(prefix: &str, level: &'a str, text: &str) -> &'a str {
    if level != "error" {
        return level;
    }
    if prefix == "ffmpeg" && text.contains("Error reading HTTP response: End of file") {
        return "debug";
    }
    let from_ffmpeg = prefix == "ffmpeg" || prefix.starts_with("ffmpeg/");
    if from_ffmpeg
        && (text.contains("non-existing PPS")
            || text.contains("non-existing SPS")
            || text == "h264: no frame!"
            || text.ends_with(" no frame!"))
    {
        return "debug";
    }
    level
}

#[cfg(test)]
mod tests {
    use super::effective_log_level;

    #[test]
    fn downgrades_benign_http_eof_from_ffmpeg() {
        assert_eq!(
            effective_log_level(
                "ffmpeg",
                "error",
                "http: Error reading HTTP response: End of file"
            ),
            "debug"
        );
    }

    #[test]
    fn leaves_real_ffmpeg_errors_alone() {
        assert_eq!(
            effective_log_level("ffmpeg", "error", "Connection refused"),
            "error"
        );
        assert_eq!(
            effective_log_level("ffmpeg", "error", "HTTP 404 Not Found"),
            "error"
        );
    }

    #[test]
    fn does_not_match_other_prefixes() {
        assert_eq!(
            effective_log_level(
                "hls",
                "error",
                "http: Error reading HTTP response: End of file"
            ),
            "error"
        );
    }

    #[test]
    fn does_not_match_non_error_levels() {
        assert_eq!(
            effective_log_level(
                "ffmpeg",
                "info",
                "http: Error reading HTTP response: End of file"
            ),
            "info"
        );
    }

    #[test]
    fn downgrades_h264_startup_pps_sps_noise() {
        assert_eq!(
            effective_log_level("ffmpeg", "error", "NULL: non-existing PPS 0 referenced"),
            "debug"
        );
        assert_eq!(
            effective_log_level(
                "ffmpeg/video",
                "error",
                "h264: non-existing SPS 0 referenced in buffering period"
            ),
            "debug"
        );
        assert_eq!(
            effective_log_level("ffmpeg/video", "error", "h264: no frame!"),
            "debug"
        );
    }

    #[test]
    fn leaves_real_h264_errors_alone() {
        // Different text about h264 - keep as error.
        assert_eq!(
            effective_log_level("ffmpeg/video", "error", "h264: decode_slice_header error"),
            "error"
        );
        assert_eq!(
            effective_log_level("ffmpeg", "error", "Invalid data found when processing input"),
            "error"
        );
    }

    #[test]
    fn a_plain_url_is_a_plain_loadfile() {
        assert_eq!(
            super::loadfile_args("http://127.0.0.1:5555/live/0.ts"),
            ["loadfile", "http://127.0.0.1:5555/live/0.ts"]
        );
    }

    #[test]
    fn headers_become_options_of_that_one_file() {
        let url = crate::m3u::with_headers(
            "https://a.example/1.m3u8".into(),
            Some("Mozilla/5.0 (X11; Linux), like Gecko"),
            Some("https://www.example.com/a:b,c"),
        );
        // After mpv 0.38's index (-1: none), each value quoted by its length
        // in bytes: commas and colons inside it are no separators. The
        // referrer is one entry of the `http-header-fields` list, which mpv
        // splits at commas: the comma in it gets a backslash.
        assert_eq!(
            super::loadfile_args(&url),
            [
                "loadfile",
                "https://a.example/1.m3u8",
                "replace",
                "-1",
                "user-agent=%36%Mozilla/5.0 (X11; Linux), like Gecko,http-header-fields=%39%Referer: https://www.example.com/a:b\\,c",
            ]
        );
    }

    #[test]
    fn a_referrer_is_a_header_field_not_the_referrer_option() {
        // libmpv gives a per-file `referrer` back as an empty string, and every
        // later load then sends an empty `Referer:`. A per-file header list
        // comes back empty: the next load sends none.
        let url = crate::m3u::with_headers(
            "http://a.example/1".into(),
            None,
            Some("https://www.example.com/a:b"),
        );
        assert_eq!(
            super::loadfile_args(&url),
            [
                "loadfile",
                "http://a.example/1",
                "replace",
                "-1",
                "http-header-fields=%36%Referer: https://www.example.com/a:b",
            ]
        );
    }

    #[test]
    fn only_a_comma_in_a_referrer_gets_a_backslash() {
        // In mpv's string lists a backslash means something in front of a comma
        // only (measured on the vendored libmpv: `\\` stays two backslashes),
        // so a backslash of the referrer's own is left as it is.
        let url = crate::m3u::with_headers(
            "http://a.example/1".into(),
            None,
            Some(r"http://r.example/a\b,c\,d"),
        );
        assert_eq!(
            super::loadfile_args(&url)[4],
            r"http-header-fields=%36%Referer: http://r.example/a\b\,c\\,d"
        );
    }

    #[test]
    fn an_option_value_is_counted_in_bytes() {
        let url = crate::m3u::with_headers("http://a.example/1".into(), Some("Zürich-TV"), None);
        assert_eq!(super::loadfile_args(&url)[4], "user-agent=%10%Zürich-TV");
    }

    #[test]
    fn a_referrer_is_counted_in_bytes_with_its_field_name() {
        let url = crate::m3u::with_headers(
            "http://a.example/1".into(),
            None,
            Some("http://zürich.example/"),
        );
        assert_eq!(
            super::loadfile_args(&url)[4],
            "http-header-fields=%32%Referer: http://zürich.example/"
        );
    }
}
