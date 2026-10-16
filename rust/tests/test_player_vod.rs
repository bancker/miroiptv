//! The player's side of the thuisbioscoop (player/, spec 2026-10-15 §7): a
//! VOD load with options of that one file and a line without its address,
//! mpv's track list, and the once-a-second position.

use bongo_player::player::events::parse_tracks;
use bongo_player::player::{
    loadfile_command, vod_loadfile_args, Cmd, SecondGate, Track, TrackKind, VodLoad,
};

fn load(url: &str, start: f64, audio_file: Option<&str>) -> VodLoad {
    VodLoad {
        url: url.into(),
        start,
        audio_file: audio_file.map(str::to_owned),
    }
}

#[test]
fn a_film_goes_to_mpv_byte_for_byte_with_its_start_as_an_option_of_that_file() {
    // A password may hold a `|`: the URL goes as it is.
    let url = "http://h:80/movie/u/pa|ss/5001.mkv";
    assert_eq!(
        vod_loadfile_args(&load(url, 4380.0, None)),
        ["loadfile", url, "replace", "-1", "start=4380"]
    );
    assert_eq!(vod_loadfile_args(&load(url, 12.5, None))[4], "start=12.5");
    // From the start: a plain loadfile, as a film from `f` is.
    assert_eq!(vod_loadfile_args(&load(url, 0.0, None)), ["loadfile", url]);
    for odd in [-5.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            vod_loadfile_args(&load(url, odd, None)),
            ["loadfile", url],
            "{odd}"
        );
    }
}

#[test]
fn a_trailer_takes_its_sound_as_an_audio_file_counted_in_bytes() {
    let video = "https://rr1.example/videoplayback?expire=1&sig=a,b";
    let audio = "https://rr2.example/videoplayback?itag=140&sig=c,d";
    let args = vod_loadfile_args(&load(video, 0.0, Some(audio)));
    assert_eq!(
        args,
        [
            "loadfile".to_owned(),
            video.to_owned(),
            "replace".to_owned(),
            "-1".to_owned(),
            format!("audio-file=%{}%{audio}", audio.len()),
        ]
    );
    let both = vod_loadfile_args(&load(video, 30.0, Some(audio)));
    assert!(both[4].starts_with("start=30,audio-file=%"), "{}", both[4]);
}

#[test]
fn the_line_of_a_vod_load_names_its_host_never_its_address() {
    let film = Cmd::LoadVod(load("http://h:80/movie/user/secret/5001.mkv", 4380.0, None));
    let (args, line) = loadfile_command(&film).unwrap();
    assert_eq!(args[1], "http://h:80/movie/user/secret/5001.mkv");
    assert_eq!(line, "loadfile vod from h (start 4380 s)");
    let trailer = Cmd::LoadVod(load(
        "https://rr1.example/videoplayback?token=abc",
        0.0,
        Some("https://rr2.example/a?token=def"),
    ));
    let (_, line) = loadfile_command(&trailer).unwrap();
    assert_eq!(line, "loadfile vod from rr1.example (audio file)");
    let (_, line) = loadfile_command(&Cmd::LoadVod(load(
        "http://h:80/series/u/p/7.mp4",
        0.0,
        None,
    )))
    .unwrap();
    assert_eq!(line, "loadfile vod from h");
    assert!(loadfile_command(&Cmd::SetMute(true)).is_none());
}

#[test]
fn mpvs_track_list_gives_the_audio_and_subtitle_tracks() {
    // As the vendored libmpv gives `track-list` as a string (see the smoke test).
    let json = r#"[
        {"id":1,"type":"video","src-id":0,"selected":true,"codec":"h264"},
        {"id":1,"type":"audio","lang":"nld","title":"Nederlands","selected":true,"default":true},
        {"id":2,"type":"audio","lang":"eng","selected":false},
        {"id":1,"type":"sub","lang":"nld","title":"Nederlands (SDH)","selected":false},
        {"type":"audio"},
        "odd"
    ]"#;
    let track = |id, kind, lang: &str, title: &str, selected| Track {
        id,
        kind,
        lang: lang.into(),
        title: title.into(),
        selected,
    };
    assert_eq!(
        parse_tracks(json),
        [
            track(1, TrackKind::Audio, "nld", "Nederlands", true),
            track(2, TrackKind::Audio, "eng", "", false),
            track(1, TrackKind::Subtitle, "nld", "Nederlands (SDH)", false),
        ]
    );
    assert!(parse_tracks("Video: (1) h264").is_empty());
    assert!(parse_tracks("").is_empty());
}

#[test]
fn a_position_is_news_once_a_second_and_after_a_seek() {
    let mut gate = SecondGate::default();
    assert!(gate.pass(0.04));
    assert!(!gate.pass(0.5));
    assert!(gate.pass(1.0));
    assert!(!gate.pass(1.96));
    assert!(gate.pass(600.2), "a seek");
    assert!(gate.pass(12.0), "and back");
    assert!(!gate.pass(f64::NAN));
    gate.reset();
    assert!(gate.pass(12.3), "a new file");
}
