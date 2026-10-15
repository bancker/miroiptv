//! The defect marks of an M3U source (bad_streams.rs): suspicions, the
//! confirmation by another channel's first picture, the marks the check sets,
//! the seven days, the file.

use bongo_player::bad_streams::{
    BadStreams, Confirmed, Failure, Picture, CONFIRM_WITHIN, KEEP_SECS,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 15 October 2026, 00:00 UTC, as unix seconds.
const NOW: i64 = 1_792_022_400;

fn marks_file(dir: &Path) -> PathBuf {
    dir.join("bad_streams.json")
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Marks of a folder that holds no file yet, and the folder.
fn fresh() -> (tempfile::TempDir, BadStreams) {
    let dir = tempfile::tempdir().unwrap();
    let bad = BadStreams::load(marks_file(dir.path()));
    (dir, bad)
}

/// Channel 1, "Een", confirmed: it failed at `failed_unix`.
fn een(failed_unix: i64) -> Vec<Confirmed> {
    vec![Confirmed {
        stream_id: 1,
        name: "Een".to_string(),
        failed_unix,
    }]
}

/// What the app does with each channel a picture confirmed once the check
/// finds its address dead too: it marks it.
fn mark_all(bad: &mut BadStreams, picture: Picture) {
    for c in picture.confirmed {
        bad.mark(c.stream_id, c.failed_unix);
    }
}

#[test]
fn a_failure_alone_marks_nothing_and_writes_nothing() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    assert_eq!(bad.failed(1, "Een", t0, NOW), Failure::Suspected);
    assert!(!bad.is_marked(1));
    assert_eq!(bad.suspects(), 1);
    assert!(
        !marks_file(dir.path()).exists(),
        "a suspicion lives in memory only"
    );
}

#[test]
fn another_channels_picture_within_two_minutes_confirms_it_for_the_check() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + CONFIRM_WITHIN);
    assert_eq!(
        seen,
        Picture {
            confirmed: een(NOW),
            cleared: false
        }
    );
    assert!(!bad.is_marked(1), "its address is checked first");
    assert_eq!(bad.suspects(), 0, "a suspect is confirmed once");
    assert!(!marks_file(dir.path()).exists());
}

#[test]
fn a_mark_after_the_check_keeps_the_time_of_the_failure_and_is_written_at_once() {
    let (dir, mut bad) = fresh();
    bad.mark(1, NOW);
    assert_eq!(bad.marks().get(&1), Some(&NOW));
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().get(&1), Some(&NOW), "written");
    bad.mark(1, NOW - 50);
    assert_eq!(bad.marks().get(&1), Some(&NOW), "the latest failure counts");
}

#[test]
fn a_mark_drops_the_suspicion_of_its_channel_and_its_next_failure_renews_it() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    // It failed again while its address was being checked.
    bad.failed(1, "Een", t0, NOW);
    bad.mark(1, NOW - 10);
    assert_eq!(bad.suspects(), 0);
    assert_eq!(
        bad.failed(1, "Een", t0 + secs(5), NOW + 5),
        Failure::Renewed
    );
}

#[test]
fn a_picture_after_two_minutes_confirms_nothing_and_the_suspicion_lapses() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + CONFIRM_WITHIN + secs(1));
    assert_eq!(seen, Picture::default());
    assert!(!bad.is_marked(1));
    assert_eq!(bad.suspects(), 0, "lapsed");
}

#[test]
fn its_own_picture_drops_the_suspicion() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    assert_eq!(bad.first_picture(Some(1), t0 + secs(5)), Picture::default());
    // A later picture of another channel finds no suspect left.
    assert_eq!(
        bad.first_picture(Some(2), t0 + secs(10)),
        Picture::default()
    );
    assert!(!bad.is_marked(1));
}

#[test]
fn a_dropped_connection_confirms_nothing() {
    // Five channels fail in a row; nothing shows for more than two minutes.
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    for id in 1..=5 {
        bad.failed(id, &format!("K{id}"), t0 + secs(10 * id as u64), NOW);
    }
    let seen = bad.first_picture(Some(6), t0 + secs(50) + CONFIRM_WITHIN + secs(1));
    assert!(seen.confirmed.is_empty(), "{seen:?}");
    assert!(bad.marks().is_empty());
}

