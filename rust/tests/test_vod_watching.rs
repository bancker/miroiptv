//! What was watched (vod/watching.rs, spec 2026-10-15 §4.5): the file's
//! format, the 60 s and 90 % rules, Mijn lijst and the thumbs, the writes 5 s
//! apart and at a stop, and a file that cannot be read.

use bongo_player::vod::watching::{
    bad_copies, bad_copy, EpisodeOf, Item, Progress, Thumb, Watching, KEPT_COPIES, WRITE_EVERY,
};
use bongo_player::vod::{Kind, TitleId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NOW: i64 = 1_792_089_306;

fn fresh() -> (tempfile::TempDir, PathBuf, Watching) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let watching = Watching::load(path.clone());
    (dir, path, watching)
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn series(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

/// A log sink the test can read back.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What `f` logs at warn level on this thread, with what it returns. The second
/// dispatcher keeps another test's thread, which has no subscriber, from
/// silencing a log line for this one: tracing works a line's interest out from
/// whoever hits it first, unless more than one dispatcher is registered.
fn logged<T>(f: impl FnOnce() -> T) -> (T, String) {
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let buffer = LogBuffer::default();
    let sink = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || sink.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    (result, text)
}

/// The log holds one line, and that line names `watching.json` and not the
/// folder it is in: the folder holds the Windows user's name.
fn assert_one_line_naming_only_the_file(log: &str, folder: &Path) {
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("vod: watching.json "), "{log}");
    let folder = folder.to_string_lossy();
    assert!(!log.contains(&*folder), "the folder is in the line: {log}");
}

#[test]
fn the_file_is_in_the_specs_format() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(12345), 4380.4, 9960.0, None, NOW);
    let episode = EpisodeOf {
        series: 4321,
        season: 2,
        number: 5,
    };
    w.played(
        Item::Episode(987654),
        1415.0,
        2820.0,
        Some(episode),
        NOW + 94,
    );
    w.toggle_list(series(4321), NOW - 306);
    w.press_thumb(series(4321), Thumb::Up);
    w.press_thumb(film(555), Thumb::Down);
    w.flush();
    let expected = serde_json::json!({
        "version": 1,
        "items": {
            "m:12345": { "pos": 4380, "dur": 9960, "at": NOW, "done": false },
            "e:987654": { "pos": 1415, "dur": 2820, "at": NOW + 94, "done": false, "series": 4321, "s": 2, "n": 5 }
        },
        "list": [ { "k": "s", "id": 4321, "at": NOW - 306 } ],
        "thumbs": { "s:4321": 1, "m:555": -1 }
    });
    assert_eq!(json(&path), expected);
    let again = Watching::load(path);
    assert_eq!(
        again.progress(Item::Episode(987654)),
        Some(&Progress {
            pos: 1415,
            dur: 2820,
            at: NOW + 94,
            done: false,
            episode: Some(episode)
        })
    );
    assert!(again.in_list(series(4321)));
    assert_eq!(again.thumb(film(555)), Some(Thumb::Down));
}

#[test]
fn under_a_minute_is_not_started() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 59.9, 6000.0, None, NOW);
    assert_eq!(w.progress(Item::Film(1)), None);
    assert!(!w.is_dirty());
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    assert!(w.progress(Item::Film(1)).is_some());
    // Back to the start: not started again.
    w.played(Item::Film(1), 12.0, 6000.0, None, NOW + 1);
    assert_eq!(w.progress(Item::Film(1)), None);
}

#[test]
fn from_90_percent_or_at_the_end_it_is_seen() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 5399.0, 6000.0, None, NOW);
    assert!(!w.progress(Item::Film(1)).unwrap().done);
    w.played(Item::Film(1), 5400.0, 6000.0, None, NOW);
    assert!(w.progress(Item::Film(1)).unwrap().done, "90 %");
    w.played(Item::Film(2), 300.0, 0.0, None, NOW);
    assert!(
        !w.progress(Item::Film(2)).unwrap().done,
        "no length: not by share"
    );
    w.finished(Item::Film(2), 0.0, None, NOW);
    let end = w.progress(Item::Film(2)).unwrap();
    assert!(end.done, "the end of the file");
    assert_eq!(end.pos, 300);
}

