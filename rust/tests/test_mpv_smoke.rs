use bongo_player::player::Mpv;

#[test]
#[ignore]
fn create_and_destroy() {
    let mpv = Mpv::new().expect("mpv init");
    let v = mpv.get_property_string("mpv-version");
    assert!(v.is_some(), "mpv-version should be readable");
    println!("mpv-version: {}", v.unwrap());
}

#[test]
#[ignore]
fn runs_none_of_mpvs_builtin_scripts() {
    // Each is a Lua VM on its own thread (~2.5 MB together) that the app never
    // uses; ytdl_hook even launches yt-dlp on the portal URL when a stream
    // fails to open - another portal session, seconds later.
    let mpv = Mpv::new().expect("mpv init");
    for opt in [
        "ytdl",
        "load-stats-overlay",
        "load-console",
        "load-auto-profiles",
        "load-select",
        "load-positioning",
        "load-commands",
    ] {
        assert_eq!(
            mpv.get_property_string(opt).as_deref(),
            Some("no"),
            "{opt} is on"
        );
    }
}

#[test]
#[ignore]
fn loadfile_dummy_succeeds() {
    let mpv = Mpv::new().expect("mpv init");
    let r = mpv.command(&[
        "loadfile",
        "av://lavfi:smptebars=size=640x360:rate=25:duration=1",
    ]);
    assert!(r.is_ok(), "loadfile should succeed: {:?}", r);
}

#[test]
#[ignore]
fn the_defect_check_sends_mpvs_own_user_agent() {
    // A channel that names no User-Agent is asked with the one mpv sent it.
    let mpv = Mpv::new().expect("mpv init");
    assert_eq!(
        mpv.get_property_string("user-agent").as_deref(),
        Some(bongo_player::stream_check::MPV_USER_AGENT),
        "set stream_check::MPV_USER_AGENT to what mpv says"
    );
}

/// The mock portal's test stream (one AAC track; `e2e/make-segments.ps1`), or
/// `None` on a checkout without the segments: the test then says so and ends.
fn test_stream() -> Option<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("e2e/segs/index.m3u8");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

/// An mpv without a window or a sound card: the tests read its track choice.
/// Video stays selected, as in the app: with no track selected at all mpv ends
/// the file with an error instead of playing it silent.
fn quiet_mpv() -> Mpv {
    let mpv = Mpv::new().expect("mpv init");
    mpv.set_property("vo", "null").expect("vo=null");
    mpv.set_property("ao", "null").expect("ao=null");
    mpv
}

/// The app's player thread (`player::spawn`) without a sound card: mpv's audio
/// output is `null` (`spawn_with`), so the tests never open the developer's and
/// run on a PC without one. Its video goes to the render API, as in the app: no
/// window either.
fn quiet_player() -> bongo_player::player::PlayerHandle {
    let ao = ("ao".to_owned(), "null".to_owned());
    bongo_player::player::spawn_with(320, 240, vec![ao]).expect("player")
}

/// Loads `url` and waits for mpv's file-loaded, 15 s at most; false on an
/// end-file of the new file before it, or on the deadline. A load that
/// replaces a file (`replacing`) first ends that one: its end-file is passed.
fn load_and_wait(mpv: &Mpv, url: &str, replacing: bool) -> bool {
    use bongo_player::player::{events, mpv_sys};
    mpv.command(&["loadfile", url]).expect("loadfile");
    let mut old_file_ends = replacing;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let evt = mpv.wait_event(0.2);
        match unsafe { (*evt).event_id } {
            mpv_sys::MPV_EVENT_FILE_LOADED => return true,
            mpv_sys::MPV_EVENT_END_FILE if old_file_ends => old_file_ends = false,
            mpv_sys::MPV_EVENT_END_FILE => {
                let reason = unsafe { events::end_file_reason(evt) };
                println!("end-file before file-loaded: {reason}");
                return false;
            }
            _ => {}
        }
    }
    false
}

