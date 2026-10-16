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

/// What `vod_report` reads of mpv's events (built here by hand, so no libmpv),
/// and the edges of `parse_tracks` and `SecondGate`.
mod review_t10 {
    use bongo_player::player::events::{parse_tracks, vod_report};
    use bongo_player::player::mpv_sys::{
        mpv_event, mpv_event_property, MPV_EVENT_PROPERTY_CHANGE, MPV_FORMAT_DOUBLE,
        MPV_FORMAT_INT64, MPV_FORMAT_NONE, MPV_FORMAT_STRING,
    };
    use bongo_player::player::{SecondGate, Track, TrackKind, VodEvent, OBSERVE_VOD};
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    fn event_of(prop: &mut mpv_event_property) -> mpv_event {
        mpv_event {
            event_id: MPV_EVENT_PROPERTY_CHANGE,
            error: 0,
            reply_userdata: OBSERVE_VOD,
            data: prop as *mut mpv_event_property as *mut c_void,
        }
    }

    fn report_double(name: &[u8], format: i32, value: f64) -> Option<VodEvent> {
        let name = CString::new(name.to_vec()).unwrap();
        let mut v = value;
        let mut prop = mpv_event_property {
            name: name.as_ptr(),
            format,
            data: &mut v as *mut f64 as *mut c_void,
        };
        let mut evt = event_of(&mut prop);
        unsafe { vod_report(&mut evt) }
    }

    fn report_string(name: &str, value: Option<&str>) -> Option<VodEvent> {
        let name = CString::new(name).unwrap();
        let s = value.map(|v| CString::new(v).unwrap());
        let mut p: *const c_char = s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let mut prop = mpv_event_property {
            name: name.as_ptr(),
            format: MPV_FORMAT_STRING,
            data: &mut p as *mut *const c_char as *mut c_void,
        };
        let mut evt = event_of(&mut prop);
        unsafe { vod_report(&mut evt) }
    }

    #[test]
    fn vod_report_reads_the_three_properties_and_nothing_else() {
        assert_eq!(
            report_double(b"time-pos", MPV_FORMAT_DOUBLE, 12.5),
            Some(VodEvent::Position(12.5))
        );
        assert_eq!(
            report_double(b"duration", MPV_FORMAT_DOUBLE, 5400.0),
            Some(VodEvent::Duration(5400.0))
        );
        let json = r#"[{"id":1,"type":"audio","lang":"nld","selected":true}]"#;
        assert_eq!(
            report_string("track-list", Some(json)),
            Some(VodEvent::Tracks(vec![Track {
                id: 1,
                kind: TrackKind::Audio,
                lang: "nld".into(),
                title: String::new(),
                selected: true,
            }]))
        );
        // Another format for a known name, an unknown name, a name that is no
        // UTF-8.
        assert_eq!(report_double(b"time-pos", MPV_FORMAT_INT64, 1.0), None);
        assert_eq!(report_string("time-pos", Some("12")), None);
        assert_eq!(report_string("duration", Some("[]")), None);
        assert_eq!(report_double(b"volume", MPV_FORMAT_DOUBLE, 50.0), None);
        assert_eq!(report_double(&[0xff, 0xfe], MPV_FORMAT_DOUBLE, 1.0), None);
        // track-list without its string; a string that is no JSON.
        assert_eq!(report_string("track-list", None), None);
        assert_eq!(
            report_string("track-list", Some("Video: (1) h264")),
            Some(VodEvent::Tracks(vec![]))
        );
    }

    #[test]
    fn vod_report_never_reads_the_data_of_an_unavailable_property() {
        // client.h (mpv_observe_property): with MPV_FORMAT_NONE "the
        // mpv_event_property.data field is invalid". It is never read.
        for name in ["time-pos", "duration", "track-list"] {
            let n = CString::new(name).unwrap();
            let mut prop = mpv_event_property {
                name: n.as_ptr(),
                format: MPV_FORMAT_NONE,
                data: 0x10 as *mut c_void,
            };
            let mut evt = event_of(&mut prop);
            assert_eq!(unsafe { vod_report(&mut evt) }, None, "{name}");
            let mut prop = mpv_event_property {
                name: n.as_ptr(),
                format: MPV_FORMAT_DOUBLE,
                data: std::ptr::null_mut(),
            };
            let mut evt = event_of(&mut prop);
            assert_eq!(unsafe { vod_report(&mut evt) }, None, "{name}: null data");
        }
        let mut evt = mpv_event {
            event_id: MPV_EVENT_PROPERTY_CHANGE,
            error: 0,
            reply_userdata: OBSERVE_VOD,
            data: std::ptr::null_mut(),
        };
        assert_eq!(unsafe { vod_report(&mut evt) }, None, "null event data");
    }