#[test]
fn a_seen_item_stays_seen_under_a_minute_and_a_rewatch_is_in_progress_again() {
    let (_dir, _path, mut w) = fresh();
    w.finished(Item::Episode(7), 2820.0, None, NOW);
    w.played(Item::Episode(7), 30.0, 2820.0, None, NOW + 10);
    assert!(
        w.progress(Item::Episode(7)).unwrap().done,
        "a peek at the start"
    );
    w.played(Item::Episode(7), 600.0, 2820.0, None, NOW + 600);
    let p = w.progress(Item::Episode(7)).unwrap();
    assert!(!p.done && p.pos == 600, "watched again: {p:?}");
}

#[test]
fn the_same_place_again_is_no_change() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.2, 6000.0, None, NOW);
    w.flush();
    assert!(!w.is_dirty());
    w.played(Item::Film(1), 600.9, 6000.0, None, NOW + 5);
    assert!(!w.is_dirty(), "paused: the same second");
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 6);
    assert!(w.is_dirty());
}

#[test]
fn writes_are_five_seconds_apart_and_a_stop_writes_at_once() {
    let (_dir, path, mut w) = fresh();
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert_eq!(
        json(&path)["items"]["m:1"]["pos"],
        600,
        "the first write goes at once"
    );
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 1);
    w.save_if_due(t0 + Duration::from_secs(1));
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 600, "not yet");
    w.save_if_due(t0 + WRITE_EVERY);
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 601);
    w.played(Item::Film(1), 602.0, 6000.0, None, NOW + 2);
    w.flush();
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 602, "a stop");
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn mijn_lijst_has_the_latest_first_and_a_second_press_takes_a_title_off() {
    let (_dir, _path, mut w) = fresh();
    assert!(w.toggle_list(series(1), NOW));
    assert!(w.toggle_list(film(2), NOW + 10));
    assert!(w.toggle_list(series(3), NOW + 5));
    assert_eq!(w.list(), [film(2), series(3), series(1)]);
    assert!(!w.toggle_list(film(2), NOW + 20));
    assert!(!w.in_list(film(2)));
    assert_eq!(w.list(), [series(3), series(1)]);
}

#[test]
fn thumbs_up_and_down_exclude_each_other_and_a_second_press_withdraws() {
    let (_dir, _path, mut w) = fresh();
    assert_eq!(w.press_thumb(film(1), Thumb::Up), Some(Thumb::Up));
    assert_eq!(
        w.press_thumb(film(1), Thumb::Down),
        Some(Thumb::Down),
        "👎 takes 👍's place"
    );
    assert_eq!(w.thumb(film(1)), Some(Thumb::Down));
    assert_eq!(
        w.press_thumb(film(1), Thumb::Down),
        None,
        "pressed again: withdrawn"
    );
    assert_eq!(w.thumb(film(1)), None);
}

#[test]
fn a_film_title_shows_the_latest_progress_of_its_versions() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.played(Item::Film(2), 1200.0, 6000.0, None, NOW + 60);
    let (item, p) = w
        .latest([Item::Film(1), Item::Film(2), Item::Film(3)])
        .unwrap();
    assert_eq!((item, p.pos), (Item::Film(2), 1200));
    assert!(w.latest([Item::Film(3)]).is_none());
}

#[test]
fn a_missing_file_is_an_empty_store() {
    let (_dir, path, w) = fresh();
    assert!(!path.exists());
    assert!(w.list().is_empty() && !w.is_dirty());
}

#[test]
fn an_unreadable_file_is_kept_as_bad_and_the_store_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    for (n, body) in [
        &b"{\"version\":1,\"items\":"[..],
        br#"{"version":2,"items":{}}"#,
    ]
    .into_iter()
    .enumerate()
    {
        std::fs::write(&path, body).unwrap();
        let mut w = Watching::load(path.clone());
        assert!(w.progress(Item::Film(1)).is_none());
        // The second goes beside the first, not over it.
        assert_eq!(
            std::fs::read(bad_copy(&path, n as u32 + 1)).unwrap(),
            body,
            "kept"
        );
        assert!(!path.exists(), "moved aside");
        w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
        w.flush();
        assert_eq!(json(&path)["version"], 1, "the next write starts afresh");
    }
}