#[test]
fn a_stream_that_is_no_channel_of_the_list_confirms_too() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    assert_eq!(bad.first_picture(None, t0 + secs(3)).confirmed, een(NOW));
}

#[test]
fn a_second_failure_of_a_suspect_counts_from_the_latest() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    bad.failed(1, "Een", t0 + secs(100), NOW + 100);
    assert_eq!(bad.suspects(), 1, "one suspicion per channel");
    let seen = bad.first_picture(Some(2), t0 + secs(200));
    assert_eq!(seen.confirmed, een(NOW + 100));
}

#[test]
fn a_marked_channels_picture_clears_its_mark_and_the_file_says_so() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.mark(1, NOW);
    let seen = bad.first_picture(Some(1), t0 + secs(60));
    assert!(seen.cleared);
    assert!(!bad.is_marked(1));
    let again = BadStreams::load(marks_file(dir.path()));
    assert!(again.marks().is_empty(), "written");
}

#[test]
fn a_marked_channel_that_fails_again_starts_its_seven_days_again_without_confirmation() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.mark(1, NOW);
    let later = NOW + 3 * 24 * 3600;
    assert_eq!(bad.failed(1, "Een", t0 + secs(30), later), Failure::Renewed);
    assert_eq!(bad.suspects(), 0, "no check for a renewal");
    assert_eq!(bad.marks().get(&1), Some(&later));
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().get(&1), Some(&later), "written at once");
}

#[test]
fn marks_older_than_seven_days_expire() {
    let (dir, mut bad) = fresh();
    for (id, failed) in [
        (1, NOW - KEEP_SECS - 1),
        (2, NOW - KEEP_SECS),
        (3, NOW - 60),
    ] {
        bad.mark(id, failed);
    }
    assert_eq!(bad.expire(NOW), 1);
    assert!(!bad.is_marked(1), "more than seven days");
    assert!(bad.is_marked(2) && bad.is_marked(3));
    assert_eq!(bad.expire(NOW), 0);
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().len(), 2, "written");
}

#[test]
fn the_file_holds_ids_and_times_only() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    // A name may be an address with a password in it: it never reaches the file.
    bad.failed(12345, "https://user:secret@x.example/1.m3u8", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + secs(1));
    mark_all(&mut bad, seen);
    let text = std::fs::read_to_string(marks_file(dir.path())).unwrap();
    assert_eq!(
        text,
        format!("{{\"version\":1,\"marks\":{{\"12345\":{NOW}}}}}")
    );
}

#[test]
fn the_marks_survive_a_new_load() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    bad.failed(7, "Zeven", t0 + secs(1), NOW + 1);
    let seen = bad.first_picture(Some(2), t0 + secs(2));
    mark_all(&mut bad, seen);
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks(), bad.marks());
    assert_eq!(again.marks().len(), 2);
    assert_eq!(again.suspects(), 0, "suspicions are not kept");
}

#[test]
fn a_broken_file_reads_as_none_and_the_next_change_writes_it_anew() {
    let dir = tempfile::tempdir().unwrap();
    let path = marks_file(dir.path());
    std::fs::write(&path, "{not json").unwrap();
    let mut bad = BadStreams::load(path.clone());
    assert!(bad.marks().is_empty());
    bad.mark(7, NOW);
    let again = BadStreams::load(path);
    assert_eq!(again.marks().get(&7), Some(&NOW));
}

#[test]
fn a_file_of_another_version_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = marks_file(dir.path());
    std::fs::write(&path, r#"{"version":2,"marks":{"1":5}}"#).unwrap();
    assert!(BadStreams::load(path).marks().is_empty());
}

#[test]
fn nothing_to_expire_writes_no_file() {
    let (dir, mut bad) = fresh();
    assert_eq!(bad.expire(NOW), 0);
    assert!(!marks_file(dir.path()).exists());
}
