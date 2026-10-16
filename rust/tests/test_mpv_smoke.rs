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