#[test]
fn entries_the_store_cannot_read_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let body = r#"{"version":1,
        "items":{"m:1":{"pos":600,"dur":6000,"at":1,"done":false},"x:2":{"pos":1,"dur":1,"at":1,"done":false}},
        "list":[{"k":"q","id":3,"at":1},{"k":"m","id":4,"at":1}],
        "thumbs":{"s:5":1,"s:6":7,"bad":1}}"#;
    std::fs::write(&path, body).unwrap();
    let w = Watching::load(path);
    assert!(w.progress(Item::Film(1)).is_some());
    assert_eq!(w.list(), [film(4)]);
    assert_eq!(w.thumb(series(5)), Some(Thumb::Up));
    assert_eq!(w.thumb(series(6)), None);
}

#[test]
fn a_write_that_fails_keeps_the_old_file_and_tries_again_later() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    std::fs::create_dir(&path).unwrap(); // a folder where the file goes
    let mut w = Watching::load(path.clone());
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert!(w.is_dirty(), "not written");
    assert!(!path.with_extension("json.tmp").exists());
    std::fs::remove_dir(&path).unwrap();
    w.save_if_due(t0 + WRITE_EVERY);
    assert!(!w.is_dirty());
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 600);
}

#[test]
fn the_file_holds_numbers_and_times_only() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.toggle_list(film(1), NOW);
    w.flush();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("http") && !text.contains("name"), "{text}");
}

// Beyond the plan's list: the edge of the first rule, and the log lines
// (Ruling 35: "is logged once until a write works"; the folder holds the
// Windows user's name, so a line names the file only).

#[test]
fn a_minute_exactly_is_started_and_takes_a_seen_item_back_in_progress() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 60.0, 6000.0, None, NOW);
    assert_eq!(w.progress(Item::Film(1)).map(|p| p.pos), Some(60));
    // The other edge of the same rule: a seen item at 60 s, under 90 %, is in
    // progress again.
    w.finished(Item::Film(2), 6000.0, None, NOW);
    w.played(Item::Film(2), 60.0, 6000.0, None, NOW + 1);
    let p = w.progress(Item::Film(2)).unwrap();
    assert!(!p.done && p.pos == 60, "{p:?}");
}

#[test]
fn a_file_that_cannot_be_read_is_logged_with_its_name_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    // A folder where the file goes: reading it fails with an error that is not
    // "not found".
    std::fs::create_dir(&path).unwrap();
    let (w, log) = logged(|| Watching::load(path.clone()));
    assert!(w.list().is_empty() && !w.is_dirty());
    assert_one_line_naming_only_the_file(&log, dir.path());
    assert!(log.contains("vod: watching.json unreadable ("), "{log}");
    assert!(log.trim_end().ends_with(") - starting empty"), "{log}");
    assert!(path.is_dir(), "nothing moved aside");
    // A file that is not there is no trouble: nothing is logged.
    let (_w, log) = logged(|| Watching::load(dir.path().join("missing.json")));
    assert_eq!(log, "");
}

#[test]
fn an_unreadable_file_is_logged_once_with_its_name_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    for (body, why) in [
        (&b"{\"version\":1,\"items\":"[..], "unreadable ("),
        (
            &br#"{"version":2,"items":{}}"#[..],
            "unreadable (version 2)",
        ),
    ] {
        std::fs::write(&path, body).unwrap();
        let (_w, log) = logged(|| Watching::load(path.clone()));
        assert_one_line_naming_only_the_file(&log, dir.path());
        assert!(log.contains(&format!("vod: watching.json {why}")), "{log}");
        assert!(
            log.trim_end().ends_with(" - kept as .bad - starting empty"),
            "{log}"
        );
    }
}

#[test]
fn an_unreadable_file_that_cannot_be_moved_aside_is_left_as_it_is_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    // A folder where every copy goes: the rename onto the last one fails.
    for copy in bad_copies(&path) {
        std::fs::create_dir(copy).unwrap();
    }
    std::fs::write(&path, b"{\"version\":1,\"items\":").unwrap();
    let (w, log) = logged(|| Watching::load(path.clone()));
    assert!(w.list().is_empty() && !w.is_dirty());
    assert_one_line_naming_only_the_file(&log, dir.path());
    assert!(
        log.trim_end()
            .ends_with(" - left as it is - starting empty"),
        "{log}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"version\":1,\"items\":");
}

