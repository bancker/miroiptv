//! What was watched (vod/watching.rs, spec 2026-10-15 §4.5): the file's
//! format, the 60 s and 90 % rules, Mijn lijst and the thumbs, the writes 5 s
//! apart and at a stop, and a file that cannot be read.

use bongo_player::vod::watching::{EpisodeOf, Item, Progress, Thumb, Watching, WRITE_EVERY};
use bongo_player::vod::{Kind, TitleId};
use std::path::{Path, PathBuf};
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
    for body in [
        &b"{\"version\":1,\"items\":"[..],
        br#"{"version":2,"items":{}}"#,
    ] {
        std::fs::write(&path, body).unwrap();
        let mut w = Watching::load(path.clone());
        assert!(w.progress(Item::Film(1)).is_none());
        assert_eq!(
            std::fs::read(path.with_extension("json.bad")).unwrap(),
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
