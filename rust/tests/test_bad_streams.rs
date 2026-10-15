//! The defect marks of an M3U source (bad_streams.rs): suspicions, the
//! confirmation by another channel's first picture, the marks the check sets,
//! the seven days, the file.

use bongo_player::bad_streams::{
    BadStreams, Confirmed, Failure, Picture, CONFIRM_WITHIN, KEEP_SECS,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

/// A first failure of a channel that is not marked: it becomes a suspect.
fn suspect(bad: &mut BadStreams, stream_id: i64, name: &str, at: Instant, at_unix: i64) {
    assert_eq!(bad.failed(stream_id, name, at, at_unix), Failure::Suspected);
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

/// The log holds the one line that says the marks file could not be read.
fn assert_unreadable_line(log: &str) {
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("m3u: defect marks unreadable ("), "{log}");
    assert!(
        log.trim_end().ends_with(") - none until the next change"),
        "{log}"
    );
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
    suspect(&mut bad, 1, "Een", t0, NOW);
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
    suspect(&mut bad, 1, "Een", t0, NOW);
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
    suspect(&mut bad, 1, "Een", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + CONFIRM_WITHIN + secs(1));
    assert_eq!(seen, Picture::default());
    assert!(!bad.is_marked(1));
    assert_eq!(bad.suspects(), 0, "lapsed");
}

#[test]
fn its_own_picture_drops_the_suspicion() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    suspect(&mut bad, 1, "Een", t0, NOW);
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
        suspect(
            &mut bad,
            id,
            &format!("K{id}"),
            t0 + secs(10 * id as u64),
            NOW,
        );
    }
    let seen = bad.first_picture(Some(6), t0 + secs(50) + CONFIRM_WITHIN + secs(1));
    assert!(seen.confirmed.is_empty(), "{seen:?}");
    assert!(bad.marks().is_empty());
}

#[test]
fn a_stream_that_is_no_channel_of_the_list_confirms_too() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    suspect(&mut bad, 1, "Een", t0, NOW);
    assert_eq!(bad.first_picture(None, t0 + secs(3)).confirmed, een(NOW));
}

#[test]
fn a_second_failure_of_a_suspect_counts_from_the_latest() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    suspect(&mut bad, 1, "Een", t0, NOW);
    suspect(&mut bad, 1, "Een", t0 + secs(100), NOW + 100);
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
    suspect(
        &mut bad,
        12345,
        "https://user:secret@x.example/1.m3u8",
        t0,
        NOW,
    );
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
    suspect(&mut bad, 1, "Een", t0, NOW);
    suspect(&mut bad, 7, "Zeven", t0 + secs(1), NOW + 1);
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

#[test]
fn a_mark_dated_later_than_now_counts_from_now() {
    // The clock was wrong when the mark was set, or has stepped back since.
    let (dir, mut bad) = fresh();
    bad.mark(1, NOW + 10 * KEEP_SECS);
    bad.mark(2, NOW - 60);
    assert_eq!(bad.expire(NOW), 0, "neither is older than seven days");
    assert_eq!(bad.marks().get(&1), Some(&NOW), "taken as now");
    assert_eq!(bad.marks().get(&2), Some(&(NOW - 60)), "a past mark stays");
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks(), bad.marks(), "written");
    // Nothing moves a second time, so nothing is written a second time.
    std::fs::remove_file(marks_file(dir.path())).unwrap();
    assert_eq!(bad.expire(NOW), 0);
    assert!(!marks_file(dir.path()).exists(), "nothing moved: no write");
    // From now on it ages like any other mark, counted from now.
    assert_eq!(bad.expire(NOW + KEEP_SECS), 1, "the past mark goes");
    assert!(bad.is_marked(1), "seven days exactly: kept");
    assert_eq!(bad.expire(NOW + KEEP_SECS + 1), 1);
    assert!(bad.marks().is_empty());
}

#[test]
fn a_write_that_fails_keeps_the_marks_in_memory_and_leaves_no_temp_file_behind() {
    let dir = tempfile::tempdir().unwrap();

    // Nothing can be written below a file, and the folder of this marks file is one.
    let in_the_way = dir.path().join("in_the_way");
    std::fs::write(&in_the_way, "x").unwrap();
    let mut bad = BadStreams::load(in_the_way.join("bad_streams.json"));
    let ((), log) = logged(|| bad.mark(1, NOW));
    assert!(log.contains("m3u: defect marks not written ("), "{log}");
    assert_eq!(bad.marks().get(&1), Some(&NOW), "kept in memory");
    assert_eq!(std::fs::read_to_string(&in_the_way).unwrap(), "x");

    // The marks path is a folder: the temp file is written, the rename onto the
    // folder fails, and the temp file goes again.
    let path = marks_file(dir.path());
    let mut bad = BadStreams::load(path.clone());
    std::fs::create_dir(&path).unwrap();
    let ((), log) = logged(|| bad.mark(2, NOW));
    assert!(log.contains("m3u: defect marks not written ("), "{log}");
    assert_eq!(bad.marks().get(&2), Some(&NOW), "kept in memory");
    assert!(path.is_dir(), "the folder is still there");
    assert!(
        !path.with_extension("json.tmp").exists(),
        "no temp file behind"
    );
}

#[test]
fn a_file_that_cannot_be_read_reads_as_none_and_is_logged_once() {
    let dir = tempfile::tempdir().unwrap();

    // A folder at the marks path: reading it fails with an error that is not
    // "not found".
    let folder = marks_file(dir.path());
    std::fs::create_dir(&folder).unwrap();
    let (bad, log) = logged(|| BadStreams::load(folder.clone()));
    assert!(bad.marks().is_empty());
    assert_unreadable_line(&log);

    // A file that is no JSON is read but not parsed: the same line says so.
    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, "{not json").unwrap();
    let (bad, log) = logged(|| BadStreams::load(broken.clone()));
    assert!(bad.marks().is_empty());
    assert_unreadable_line(&log);

    // A file that is not there is no trouble: nothing is logged.
    let (bad, log) = logged(|| BadStreams::load(dir.path().join("missing.json")));
    assert!(bad.marks().is_empty());
    assert_eq!(log, "");
}