#[test]
fn a_failed_write_is_logged_with_the_files_name_only_and_once_until_a_write_works() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let mut w = Watching::load(path.clone());
    // A folder where the file goes: the temporary file is written, and the
    // rename onto the folder fails.
    std::fs::create_dir(&path).unwrap();
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    let ((), log) = logged(|| w.save_if_due(t0));
    assert_one_line_naming_only_the_file(&log, dir.path());
    assert!(log.contains("vod: watching.json not written ("), "{log}");
    assert!(log.trim_end().ends_with(')'), "{log}");
    // The next try fails too: nothing more is said.
    let ((), log) = logged(|| w.save_if_due(t0 + WRITE_EVERY));
    assert_eq!(log, "", "said once");
    assert!(w.is_dirty());
    // A write that works ...
    std::fs::remove_dir(&path).unwrap();
    w.save_if_due(t0 + 2 * WRITE_EVERY);
    assert!(!w.is_dirty());
    // ... and one that fails after it is said again.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 1);
    let ((), log) = logged(|| w.save_if_due(t0 + 3 * WRITE_EVERY));
    assert_one_line_naming_only_the_file(&log, dir.path());
    assert!(log.contains("vod: watching.json not written ("), "{log}");
    // A stop that fails says nothing more either.
    let ((), log) = logged(|| w.flush());
    assert_eq!(log, "", "said once");
}

#[test]
fn a_stop_with_nothing_changed_writes_nothing() {
    let (_dir, path, mut w) = fresh();
    w.flush();
    assert!(!path.exists(), "nothing ever changed: no file");
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.flush();
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();
    w.flush();
    assert!(!path.exists(), "nothing changed since the last write");
}

// From the review of Task 7. An entry that cannot be read costs that entry
// only (Ruling 35), a copy that was kept is never replaced, a byte order mark
// is no trouble, and what the rules and the writes promise but no test held.

/// A file with one good entry of each kind and the given extra ones, each
/// starting with a comma.
fn with_good_entries(items: &str, list: &str, thumbs: &str) -> String {
    format!(
        r#"{{"version":1,
        "items":{{"m:1":{{"pos":600,"dur":6000,"at":1,"done":false}}{items}}},
        "list":[{{"k":"s","id":3,"at":1}}{list}],
        "thumbs":{{"s:5":1{thumbs}}}}}"#
    )
}

/// `body` as the file, loaded with what it logged.
fn load_logged(body: &str) -> (tempfile::TempDir, PathBuf, Watching, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    std::fs::write(&path, body).unwrap();
    let (w, log) = logged(|| Watching::load(path.clone()));
    (dir, path, w, log)
}

#[test]
fn an_entry_that_cannot_be_read_costs_that_entry_only() {
    let items = [
        r#","m:2":{"pos":-1,"dur":6000,"at":1,"done":false}"#,
        r#","m:2":{"pos":1.5,"dur":6000,"at":1,"done":false}"#,
        r#","m:2":{"pos":"600","dur":6000,"at":1,"done":false}"#,
        r#","m:2":{"pos":4294967296,"dur":6000,"at":1,"done":false}"#,
        r#","m:2":{"pos":600,"dur":6000,"at":1,"done":1}"#,
        r#","m:2":{"pos":600,"dur":6000,"done":false}"#,
        r#","m:2":null"#,
        r#","e:2":{"pos":600,"dur":6000,"at":1,"done":false,"series":4,"s":-1,"n":1}"#,
    ];
    let lists = [
        r#",{"k":"s","id":4}"#,
        r#",{"k":"s","id":"4","at":1}"#,
        r#",{"k":5,"id":4,"at":1}"#,
        r#","s:4""#,
        ",null",
    ];
    let thumbs = [
        r#","s:2":300"#,
        r#","s:2":1.0"#,
        r#","s:2":"up""#,
        r#","s:2":null"#,
    ];
    let bodies = items
        .iter()
        .map(|x| with_good_entries(x, "", ""))
        .chain(lists.iter().map(|x| with_good_entries("", x, "")))
        .chain(thumbs.iter().map(|x| with_good_entries("", "", x)));
    for body in bodies {
        let (dir, path, w, log) = load_logged(&body);
        assert!(w.progress(Item::Film(1)).is_some(), "{body}");
        assert!(w.progress(Item::Film(2)).is_none(), "{body}");
        assert!(w.progress(Item::Episode(2)).is_none(), "{body}");
        assert_eq!(w.list(), [series(3)], "{body}");
        assert_eq!(w.thumb(series(5)), Some(Thumb::Up), "{body}");
        assert_eq!(w.thumb(series(2)), None, "{body}");
        assert!(!bad_copy(&path, 1).exists(), "the file is no .bad: {body}");
        assert!(path.exists(), "the file stays where it is: {body}");
        assert_one_line_naming_only_the_file(&log, dir.path());
        assert!(
            log.contains("vod: watching.json left out entries it cannot read (1)"),
            "{log}"
        );
    }
}