#[test]
#[ignore]
fn mpvs_own_cycle_silences_a_one_track_stream_and_every_later_one() {
    // The bug: `a` was mpv's `cycle audio`. On a one-track stream (Bloomberg
    // on an M3U list) it goes from the track to "no", and that choice stayed
    // for every later file. The player now picks a track itself
    // (`tracks::pick_next`) and puts a choice back to auto for another
    // stream (`TrackChoice`).
    let Some(url) = test_stream() else {
        println!("no e2e/segs/index.m3u8: run e2e/make-segments.ps1 first");
        return;
    };
    let mpv = quiet_mpv();
    assert!(load_and_wait(&mpv, &url, false), "the test stream loads");
    assert_eq!(
        mpv.get_property_string("aid").as_deref(),
        Some("1"),
        "mpv picks the one track"
    );
    mpv.command(&["cycle", "audio"]).unwrap();
    assert_eq!(
        mpv.get_property_string("aid").as_deref(),
        Some("no"),
        "mpv's cycle goes from the one track to no audio"
    );
    assert!(load_and_wait(&mpv, &url, true), "the stream loads again");
    assert_eq!(
        mpv.get_property_string("aid").as_deref(),
        Some("no"),
        "the choice outlives the file: why the player puts it back"
    );
    mpv.set_property("aid", "auto").unwrap();
    assert!(
        load_and_wait(&mpv, &url, true),
        "the stream loads once more"
    );
    assert_eq!(
        mpv.get_property_string("aid").as_deref(),
        Some("1"),
        "auto picks the track again"
    );
}

#[test]
#[ignore]
fn the_test_streams_one_audio_track_is_read_and_kept() {
    use bongo_player::player::tracks::{audio_tracks, pick_next, Pick};
    let Some(url) = test_stream() else {
        println!("no e2e/segs/index.m3u8: run e2e/make-segments.ps1 first");
        return;
    };
    let mpv = quiet_mpv();
    assert!(load_and_wait(&mpv, &url, false), "the test stream loads");
    let tracks = audio_tracks(|name| mpv.get_property_string(name));
    assert_eq!(tracks.len(), 1, "{tracks:?}");
    assert_eq!(tracks[0].id, 1);
    assert!(tracks[0].selected);
    assert_eq!(pick_next(&tracks), Pick::OnlyOne);
}

/// The log lines mpv writes while it loads `url`, up to its first picture or
/// 15 s at most. A load that replaces a file first ends that one.
fn log_of_a_load(mpv: &Mpv, url: &str, mut replacing: bool) -> Vec<String> {
    use bongo_player::player::{events, mpv_sys};
    mpv.command(&["loadfile", url]).expect("loadfile");
    let mut lines = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let evt = mpv.wait_event(0.2);
        match unsafe { (*evt).event_id } {
            mpv_sys::MPV_EVENT_LOG_MESSAGE => {
                if let Some((_, _, text)) = unsafe { events::log_message(evt) } {
                    lines.push(text);
                }
            }
            mpv_sys::MPV_EVENT_PLAYBACK_RESTART => break,
            mpv_sys::MPV_EVENT_END_FILE if replacing => replacing = false,
            mpv_sys::MPV_EVENT_END_FILE => break,
            _ => {}
        }
    }
    lines
}

#[test]
#[ignore]
fn mpv_marks_a_selected_track_and_one_it_leaves_off_in_its_log() {
    // e2e's N-K reads these marks in the app log: U+25CF before a track that
    // plays, U+25CB before one mpv leaves off (no audio).
    let Some(url) = test_stream() else {
        println!("no e2e/segs/index.m3u8: run e2e/make-segments.ps1 first");
        return;
    };
    let audio = |lines: &[String], mark: char| {
        lines
            .iter()
            .any(|l| l.trim_start().starts_with(&format!("{mark} Audio")))
    };
    let mpv = quiet_mpv();
    let lines = log_of_a_load(&mpv, &url, false);
    assert!(audio(&lines, '\u{25CF}'), "{lines:#?}");
    assert!(!audio(&lines, '\u{25CB}'), "{lines:#?}");
    mpv.set_property("aid", "no").unwrap();
    let lines = log_of_a_load(&mpv, &url, true);
    assert!(audio(&lines, '\u{25CB}'), "{lines:#?}");
    assert!(!audio(&lines, '\u{25CF}'), "{lines:#?}");
}