    #[test]
    fn odd_doubles_pass_vod_report_and_the_gate_keeps_non_finite_ones_out() {
        for v in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            -0.0,
            1e300,
            -1e300,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            5e-324,
        ] {
            let p = report_double(b"time-pos", MPV_FORMAT_DOUBLE, v);
            assert!(
                matches!(p, Some(VodEvent::Position(x)) if x.to_bits() == v.to_bits()),
                "{v}: {p:?}"
            );
            let mut gate = SecondGate::default();
            assert_eq!(gate.pass(v), v.is_finite(), "{v}");
        }
    }

    #[test]
    fn the_gate_on_infinite_negative_and_huge_values() {
        let mut gate = SecondGate::default();
        assert!(!gate.pass(f64::INFINITY), "+inf is no news");
        assert!(!gate.pass(f64::NEG_INFINITY), "-inf is no news");
        assert!(
            gate.pass(-0.04),
            "a position just before 0 is news (floor -1)"
        );
        assert!(!gate.pass(-0.5), "the same whole second");
        assert!(gate.pass(0.0), "then 0");
        assert!(gate.pass(1e300));
        // The cast saturates: two huge values are "the same second". Harmless.
        assert!(!gate.pass(2e300));
    }

    #[test]
    fn a_selected_flag_that_is_missing_or_no_bool_is_false() {
        let json = r#"[
            {"id":1,"type":"audio"},
            {"id":2,"type":"audio","selected":"yes"},
            {"id":3,"type":"sub","selected":1},
            {"id":4,"type":"sub","selected":null}
        ]"#;
        let tracks = parse_tracks(json);
        assert_eq!(tracks.len(), 4);
        assert!(tracks.iter().all(|t| !t.selected), "{tracks:?}");
    }

    #[test]
    fn ids_that_are_no_i64_leave_the_track_out() {
        let json = r#"[
            {"id":1.0,"type":"audio"},
            {"id":"2","type":"audio"},
            {"id":18446744073709551615,"type":"audio"},
            {"id":-3,"type":"audio","lang":5,"title":["x"]},
            {"id":9223372036854775807,"type":"sub"}
        ]"#;
        let tracks = parse_tracks(json);
        assert_eq!(
            tracks
                .iter()
                .map(|t| (t.id, t.lang.as_str(), t.title.as_str()))
                .collect::<Vec<_>>(),
            [(-3, "", ""), (i64::MAX, "", "")]
        );
    }

    #[test]
    fn deep_huge_and_broken_track_lists_never_panic() {
        assert!(parse_tracks(&"[".repeat(100_000)).is_empty());
        let deep_obj = format!("[{}]", "{\"a\":".repeat(50_000));
        assert!(parse_tracks(&deep_obj).is_empty());
        let many = format!(
            "[{}]",
            vec![r#"{"id":1,"type":"audio"}"#; 100_000].join(",")
        );
        assert_eq!(parse_tracks(&many).len(), 100_000);
        // A bare non-finite double loses the whole list, quietly.
        assert!(parse_tracks(r#"[{"id":1,"type":"audio","demux-fps":nan}]"#).is_empty());
        let _ = parse_tracks(r#"[{"id":1,"id":2,"type":"audio","type":"sub"}]"#);
        assert!(parse_tracks("{}").is_empty());
        assert!(parse_tracks("null").is_empty());
    }
}

/// parse_tracks against an oracle written apart from it (proptest).
mod review_t10_oracle {
    use bongo_player::player::events::parse_tracks;
    use bongo_player::player::TrackKind;
    use proptest::prelude::*;
    use serde_json::Value;

    fn oracle(v: &Value) -> Vec<(i64, bool, String, String, bool)> {
        let Value::Array(list) = v else {
            return vec![];
        };
        let mut out = vec![];
        for t in list {
            let Value::Object(o) = t else { continue };
            let audio = match o.get("type") {
                Some(Value::String(s)) if s == "audio" => true,
                Some(Value::String(s)) if s == "sub" => false,
                _ => continue,
            };
            let Some(id) = o.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let text = |k: &str| match o.get(k) {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let selected = matches!(o.get("selected"), Some(Value::Bool(true)));
            out.push((id, audio, text("lang"), text("title"), selected));
        }
        out
    }

    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| serde_json::json!(n)),
            any::<u64>().prop_map(|n| serde_json::json!(n)),
            (-1e6f64..1e6).prop_map(|f| serde_json::json!(f)),
            ".{0,6}".prop_map(Value::String),
            prop_oneof![Just("audio"), Just("sub"), Just("video"), Just("Audio")]
                .prop_map(|s| Value::String(s.into())),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            let key = prop_oneof![
                Just("id".to_string()),
                Just("type".to_string()),
                Just("lang".to_string()),
                Just("title".to_string()),
                Just("selected".to_string()),
                ".{0,4}",
            ];
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                prop::collection::btree_map(key, inner, 0..7)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 4000, ..ProptestConfig::default() })]

        #[test]
        fn parse_tracks_never_panics_on_any_text(s in ".{0,200}") {
            let _ = parse_tracks(&s);
        }

        #[test]
        fn parse_tracks_matches_the_oracle(v in json_value()) {
            let got: Vec<_> = parse_tracks(&v.to_string())
                .into_iter()
                .map(|t| (t.id, t.kind == TrackKind::Audio, t.lang, t.title, t.selected))
                .collect();
            prop_assert_eq!(got, oracle(&v));
        }
    }
}