#[test]
fn the_entries_left_out_are_counted_in_one_line() {
    // A null item, an unknown kind of item, a kind of list entry, a thumb of
    // 7 and a thumb of a key that names no title.
    let body = with_good_entries(
        r#","m:2":null,"x:9":{"pos":1,"dur":1,"at":1,"done":false}"#,
        r#",{"k":"q","id":1,"at":1}"#,
        r#","s:2":7,"nokey":1"#,
    );
    let (dir, _path, w, log) = load_logged(&body);
    assert!(w.progress(Item::Film(1)).is_some());
    assert_one_line_naming_only_the_file(&log, dir.path());
    assert!(log.contains("it cannot read (5)"), "{log}");
    // A file that is read whole says nothing.
    let (_dir, _path, _w, log) = load_logged(&with_good_entries("", "", ""));
    assert_eq!(log, "");
}

#[test]
fn keys_the_store_does_not_know_are_left_out_and_not_guessed() {
    let body = r#"{"version":1,
        "items":{"x:2":{"pos":600,"dur":6000,"at":1,"done":false},"m:abc":{"pos":600,"dur":6000,"at":1,"done":false}},
        "thumbs":{"x:5":1,"s:abc":1}}"#;
    let (_dir, _path, w, _log) = load_logged(body);
    for id in [0, 2] {
        assert!(w.progress(Item::Film(id)).is_none(), "film {id}");
        assert!(w.progress(Item::Episode(id)).is_none(), "episode {id}");
    }
    for t in [film(5), series(5), film(0), series(0)] {
        assert_eq!(w.thumb(t), None, "{t:?}");
    }
}

#[test]
fn an_episode_without_a_season_or_number_reads_them_as_0() {
    let body =
        r#"{"version":1,"items":{"e:9":{"pos":600,"dur":6000,"at":1,"done":false,"series":4}}}"#;
    let (_dir, _path, w, _log) = load_logged(body);
    let episode = w.progress(Item::Episode(9)).and_then(|p| p.episode);
    assert_eq!(
        episode,
        Some(EpisodeOf {
            series: 4,
            season: 0,
            number: 0
        })
    );
}

#[test]
fn a_file_whose_frame_is_wrong_is_kept_as_bad() {
    for body in [
        "",
        "[]",
        "null",
        "7",
        r#"{"items":{}}"#,
        r#"{"version":"1","items":{}}"#,
        r#"{"version":1.0}"#,
        r#"{"version":0,"items":{}}"#,
        r#"{"version":1,"items":[]}"#,
        r#"{"version":1,"items":null}"#,
        r#"{"version":1,"list":{}}"#,
        r#"{"version":1,"thumbs":[]}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watching.json");
        std::fs::write(&path, body).unwrap();
        let w = Watching::load(path.clone());
        assert!(w.list().is_empty() && w.progress(Item::Film(1)).is_none());
        assert_eq!(std::fs::read(bad_copy(&path, 1)).unwrap(), body.as_bytes());
        assert!(!path.exists(), "moved aside: {body}");
    }
}