#[test]
#[ignore]
fn a_vod_load_reports_its_length_position_and_tracks_and_live_tv_reports_none() {
    use bongo_player::player::{Cmd, Event, TrackKind, VodEvent, VodLoad};
    use std::time::{Duration, Instant};
    let player = quiet_player();
    // No sound on the developer's speakers.
    player.cmd_tx.send(Cmd::SetMute(true)).unwrap();
    let vod = VodLoad {
        url: "av://lavfi:testsrc2=size=320x240:rate=25:duration=8".into(),
        start: 1.0,
        audio_file: Some("av://lavfi:sine=frequency=440:duration=8".into()),
    };
    player.cmd_tx.send(Cmd::LoadVod(vod)).unwrap();
    let mut events = player.evt_rx.lock();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut length, mut first, mut audio) = (None, None, false);
    while Instant::now() < deadline && !(length.is_some() && first.is_some() && audio) {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Duration(d))) => length = Some(d),
            Ok(Event::Vod(VodEvent::Position(p))) => {
                first.get_or_insert(p);
            }
            Ok(Event::Vod(VodEvent::Tracks(tracks))) => {
                audio |= tracks.iter().any(|t| t.kind == TrackKind::Audio);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    // A lavfi source has no length up front: mpv tells it as it reads it.
    assert!(length.is_some_and(|d| d > 0.0), "{length:?}");
    assert!(
        first.is_some_and(|p| p >= 0.9),
        "start=1 is kept: {first:?}"
    );
    assert!(
        audio,
        "the audio file is a track of mpv's track-list, read as JSON"
    );
    // `Cmd::SetAudio(None)` sets aid=no: the track list says no audio plays.
    // (`SetSubtitles` gets no check: lavfi has no subtitle track.)
    while events.try_recv().is_ok() {}
    player.cmd_tx.send(Cmd::SetAudio(None)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut silent = false;
    while Instant::now() < deadline && !silent {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Tracks(tracks))) => {
                let audio: Vec<_> = tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio)
                    .collect();
                silent = !audio.is_empty() && audio.iter().all(|t| !t.selected);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(silent, "Cmd::SetAudio(None) sets aid=no");
    // A VOD load after another reports its own position, never the first
    // file's. It goes right after a position of the first, so no other comes
    // in before the player takes it.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Position(_))) => break,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    while events.try_recv().is_ok() {}
    let second = VodLoad {
        url: "av://lavfi:testsrc2=size=320x240:rate=25:duration=8".into(),
        start: 0.0,
        audio_file: None,
    };
    player.cmd_tx.send(Cmd::LoadVod(second)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first = None;
    while Instant::now() < deadline && first.is_none() {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Position(p))) => first = Some(p),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(
        first.is_some_and(|p| p < 1.0),
        "the second load's own position: {first:?}"
    );
    // `Cmd::SetAudio` is a track choice, as `a`'s is (`TrackChoice`): the
    // second load, the same film again, kept aid=no, and the load of another
    // film puts it back to auto, so its sound track plays.
    while events.try_recv().is_ok() {}
    let other = VodLoad {
        url: "av://lavfi:testsrc2=size=320x180:rate=25:duration=8".into(),
        start: 0.0,
        audio_file: Some("av://lavfi:sine=frequency=330:duration=8".into()),
    };
    player.cmd_tx.send(Cmd::LoadVod(other)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut plays = false;
    while Instant::now() < deadline && !plays {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Tracks(tracks))) => {
                plays = tracks
                    .iter()
                    .any(|t| t.kind == TrackKind::Audio && t.selected);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(plays, "another film's sound track plays: aid is auto again");
    // Live TV reports none of it.
    let live = "av://lavfi:smptebars=size=320x240:rate=25:duration=3";
    player.cmd_tx.send(Cmd::LoadUrl(live.into())).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}
    let quiet = Instant::now() + Duration::from_secs(2);
    while Instant::now() < quiet {
        if let Ok(Event::Vod(report)) = events.try_recv() {
            panic!("live TV reported {report:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    player.cmd_tx.send(Cmd::Quit).unwrap();
}

/// What the player thread reports, on the vendored libmpv without a sound card
/// (`quiet_player`). Run them one at a time (`--test-threads=1`): they count
/// reports against the clock.
mod vod_review {
    use super::quiet_player;
    use bongo_player::player::{
        events, mpv_sys, vod_loadfile_args, Cmd, Event, Mpv, PlayerHandle, TrackKind, VodEvent,
        VodLoad,
    };
    use std::time::{Duration, Instant};

    const V1: &str = "av://lavfi:testsrc2=size=320x240:rate=25:duration=8";
    const V2: &str = "av://lavfi:testsrc2=size=160x120:rate=25:duration=8";
    const A1: &str = "av://lavfi:sine=frequency=440:duration=8";

    fn vod(url: &str, start: f64, audio: Option<&str>) -> VodLoad {
        VodLoad {
            url: url.into(),
            start,
            audio_file: audio.map(str::to_owned),
        }
    }

    fn drain(p: &PlayerHandle) {
        let mut rx = p.evt_rx.lock();
        while rx.try_recv().is_ok() {}
    }

    /// The events of `secs` seconds, or up to the one `until` says yes to.
    fn events_for(
        p: &PlayerHandle,
        secs: f64,
        mut until: impl FnMut(&Event) -> bool,
    ) -> Vec<Event> {
        let mut out = vec![];
        let deadline = Instant::now() + Duration::from_secs_f64(secs);
        let mut rx = p.evt_rx.lock();
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(e) => {
                    let stop = until(&e);
                    out.push(e);
                    if stop {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        out
    }

    fn has_audio(e: &Event) -> bool {
        matches!(e, Event::Vod(VodEvent::Tracks(t))
            if t.iter().any(|t| t.kind == TrackKind::Audio))
    }

    fn audio_selected(e: &Event) -> bool {
        matches!(e, Event::Vod(VodEvent::Tracks(t))
            if t.iter().any(|t| t.kind == TrackKind::Audio && t.selected))
    }

    fn write_wav(path: &std::path::Path, secs: f64) {
        let rate = 8000u32;
        let samples = (rate as f64 * secs) as u32;
        let data_len = samples * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for n in 0..samples {
            let v = ((n as f64 * 0.2).sin() * 1000.0) as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// A local server that answers every request 404 at once; its port.
    fn not_found_server() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { continue };
                let mut req = [0u8; 4096];
                let _ = c.read(&mut req);
                let _ = c.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        port
    }

    #[test]
    #[ignore]
    fn nothing_is_reported_after_a_vod_ends() {
        // Without `VodFollow::end_file` the idle player's empty track list
        // would be reported as the VOD's.
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        p.cmd_tx
            .send(Cmd::LoadVod(vod(
                "av://lavfi:testsrc2=size=320x240:rate=25:duration=1",
                0.0,
                Some("av://lavfi:sine=frequency=440:duration=1"),
            )))
            .unwrap();
        let ev = events_for(&p, 10.0, |e| matches!(e, Event::EndOfFile { .. }));
        let reason = ev.iter().find_map(|e| match e {
            Event::EndOfFile { reason } => Some(reason.clone()),
            _ => None,
        });
        assert_eq!(reason.as_deref(), Some("eof"), "{ev:?}");
        assert!(ev.iter().any(has_audio), "it reported while it played");
        let after = events_for(&p, 2.0, |_| false);
        assert!(
            !after.iter().any(|e| matches!(e, Event::Vod(_))),
            "reported after the end: {after:?}"
        );
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn positions_and_lengths_each_come_once_a_whole_second() {
        // A lavfi source tells its length as it is read: with one gate for
        // both, every length would let the next position through.
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        p.cmd_tx
            .send(Cmd::LoadVod(vod(
                "av://lavfi:testsrc2=size=160x120:rate=25:duration=6",
                0.0,
                Some(A1),
            )))
            .unwrap();
        let ev = events_for(&p, 12.0, |e| matches!(e, Event::EndOfFile { .. }));
        let pick = |want_position: bool| -> Vec<f64> {
            ev.iter()
                .filter_map(|e| match e {
                    Event::Vod(VodEvent::Position(x)) if want_position => Some(*x),
                    Event::Vod(VodEvent::Duration(x)) if !want_position => Some(*x),
                    _ => None,
                })
                .collect()
        };
        let (pos, lens) = (pick(true), pick(false));
        assert!(pos.len() >= 4, "{pos:?}");
        for w in pos.windows(2) {
            assert_ne!(
                w[0].floor(),
                w[1].floor(),
                "two positions in one second: {pos:?}"
            );
        }
        for w in lens.windows(2) {
            assert_ne!(
                w[0].floor(),
                w[1].floor(),
                "two lengths in one second: {lens:?}"
            );
        }
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn a_vod_load_after_another_observes_the_properties_once() {
        // With an observer more per VOD load every change would come once more
        // (measured on such a build: 1, 2, 3 reports of one change at loads 1,
        // 2, 3).
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        let mut counts = vec![];
        for size in ["320x240", "160x120", "320x180"] {
            let url = format!("av://lavfi:testsrc2=size={size}:rate=25:duration=8");
            p.cmd_tx
                .send(Cmd::LoadVod(vod(&url, 0.0, Some(A1))))
                .unwrap();
            let ev = events_for(&p, 10.0, audio_selected);
            assert!(ev.iter().any(audio_selected), "{size}: {ev:?}");
            let _ = events_for(&p, 0.6, |_| false);
            drain(&p);
            p.cmd_tx.send(Cmd::SetAudio(None)).unwrap();
            let ev = events_for(&p, 1.5, |_| false);
            counts.push(
                ev.iter()
                    .filter(|e| matches!(e, Event::Vod(VodEvent::Tracks(_))))
                    .count(),
            );
        }
        assert_eq!(counts, [1, 1, 1], "Tracks reports of one SetAudio(None)");
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn the_audio_file_of_one_load_does_not_reach_the_next_and_a_keeps_the_one_track() {
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        p.cmd_tx.send(Cmd::LoadVod(vod(V1, 0.0, Some(A1)))).unwrap();
        let ev = events_for(&p, 10.0, has_audio);
        assert!(ev.iter().any(has_audio), "{ev:?}");
        // `a` during a VOD: cycle_audio, the one track kept, never no audio.
        drain(&p);
        p.cmd_tx.send(Cmd::CycleAudio).unwrap();
        let ev = events_for(&p, 3.0, |e| matches!(e, Event::AudioTrack { .. }));
        let answer = ev.iter().find_map(|e| match e {
            Event::AudioTrack { count, index, .. } => Some((*count, *index)),
            _ => None,
        });
        assert_eq!(answer, Some((1, 1)), "{ev:?}");
        drain(&p);
        p.cmd_tx.send(Cmd::LoadVod(vod(V2, 0.0, None))).unwrap();
        let ev = events_for(&p, 3.0, |_| false);
        assert!(ev.iter().any(|e| matches!(e, Event::FileLoaded)), "{ev:?}");
        assert!(
            !ev.iter().any(has_audio),
            "the audio file of the load before: {ev:?}"
        );
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    /// Runs `args` and waits for the playback restart of the new file (15 s
    /// at most); an end-file of a file it replaces is passed over.
    fn run_and_wait(mpv: &Mpv, args: &[&str], mut replacing: bool) -> Result<(), String> {
        mpv.command(args).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let evt = mpv.wait_event(0.2);
            match unsafe { (*evt).event_id } {
                mpv_sys::MPV_EVENT_PLAYBACK_RESTART => return Ok(()),
                mpv_sys::MPV_EVENT_END_FILE if replacing => replacing = false,
                mpv_sys::MPV_EVENT_END_FILE => {
                    return Err(format!("end-file: {}", unsafe {
                        events::end_file_reason(evt)
                    }))
                }
                _ => {}
            }
        }
        Err("deadline".into())
    }

    /// (type, external, external-filename) of every track.
    fn tracks_of(mpv: &Mpv) -> Vec<(String, String, String)> {
        let n: usize = mpv
            .get_property_string("track-list/count")
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (0..n)
            .map(|i| {
                let get = |k: &str| {
                    mpv.get_property_string(&format!("track-list/{i}/{k}"))
                        .unwrap_or_default()
                };
                (get("type"), get("external"), get("external-filename"))
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn odd_characters_reach_mpv_byte_for_byte_and_the_options_stay_with_their_file() {
        // Ruling 44 on libmpv: the URL as it is, `%n%` counted in bytes, and
        // start= and audio-file= of that one file.
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("a, b=c %41 ü d.wav");
        let extra = dir.path().join("e, f=g %42 h;i.wav");
        write_wav(&main, 3.0);
        write_wav(&extra, 3.0);
        let (main, extra) = (
            main.to_string_lossy().into_owned(),
            extra.to_string_lossy().into_owned(),
        );
        let mpv = Mpv::new().expect("mpv");
        mpv.set_property("vo", "null").unwrap();
        mpv.set_property("ao", "null").unwrap();
        let args = vod_loadfile_args(&VodLoad {
            url: main.clone(),
            start: 0.5,
            audio_file: Some(extra.clone()),
        });
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run_and_wait(&mpv, &args, false).expect("the odd file plays");
        assert_eq!(
            mpv.get_property_string("path").as_deref(),
            Some(main.as_str())
        );
        let pos = mpv.get_property_f64("time-pos");
        assert!(pos.is_some_and(|p| p >= 0.45), "start=0.5: {pos:?}");
        let tracks = tracks_of(&mpv);
        let audio: Vec<_> = tracks.iter().filter(|t| t.0 == "audio").collect();
        assert_eq!(audio.len(), 2, "{tracks:?}");
        assert!(
            audio.iter().any(|t| t.1 == "yes" && t.2 == extra),
            "the audio file, byte for byte: {tracks:?}"
        );
        // The next load without options: neither the start nor the file stays.
        run_and_wait(&mpv, &["loadfile", &main], true).expect("plays again");
        let pos = mpv.get_property_f64("time-pos");
        assert!(
            pos.is_some_and(|p| p < 0.3),
            "start went with its file: {pos:?}"
        );
        let tracks = tracks_of(&mpv);
        assert_eq!(
            tracks.iter().filter(|t| t.0 == "audio").count(),
            1,
            "the audio file went with its file: {tracks:?}"
        );
    }

    #[test]
    #[ignore]
    fn set_subtitles_sets_sid_on_a_vod_with_a_subtitle_track() {
        // lavfi has no subtitles; an EDL of a WAV and an SRT has one.
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("s, t=1 %41.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:08,000\nHallo\n\n").unwrap();
        let srt = srt.to_string_lossy().into_owned();
        let wav = dir.path().join("a.wav");
        write_wav(&wav, 8.0);
        let wav = wav.to_string_lossy().into_owned();
        let q = |s: &str| format!("%{}%{}", s.len(), s);
        let url = format!(
            "edl://!new_stream;!no_clip;{};!new_stream;!no_clip;{}",
            q(&wav),
            q(&srt)
        );
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        p.cmd_tx.send(Cmd::LoadVod(vod(&url, 0.0, None))).unwrap();
        let sub_of = |e: &Event| match e {
            Event::Vod(VodEvent::Tracks(t)) => t
                .iter()
                .find(|t| t.kind == TrackKind::Subtitle)
                .map(|t| (t.id, t.selected)),
            _ => None,
        };
        let ev = events_for(&p, 10.0, |e| sub_of(e).is_some());
        let (id, _) = ev
            .iter()
            .find_map(sub_of)
            .expect("a subtitle track, read as \"sub\"");
        drain(&p);
        p.cmd_tx.send(Cmd::SetSubtitles(Some(id))).unwrap();
        let ev = events_for(&p, 5.0, |e| sub_of(e) == Some((id, true)));
        assert!(
            ev.iter().any(|e| sub_of(e) == Some((id, true))),
            "sid={id}: {ev:?}"
        );
        drain(&p);
        p.cmd_tx.send(Cmd::SetSubtitles(None)).unwrap();
        let ev = events_for(&p, 5.0, |e| sub_of(e) == Some((id, false)));
        assert!(
            ev.iter().any(|e| sub_of(e) == Some((id, false))),
            "sid=no: {ev:?}"
        );
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn a_vod_load_that_fails_reports_no_position_or_length() {
        let port = not_found_server();
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        p.cmd_tx
            .send(Cmd::LoadVod(vod(
                "av://lavfi:testsrc2=size=160x120:rate=25:duration=8",
                2.0,
                Some(A1),
            )))
            .unwrap();
        let _ = events_for(&p, 10.0, audio_selected);
        let _ = events_for(&p, 1.0, |_| false);
        drain(&p);
        let url = format!("http://127.0.0.1:{port}/movie/u/p/1.mkv");
        p.cmd_tx.send(Cmd::LoadVod(vod(&url, 30.0, None))).unwrap();
        let error = |e: &Event| matches!(e, Event::EndOfFile { reason } if reason == "error");
        let ev = events_for(&p, 10.0, error);
        assert!(ev.iter().any(error), "{ev:?}");
        let after = events_for(&p, 1.5, |_| false);
        let vods: Vec<_> = ev
            .iter()
            .chain(after.iter())
            .filter(|e| matches!(e, Event::Vod(_)))
            .collect();
        assert!(
            vods.iter()
                .all(|e| matches!(e, Event::Vod(VodEvent::Tracks(t)) if t.is_empty())),
            "only an empty track list, never the file before it's: {vods:?}"
        );
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn a_vod_after_live_tv_reports_its_own_and_live_tv_after_it_nothing() {
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        for round in 0..5 {
            let live = format!(
                "av://lavfi:smptebars=size=320x240:rate=25:duration={}",
                30 + round
            );
            p.cmd_tx.send(Cmd::LoadUrl(live)).unwrap();
            let ev = events_for(&p, 5.0, |_| false);
            // What came before the VOD's end-file was sent before the player
            // took the live load (see `Event::Vod`): the VOD's own. From that
            // end on, nothing.
            let end = match round {
                0 => 0,
                _ => ev
                    .iter()
                    .position(|e| matches!(e, Event::EndOfFile { .. }))
                    .map_or(ev.len(), |i| i + 1),
            };
            assert!(
                !ev[end..].iter().any(|e| matches!(e, Event::Vod(_))),
                "live TV reported: {ev:?}"
            );
            drain(&p);
            p.cmd_tx.send(Cmd::LoadVod(vod(V2, 0.0, Some(A1)))).unwrap();
            let ev = events_for(&p, 3.0, |_| false);
            let first = ev.iter().find_map(|e| match e {
                Event::Vod(VodEvent::Position(x)) => Some(*x),
                _ => None,
            });
            assert!(first.is_some_and(|x| x < 1.0), "the VOD's own: {first:?}");
            assert!(
                ev.iter().all(|e| !matches!(e,
                    Event::Vod(VodEvent::Duration(d)) if *d > 8.5)),
                "never the live stream's length: {ev:?}"
            );
        }
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }

    #[test]
    #[ignore]
    fn back_to_back_loads_report_only_the_second() {
        // Two loads in one drain of the command channel: nothing of the first
        // (start=20, its sine, its length) is reported (measured: 0 of 15).
        let p = quiet_player();
        p.cmd_tx.send(Cmd::SetMute(true)).unwrap();
        let a = "av://lavfi:testsrc2=size=320x240:rate=25:duration=60";
        let a_audio = "av://lavfi:sine=frequency=440:duration=60";
        let mut stale = vec![];
        let mut seen = 0;
        for round in 0..15 {
            drain(&p);
            p.cmd_tx
                .send(Cmd::LoadVod(vod(a, 20.0, Some(a_audio))))
                .unwrap();
            p.cmd_tx.send(Cmd::LoadVod(vod(V2, 0.0, None))).unwrap();
            for e in events_for(&p, 1.5, |_| false) {
                match &e {
                    Event::Vod(VodEvent::Position(x)) if *x >= 10.0 => {
                        stale.push(format!("{round}: {e:?}"))
                    }
                    Event::Vod(VodEvent::Duration(d)) if *d >= 15.0 => {
                        stale.push(format!("{round}: {e:?}"))
                    }
                    e if has_audio(e) => stale.push(format!("{round}: {e:?}")),
                    Event::Vod(VodEvent::Position(_)) => seen += 1,
                    _ => {}
                }
            }
        }
        assert!(seen > 0, "the second load reports");
        assert!(stale.is_empty(), "{stale:#?}");
        p.cmd_tx.send(Cmd::Quit).unwrap();
    }
}
