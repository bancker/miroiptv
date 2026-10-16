pub mod events;
pub mod mpv;
pub mod mpv_sys;
pub mod render;
pub mod tracks;

pub use mpv::{Mpv, MpvError};
pub use render::{FrameBus, FrameFence, RenderCtx, RgbaFrame};

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

/// What the app tells the player. `Debug` for the tests only: a load holds a
/// stream URL, with the account in it.
#[cfg_attr(test, derive(Debug))]
pub enum Cmd {
    /// Load this URL as it is, byte for byte: an Xtream URL's password may
    /// hold a `|`.
    LoadUrl(String),
    /// Load this URL of an M3U list's stream, which may carry its headers:
    /// `url|User-Agent=…&Referer=…` (`loadfile_args`).
    LoadWithHeaders(String),
    /// Load a film, an episode or a trailer of the thuisbioscoop (spec
    /// 2026-10-15 §7): its URL byte for byte, with options of that one file.
    /// While it plays the player reports it (`Event::Vod`).
    LoadVod(VodLoad),
    Pause(bool),
    /// Stop playing; nothing is loaded afterwards.
    Stop,
    SeekRelative(f64),
    CycleAudio,
    CycleSubtitle,
    /// Plays audio track `id` (mpv's `aid`, an id of `Event::Vod`'s tracks);
    /// none: no sound track. A track choice, as `a`'s is: the load of another
    /// stream puts it back to auto (`tracks::TrackChoice`), so it goes after
    /// the load it is for.
    SetAudio(Option<i64>),
    /// Shows subtitle track `id` (mpv's `sid`); none: no subtitles. A track
    /// choice, as `SetAudio` is.
    SetSubtitles(Option<i64>),
    /// Mutes or unmutes (mpv's `mute`).
    SetMute(bool),
    SetWindowSize(u32, u32),
    Quit,
}

/// A load of the thuisbioscoop. `Debug` for the tests only: its URL holds the
/// account.
#[cfg_attr(test, derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct VodLoad {
    /// What plays, as it is.
    pub url: String,
    /// Where it starts, in seconds; 0 from the start.
    pub start: f64,
    /// A separate sound track: a trailer's audio, as yt-dlp gives it.
    pub audio_file: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    PlaybackStarted,
    EndOfFile {
        reason: String,
    },
    Error {
        msg: String,
    },
    PropertyChanged {
        name: String,
        value: String,
    },
    /// What `a` left selected: track `index` of `count` (both from 1), named
    /// by its language or title in `label`. `count` 0 means no audio track,
    /// `index` 0 none selected.
    AudioTrack {
        count: usize,
        index: usize,
        label: String,
    },
    Vod(VodEvent),
}

/// What the player reports of a `Cmd::LoadVod` while it plays
/// (`Event::Vod`), and of nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum VodEvent {
    /// Where it plays, in seconds: when the whole second changes, so once a
    /// second at most while it plays, and at once after a seek.
    Position(f64),
    /// How long it lasts, in seconds, when the whole second changes: a stream
    /// that tells its length as it is read (a TS) tells it once a second.
    Duration(f64),
    /// Its audio and subtitle tracks, as mpv lists them.
    Tracks(Vec<Track>),
}

/// An audio or subtitle track of what plays (mpv's `track-list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// What `Cmd::SetAudio` and `Cmd::SetSubtitles` take.
    pub id: i64,
    pub kind: TrackKind,
    /// Its language as the file names it ("nld", "eng"); empty when none.
    pub lang: String,
    /// Its title as the file names it; empty when none.
    pub title: String,
    /// It plays or shows now.
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

/// The `reply_userdata` the properties of a VOD load are observed with:
/// `time-pos`, `duration` and `track-list`, all unobserved at once by it. 1 is
/// `paused-for-cache`'s.
pub const OBSERVE_VOD: u64 = 2;

/// Lets a position through when its whole second differs from the last one's.
#[derive(Debug, Default)]
pub struct SecondGate {
    last: Option<i64>,
}

impl SecondGate {
    /// Whether `pos` (seconds) is news.
    pub fn pass(&mut self, pos: f64) -> bool {
        if !pos.is_finite() {
            return false;
        }
        let second = pos.floor() as i64;
        let news = self.last != Some(second);
        self.last = Some(second);
        news
    }