#[test]
fn a_file_may_leave_a_collection_out() {
    for body in [
        r#"{"version":1}"#,
        r#"{"version":1,"items":{}}"#,
        r#"{"version":1,"list":[]}"#,
        r#"{"version":1,"thumbs":{"s:5":1}}"#,
    ] {
        let (_dir, path, w, log) = load_logged(body);
        assert!(!bad_copy(&path, 1).exists(), "{body}");
        assert_eq!(log, "", "{body}");
        assert_eq!(w.thumb(series(5)).is_some(), body.contains("s:5"), "{body}");
    }
}

#[test]
fn a_title_twice_in_the_list_is_in_it_once() {
    let body = r#"{"version":1,"list":[
        {"k":"s","id":4,"at":5},{"k":"s","id":4,"at":5},{"k":"m","id":4,"at":7}]}"#;
    let (_dir, _path, w, log) = load_logged(body);
    assert_eq!(w.list(), [film(4), series(4)]);
    assert_eq!(log, "");
}

#[test]
fn of_a_title_twice_in_the_list_the_first_entry_stays_in_the_order_of_the_file() {
    // The first entry of a title says when it was added, and a later one is
    // dropped whatever its time. Entries of one time keep the order of the file.
    let body = r#"{"version":1,"list":[
        {"k":"s","id":9,"at":5},{"k":"s","id":2,"at":5},{"k":"m","id":9,"at":5},
        {"k":"s","id":9,"at":99},{"k":"m","id":2,"at":5},{"k":"s","id":2,"at":1},
        {"k":"s","id":1,"at":5}]}"#;
    let (_dir, _path, w, log) = load_logged(body);
    assert_eq!(
        w.list(),
        [series(9), series(2), film(9), film(2), series(1)]
    );
    assert_eq!(log, "");
}

#[test]
fn a_byte_order_mark_is_no_trouble() {
    // Notepad's "UTF-8 with BOM", as settings.json and the lists allow for.
    let body = format!("\u{feff}{}", with_good_entries("", "", ""));
    let (_dir, path, w, log) = load_logged(&body);
    assert!(w.progress(Item::Film(1)).is_some());
    assert_eq!(w.list(), [series(3)]);
    assert_eq!(log, "");
    assert!(!bad_copy(&path, 1).exists());
}

#[test]
fn a_second_unreadable_file_does_not_take_the_first_copys_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    for n in 1..=3 {
        std::fs::write(&path, format!("garbage {n}")).unwrap();
        let w = Watching::load(path.clone());
        assert!(w.list().is_empty());
        assert!(!path.exists(), "moved aside");
    }
    for n in 1..=3 {
        let copy = std::fs::read(bad_copy(&path, n)).unwrap();
        assert_eq!(copy, format!("garbage {n}").as_bytes(), "copy {n}");
    }
    assert_eq!(bad_copy(&path, 1), dir.path().join("watching.json.bad"));
    assert_eq!(bad_copy(&path, 2), dir.path().join("watching.json.2.bad"));
    assert_eq!(bad_copies(&path).count(), KEPT_COPIES as usize);
}

#[test]
fn a_copy_that_is_a_folder_is_stepped_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    std::fs::create_dir(bad_copy(&path, 1)).unwrap();
    std::fs::write(&path, "garbage").unwrap();
    let (_w, log) = logged(|| Watching::load(path.clone()));
    assert_eq!(std::fs::read(bad_copy(&path, 2)).unwrap(), b"garbage");
    assert!(!path.exists());
    assert!(log.contains(" - kept as .bad - starting empty"), "{log}");
}

#[test]
fn past_the_ninth_copy_the_last_is_replaced_and_no_tenth_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    for n in 1..=KEPT_COPIES + 1 {
        std::fs::write(&path, format!("garbage {n}")).unwrap();
        let _ = Watching::load(path.clone());
    }
    let read = |n| String::from_utf8(std::fs::read(bad_copy(&path, n)).unwrap()).unwrap();
    assert_eq!(read(1), "garbage 1", "the first stays");
    assert_eq!(
        read(KEPT_COPIES - 1),
        format!("garbage {}", KEPT_COPIES - 1)
    );
    assert_eq!(read(KEPT_COPIES), format!("garbage {}", KEPT_COPIES + 1));
    let kept = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(kept, KEPT_COPIES as usize, "nothing else is made");
}

