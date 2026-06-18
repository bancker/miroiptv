pub mod events;
pub mod mpv;
pub mod mpv_sys;
pub mod render;

pub use mpv::{Mpv, MpvError};
pub use render::{FrameBus, RenderCtx, RgbaFrame};

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub enum Cmd {
    LoadUrl(String),
    Pause(bool),
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
                            info!("loadfile {}", url);
                            if let Err(e) = mpv.command(&["loadfile", &url]) {
                                warn!("loadfile failed: {}", e);
                                let _ = evt_tx.send(Event::Error { msg: e.to_string() });
                            }
                        }
                        Ok(Cmd::Pause(p)) => {
                            let _ = mpv.set_property("pause", if p { "yes" } else { "no" });
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
                            let line = format!("[mpv:{}] {}", prefix, text);
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
}