    /// A new file: its first position is news.
    pub fn reset(&mut self) {
        self.last = None;
    }
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
            let mut choice = tracks::TrackChoice::new();
            // A VOD load is playing: its properties are observed and reported.
            let mut vod = false;
            // Its file has started (`MPV_EVENT_START_FILE`): what the properties
            // say is its own from then on, not the file before it's.
            let mut vod_started = false;
            let (mut seconds, mut lengths) = (SecondGate::default(), SecondGate::default());

            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(
                            load @ (Cmd::LoadUrl(_) | Cmd::LoadWithHeaders(_) | Cmd::LoadVod(_)),
                        ) => {
                            follow_vod(&mpv, &mut vod, matches!(load, Cmd::LoadVod(_)));
                            vod_started = false;
                            seconds.reset();
                            lengths.reset();
                            if let Some((args, line)) = loadfile_command(&load) {
                                if choice.load(&args[1]) {
                                    reset_tracks(&mpv);
                                }
                                info!("{}", line);
                                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                                if let Err(e) = mpv.command(&args) {
                                    warn!("loadfile failed: {}", e);
                                    let _ = evt_tx.send(Event::Error { msg: e.to_string() });
                                }
                            }
                        }
                        Ok(Cmd::Pause(p)) => {
                            let _ = mpv.set_property("pause", if p { "yes" } else { "no" });
                        }
                        Ok(Cmd::Stop) => {
                            info!("stop");
                            follow_vod(&mpv, &mut vod, false);
                            vod_started = false;
                            let _ = mpv.command(&["stop"]);
                        }
                        Ok(Cmd::SetAudio(id)) => {
                            set_track(&mut choice, "aid", id, |n, v| mpv.set_property(n, v));
                        }
                        Ok(Cmd::SetSubtitles(id)) => {
                            set_track(&mut choice, "sid", id, |n, v| mpv.set_property(n, v));
                        }
                        Ok(Cmd::SetMute(on)) => {
                            debug!("mute {}", on);
                            let _ = mpv.set_property("mute", if on { "yes" } else { "no" });
                        }
                        Ok(Cmd::SeekRelative(s)) => {
                            let _ = mpv.command(&["seek", &s.to_string(), "relative"]);
                        }
                        Ok(Cmd::CycleAudio) => {
                            let _ = evt_tx.send(cycle_audio(&mpv, &mut choice));
                        }
                        Ok(Cmd::CycleSubtitle) => {
                            if mpv.command(&["cycle", "sub"]).is_ok() {
                                choice.chosen();
                            }
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
                    mpv_sys::MPV_EVENT_START_FILE => {
                        // What the VOD properties say from here on is this load's.
                        vod_started = true;
                        seconds.reset();
                        lengths.reset();
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
                        vod_started = false;
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
                    mpv_sys::MPV_EVENT_PROPERTY_CHANGE
                        if unsafe { (*evt).reply_userdata } == OBSERVE_VOD =>
                    {
                        // A late change of a load that is no VOD load any more,
                        // or of the file before this one, says nothing.
                        let report = match vod && vod_started {
                            true => unsafe { events::vod_report(evt) },
                            false => None,
                        };
                        let news = match &report {
                            Some(VodEvent::Position(pos)) => seconds.pass(*pos),
                            Some(VodEvent::Duration(length)) => lengths.pass(*length),
                            Some(_) => true,
                            None => false,
                        };
                        if let (true, Some(report)) = (news, report) {
                            let _ = evt_tx.send(Event::Vod(report));
                        }
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

/// `a`: the next audio track of what plays, round to the first, never "no
/// audio". mpv's own `cycle audio` passes through "no", and an `aid` set at
/// runtime outlives the file: one press on a one-track stream silenced every
/// later stream. The event says what is selected now, for the app's toast.
fn cycle_audio(mpv: &Mpv, choice: &mut tracks::TrackChoice) -> Event {
    let list = tracks::audio_tracks(|name| mpv.get_property_string(name));
    let count = list.len();
    let selected = list.iter().position(|t| t.selected);
    let answer = |index: Option<usize>| Event::AudioTrack {
        count,
        index: index.map_or(0, |i| i + 1),
        label: index.map(|i| list[i].label()).unwrap_or_default(),
    };
    match tracks::pick_next(&list) {
        tracks::Pick::None => {
            info!("audio: no track");
            answer(None)
        }
        tracks::Pick::OnlyOne => {
            info!("audio: one track, kept");
            answer(selected)
        }
        tracks::Pick::Next(i) => match mpv.set_property("aid", &list[i].id.to_string()) {
            Ok(()) => {
                choice.chosen();
                info!("audio: track {}/{} ({})", i + 1, count, list[i].label());
                answer(Some(i))
            }
            Err(e) => {
                warn!("audio: aid={} failed: {}", list[i].id, e);
                answer(selected)
            }
        },
    }
}

/// A track chosen with `a` or `s` is mpv's choice for every later file too
/// (an `aid` or `sid` set at runtime outlives the file): another stream starts
/// from mpv's own choice again.
fn reset_tracks(mpv: &Mpv) {
    info!("audio and subtitles back to auto for another stream");
    for name in ["aid", "sid"] {
        if let Err(e) = mpv.set_property(name, "auto") {
            warn!("{}=auto failed: {}", name, e);
        }
    }
}

/// `Cmd::SetAudio` and `Cmd::SetSubtitles`: `aid` or `sid` (`name`) set
/// through `set` to track `id`, or to `no` for none. A track choice, as `a`'s
/// and `s`'s are: mpv keeps it for every later file, so the load of another
/// stream puts it back to auto (`TrackChoice`), and the app sends it after the
/// load it is for. A value mpv does not take chooses nothing.
fn set_track(
    choice: &mut tracks::TrackChoice,
    name: &str,
    id: Option<i64>,
    set: impl FnOnce(&str, &str) -> Result<(), MpvError>,
) {
    let value = id.map_or_else(|| "no".to_owned(), |id| id.to_string());
    debug!("{} {}", name, value);
    if set(name, &value).is_ok() {
        choice.chosen();
    }
}

/// Starts or stops observing a VOD load's properties when what loads is or is
/// no VOD load: live TV reports nothing of them, so it costs nothing.
fn follow_vod(mpv: &Mpv, vod: &mut bool, now_vod: bool) {
    if now_vod == *vod {
        return;
    }
    *vod = now_vod;
    if now_vod {
        for (name, format) in [
            ("time-pos", mpv_sys::MPV_FORMAT_DOUBLE),
            ("duration", mpv_sys::MPV_FORMAT_DOUBLE),
            ("track-list", mpv_sys::MPV_FORMAT_STRING),
        ] {
            if let Err(e) = mpv.observe_property(name, format, OBSERVE_VOD) {
                warn!("observe {} failed: {}", name, e);
            }
        }
    } else if let Err(e) = mpv.unobserve_property(OBSERVE_VOD) {
        warn!("unobserve failed: {}", e);
    }
}

/// What the player thread runs for a load, and the line it logs (`None` for a
/// command that is no load). A plain load hands mpv its URL as it is, as 0.6.1
/// did; only a load with headers has them split off (`loadfile_args`). The line
/// names the headers a load sends, never their values, and its address goes
/// through `redact`. A VOD load's line names its host and options only: a
/// trailer's address holds tokens `redact` does not know.
pub fn loadfile_command(cmd: &Cmd) -> Option<(Vec<String>, String)> {
    if let Cmd::LoadVod(load) = cmd {
        let mut said = Vec::new();
        if load.start.is_finite() && load.start > 0.0 {
            said.push(format!("start {} s", load.start));
        }
        if load.audio_file.is_some() {
            said.push("audio file".to_owned());
        }
        let host = crate::redact::url_host(&load.url);
        let line = match said.is_empty() {
            true => format!("loadfile vod from {host}"),
            false => format!("loadfile vod from {host} ({})", said.join(", ")),
        };
        return Some((vod_loadfile_args(load), line));
    }
    let (args, sent) = match cmd {
        Cmd::LoadUrl(url) => (vec!["loadfile".to_owned(), url.clone()], Vec::new()),
        Cmd::LoadWithHeaders(url) => {
            let (_, headers) = crate::m3u::split_headers(url);
            let sent: Vec<&str> = [
                ("user-agent", headers.user_agent.is_some()),
                ("referrer", headers.referrer.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, on)| on.then_some(name))
            .collect();
            (loadfile_args(url), sent)
        }
        _ => return None,
    };
    let address = crate::redact::redact(&args[1]);
    let line = if sent.is_empty() {
        format!("loadfile {address}")
    } else {
        format!("loadfile {address} ({})", sent.join(", "))
    };
    Some((args, line))
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

/// The `loadfile` command of a VOD load: its URL as it is, byte for byte, and
/// `start` and `audio-file` as options of that one file (mpv puts them back
/// after it). A load from the start without a sound track is a plain loadfile,
/// as a film from `f` is.
pub fn vod_loadfile_args(load: &VodLoad) -> Vec<String> {
    let mut options = Vec::new();
    if load.start.is_finite() && load.start > 0.0 {
        options.push(format!("start={}", load.start));
    }
    if let Some(audio) = &load.audio_file {
        options.push(format!("audio-file={}", option_value(audio)));
    }
    let mut args = vec!["loadfile".to_owned(), load.url.clone()];
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
    fn an_xtream_url_goes_to_mpv_byte_for_byte_with_a_pipe_in_its_password() {
        // 0.6.1 sent a film's or an episode's URL as it is, and a password may
        // hold a `|`.
        for url in [
            "http://h:80/movie/u/pa|ss/9.mkv",
            "http://h:80/series/u/pa|ss/7.mp4",
        ] {
            let load = super::Cmd::LoadUrl(url.into());
            let (args, line) = super::loadfile_command(&load).unwrap();
            assert_eq!(args, ["loadfile", url]);
            assert!(!line.contains("pa|ss"), "{line}");
        }
    }

    #[test]
    fn a_lists_load_hands_its_headers_over_and_its_line_names_only_them() {
        use super::Cmd::LoadWithHeaders;
        let url = crate::m3u::with_headers(
            "https://a.example/1.m3u8".into(),
            Some("Secret-Agent/1"),
            Some("https://ref.example/x"),
        );
        let (args, line) = super::loadfile_command(&LoadWithHeaders(url.clone())).unwrap();
        assert_eq!(args, super::loadfile_args(&url), "options of that one file");
        assert_eq!(args[1], "https://a.example/1.m3u8");
        assert_eq!(
            line,
            "loadfile https://a.example/1.m3u8 (user-agent, referrer)"
        );
        let one = crate::m3u::with_headers("http://a.example/2".into(), Some("VLC"), None);
        let (_, line) = super::loadfile_command(&LoadWithHeaders(one)).unwrap();
        assert_eq!(line, "loadfile http://a.example/2 (user-agent)");
        let none = LoadWithHeaders("http://a.example/3".into());
        let (args, line) = super::loadfile_command(&none).unwrap();
        assert_eq!(args, ["loadfile", "http://a.example/3"]);
        assert_eq!(line, "loadfile http://a.example/3");
        assert!(super::loadfile_command(&super::Cmd::Stop).is_none());
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

    #[test]
    fn a_track_the_app_sets_goes_back_to_auto_when_another_stream_loads() {
        // An `aid` or `sid` set at runtime outlives the file: a subtitle track
        // set on a film would follow the user to a channel. `SetAudio` and
        // `SetSubtitles` are choices as `a`'s and `s`'s are (`TrackChoice`).
        let film = "http://h:80/movie/u/p/5001.mkv";
        let channel = "http://127.0.0.1:5555/live/7.ts";
        for (name, id, value) in [
            ("aid", Some(2), "2"),
            ("aid", None, "no"),
            ("sid", Some(3), "3"),
            ("sid", None, "no"),
        ] {
            let mut choice = super::tracks::TrackChoice::new();
            assert!(!choice.load(film));
            let mut set = Vec::new();
            super::set_track(&mut choice, name, id, |n, v| {
                set.push(format!("{n}={v}"));
                Ok(())
            });
            assert_eq!(set, [format!("{name}={value}")]);
            assert!(!choice.load(film), "the same film again keeps {name}");
            assert!(choice.load(channel), "another stream: {name} back to auto");
        }
        // A track mpv does not take is no choice.
        let mut choice = super::tracks::TrackChoice::new();
        assert!(!choice.load(film));
        super::set_track(&mut choice, "sid", Some(3), |_, _| {
            Err(super::MpvError::NullHandle)
        });
        assert!(!choice.load(channel));
    }
}