#[test]
fn a_position_that_is_not_a_time_changes_nothing() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.flush();
    for pos in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, -0.5] {
        w.played(Item::Film(1), pos, 6000.0, None, NOW + 1);
        w.played(Item::Film(2), pos, 6000.0, None, NOW + 1);
    }
    assert!(!w.is_dirty());
    let kept = w.progress(Item::Film(1)).map(|p| p.pos);
    assert_eq!(kept, Some(600), "where it was stays");
    assert!(w.progress(Item::Film(2)).is_none());
}

#[test]
fn a_length_that_is_not_a_time_is_no_length() {
    let odd = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 0.0];
    for dur in odd {
        let (_dir, _path, mut w) = fresh();
        w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
        w.played(Item::Film(1), 601.0, dur, None, NOW + 1);
        let p = w.progress(Item::Film(1)).unwrap();
        assert_eq!((p.pos, p.dur, p.done), (601, 6000, false), "played, {dur}");
        w.finished(Item::Film(1), dur, None, NOW + 2);
        let p = w.progress(Item::Film(1)).unwrap();
        assert_eq!(
            (p.pos, p.dur, p.done),
            (6000, 6000, true),
            "finished, {dur}"
        );
    }
}

#[test]
fn a_length_that_becomes_known_is_a_change() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 0.0, None, NOW);
    w.flush();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW + 1);
    assert!(w.is_dirty());
    assert_eq!(w.progress(Item::Film(1)).map(|p| p.dur), Some(6000));
}

#[test]
fn an_episode_keeps_its_place_in_its_series_when_a_report_names_none() {
    let (_dir, _path, mut w) = fresh();
    let episode = EpisodeOf {
        series: 4321,
        season: 2,
        number: 5,
    };
    w.played(Item::Episode(7), 600.0, 2820.0, None, NOW);
    w.flush();
    w.played(Item::Episode(7), 600.0, 2820.0, Some(episode), NOW + 1);
    assert!(w.is_dirty(), "naming the episode is a change");
    w.played(Item::Episode(7), 700.0, 2820.0, None, NOW + 2);
    assert_eq!(w.progress(Item::Episode(7)).unwrap().episode, Some(episode));
    w.finished(Item::Episode(7), 2820.0, None, NOW + 3);
    assert_eq!(w.progress(Item::Episode(7)).unwrap().episode, Some(episode));
}

#[test]
fn an_item_taken_back_under_a_minute_is_taken_out_of_the_file_too() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.flush();
    assert!(json(&path)["items"]["m:1"].is_object());
    w.played(Item::Film(1), 12.0, 6000.0, None, NOW + 1);
    assert!(w.is_dirty());
    w.flush();
    assert!(json(&path)["items"].get("m:1").is_none());
}

#[test]
fn the_end_of_a_file_is_written_and_stamped() {
    let (_dir, path, mut w) = fresh();
    let episode = EpisodeOf {
        series: 4321,
        season: 2,
        number: 5,
    };
    w.played(Item::Episode(7), 600.0, 2820.0, Some(episode), NOW);
    w.flush();
    w.finished(Item::Episode(7), 2820.0, None, NOW + 3000);
    assert!(w.is_dirty(), "a seen mark is a change");
    w.flush();
    let again = Watching::load(path);
    assert_eq!(
        again.progress(Item::Episode(7)),
        Some(&Progress {
            pos: 2820,
            dur: 2820,
            at: NOW + 3000,
            done: true,
            episode: Some(episode)
        })
    );
}

#[test]
fn what_was_written_is_read_back_whole() {
    let (_dir, path, mut w) = fresh();
    w.finished(Item::Film(2), 6000.0, None, NOW);
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW + 1);
    w.toggle_list(film(10), NOW + 10);
    w.toggle_list(series(11), NOW + 30);
    w.toggle_list(film(12), NOW + 20);
    w.press_thumb(series(5), Thumb::Up);
    w.press_thumb(film(6), Thumb::Down);
    w.flush();
    let again = Watching::load(path);
    assert!(
        again.progress(Item::Film(2)).unwrap().done,
        "seen stays seen"
    );
    assert_eq!(again.progress(Item::Film(2)), w.progress(Item::Film(2)));
    assert_eq!(again.progress(Item::Film(1)), w.progress(Item::Film(1)));
    assert_eq!(again.list(), [series(11), film(12), film(10)], "the order");
    assert_eq!(again.thumb(series(5)), Some(Thumb::Up));
    assert_eq!(again.thumb(film(6)), Some(Thumb::Down));
}

#[test]
fn a_film_and_a_series_with_the_same_number_are_two_titles() {
    let (_dir, path, mut w) = fresh();
    w.toggle_list(series(7), NOW);
    assert!(!w.in_list(film(7)));
    w.toggle_list(film(7), NOW + 1);
    assert_eq!(w.list(), [film(7), series(7)]);
    w.toggle_list(film(7), NOW + 2);
    assert_eq!(w.list(), [series(7)], "the film went, not the series");
    w.press_thumb(series(7), Thumb::Up);
    assert_eq!(w.thumb(film(7)), None);
    w.press_thumb(film(7), Thumb::Down);
    w.flush();
    let again = Watching::load(path);
    assert_eq!(again.thumb(series(7)), Some(Thumb::Up));
    assert_eq!(again.thumb(film(7)), Some(Thumb::Down));
}

#[test]
fn a_list_press_or_a_thumb_press_alone_is_a_change_to_write() {
    let (_dir, path, mut w) = fresh();
    w.toggle_list(film(1), NOW);
    assert!(w.is_dirty());
    w.flush();
    assert!(!w.is_dirty());
    w.press_thumb(film(1), Thumb::Up);
    assert!(w.is_dirty());
    w.flush();
    let again = Watching::load(path);
    assert!(again.in_list(film(1)));
    assert_eq!(again.thumb(film(1)), Some(Thumb::Up));
}

#[test]
fn a_film_title_shows_the_version_played_last_and_not_the_one_played_furthest() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 3000.0, 9000.0, None, NOW);
    w.played(Item::Film(2), 1200.0, 6000.0, None, NOW + 60);
    let (item, p) = w.latest([Item::Film(1), Item::Film(2)]).unwrap();
    assert_eq!((item, p.pos), (Item::Film(2), 1200));
}

#[test]
fn the_pace_is_five_seconds_and_a_failed_write_waits_as_long() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let mut w = Watching::load(path.clone());
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert!(path.exists(), "the first write goes at once");
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 1);
    w.save_if_due(t0 + Duration::from_millis(4999));
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 600, "4.999 s: not yet");
    w.save_if_due(t0 + Duration::from_secs(5));
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 601, "5 s: now");
    // A folder where the file goes: the next write fails, and the one after
    // it waits 5 s as well.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    w.played(Item::Film(1), 602.0, 6000.0, None, NOW + 2);
    let failed_at = t0 + Duration::from_secs(10);
    w.save_if_due(failed_at);
    assert!(w.is_dirty());
    std::fs::remove_dir(&path).unwrap();
    w.save_if_due(failed_at + Duration::from_millis(4999));
    assert!(!path.exists(), "a failed write is not tried again at once");
    w.save_if_due(failed_at + Duration::from_secs(5));
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 602);
}

#[test]
fn a_store_nothing_changed_in_is_not_written_by_the_pace_either() {
    let (_dir, path, mut w) = fresh();
    let t0 = Instant::now();
    w.save_if_due(t0 + 100 * WRITE_EVERY);
    assert!(!path.exists(), "nothing ever changed: no file");
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();
    w.save_if_due(t0 + 100 * WRITE_EVERY);
    assert!(!path.exists(), "nothing changed since the last write");
}

#[test]
fn the_file_is_written_pretty_as_the_spec_shows_it() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.flush();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\n  \"items\": {\n"), "{text}");
    assert!(text.contains("\n    \"m:1\": {\n"), "{text}");
}

#[test]
fn a_write_goes_through_a_temporary_file_beside_the_old_one() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.flush();
    let old = std::fs::read(&path).unwrap();
    // A folder where the temporary file goes: nothing can be written beside
    // the old file, and the old file stays whole.
    std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
    w.played(Item::Film(1), 700.0, 6000.0, None, NOW + 1);
    w.flush();
    assert!(w.is_dirty(), "not written");
    assert_eq!(std::fs::read(&path).unwrap(), old);
}
