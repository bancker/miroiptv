//! Defect marks for the channels of an M3U list (spec 2026-10-15, §4).
//!
//! A channel whose start ends without a first picture is a suspect. It is
//! confirmed once another channel shows a first picture within two minutes of
//! that failure: the connection worked. Its address is then asked once more
//! (`stream_check.rs`), and only a dead answer marks it (spec §4.2), so a Wi-Fi
//! drop of under two minutes marks no good channel. Without a confirmation the
//! suspicion lapses. A first picture of the channel itself clears its
//! suspicion and its mark; a marked channel that fails again is marked anew at
//! once, without a check.
//!
//! Marks are kept per source for seven days, in `bad_streams.json` for source
//! 1 and `bad_streams-<id>.json` for the others (`Storage::bad_streams_path`):
//! `{"version":1,"marks":{"<stream_id>":<unix seconds>}}`, ids and times only.
//! No names and no addresses: an M3U address can hold a password. Suspicions
//! live in memory only. The callers bring the clocks: an `Instant` for the two
//! minutes, unix seconds for the seven days.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long after a failure another channel's first picture confirms it.
pub const CONFIRM_WITHIN: Duration = Duration::from_secs(120);

/// How long a mark is kept: seven days, in seconds.
pub const KEEP_SECS: i64 = 7 * 24 * 60 * 60;

/// The file's format.
const VERSION: u32 = 1;

/// The marks file as it is written.
#[derive(serde::Serialize, serde::Deserialize)]
struct MarksFile {
    version: u32,
    marks: BTreeMap<i64, i64>,
}

/// A channel whose start failed, waiting for another channel's picture.
struct Suspect {
    stream_id: i64,
    name: String,
    failed_at: Instant,
    failed_unix: i64,
}

/// What a failure did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The channel is a suspect now, counted from this failure.
    Suspected,
    /// It was marked already: the mark's seven days start again.
    Renewed,
}

/// A suspect that another channel's first picture confirmed: its address is
/// checked (`stream_check.rs`) before it gets the mark (spec §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub stream_id: i64,
    pub name: String,
    /// When it failed: the mark keeps this time.
    pub failed_unix: i64,
}

/// What a first picture did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Picture {
    /// The suspects it confirmed, for the check. None of them is marked yet.
    pub confirmed: Vec<Confirmed>,
    /// The channel that shows it was marked: its mark is gone.
    pub cleared: bool,
}

/// The marks of one source, and the suspicions of this session.
pub struct BadStreams {
    path: PathBuf,
    /// Stream id → when it failed, unix seconds.
    marks: HashMap<i64, i64>,
    suspects: Vec<Suspect>,
}

impl BadStreams {
    /// The marks in the file at `path`. A file that is not there holds none.
    /// One that cannot be read, or is no marks file of this version, holds none
    /// either: the log says so, and the next change writes it anew.
    pub fn load(path: PathBuf) -> Self {
        let marks = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<MarksFile>(&bytes) {
                Ok(file) if file.version == VERSION => file.marks.into_iter().collect(),
                Ok(file) => {
                    tracing::warn!("m3u: defect marks of version {} not read", file.version);
                    HashMap::new()
                }
                Err(e) => {
                    tracing::warn!(
                        "m3u: defect marks unreadable ({e}) - none until the next change"
                    );
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                tracing::warn!("m3u: defect marks unreadable ({e}) - none until the next change");
                HashMap::new()
            }
        };
        Self {
            path,
            marks,
            suspects: Vec::new(),
        }
    }

    /// When each marked channel failed: stream id → unix seconds.
    pub fn marks(&self) -> &HashMap<i64, i64> {
        &self.marks
    }

    pub fn is_marked(&self, stream_id: i64) -> bool {
        self.marks.contains_key(&stream_id)
    }

    /// How many suspicions wait for a picture.
    pub fn suspects(&self) -> usize {
        self.suspects.len()
    }

    /// Channel `stream_id` (`name`, for the log) failed before its first
    /// picture at `now` (`now_unix`). A marked one is marked anew at once and
    /// the file written; another one is a suspect, counted from this failure.
    pub fn failed(&mut self, stream_id: i64, name: &str, now: Instant, now_unix: i64) -> Failure {
        // Suspicions too old to be confirmed would only lapse: they go now.
        self.suspects.retain(|s| {
            s.stream_id != stream_id && now.saturating_duration_since(s.failed_at) <= CONFIRM_WITHIN
        });
        if let Some(at) = self.marks.get_mut(&stream_id) {
            *at = now_unix;
            self.save();
            return Failure::Renewed;
        }
        self.suspects.push(Suspect {
            stream_id,
            name: name.to_owned(),
            failed_at: now,
            failed_unix: now_unix,
        });
        Failure::Suspected
    }

    /// A first picture at `now` of channel `stream_id` (`None`: a stream that is
    /// no channel of the list). The suspects that failed within
    /// [`CONFIRM_WITHIN`] before it are confirmed: they leave the suspects, for
    /// the caller to check their addresses and [`BadStreams::mark`] the dead
    /// ones. Older ones lapse. The channel's own suspicion goes, and so does its
    /// mark (the file is written then).
    pub fn first_picture(&mut self, stream_id: Option<i64>, now: Instant) -> Picture {
        let mut picture = Picture::default();
        for s in std::mem::take(&mut self.suspects) {
            if Some(s.stream_id) == stream_id {
                continue;
            }
            if now.saturating_duration_since(s.failed_at) <= CONFIRM_WITHIN {
                picture.confirmed.push(Confirmed {
                    stream_id: s.stream_id,
                    name: s.name,
                    failed_unix: s.failed_unix,
                });
            }
        }
        if let Some(id) = stream_id {
            picture.cleared = self.marks.remove(&id).is_some();
        }
        if picture.cleared {
            self.save();
        }
        picture
    }

    /// Marks channel `stream_id`, which failed at `failed_unix` and whose
    /// address the check found dead, and writes the file. A mark it has for a
    /// later failure keeps that time. A suspicion of it (it failed again while
    /// its address was asked) goes: from now on a failure renews the mark.
    pub fn mark(&mut self, stream_id: i64, failed_unix: i64) {
        let at = self.marks.entry(stream_id).or_insert(failed_unix);
        *at = (*at).max(failed_unix);
        self.suspects.retain(|s| s.stream_id != stream_id);
        self.save();
    }

    /// Marks older than [`KEEP_SECS`] at `now_unix` go; how many went. Writes
    /// the file when any did.
    pub fn expire(&mut self, now_unix: i64) -> usize {
        let before = self.marks.len();
        self.marks
            .retain(|_, at| now_unix.saturating_sub(*at) <= KEEP_SECS);
        let gone = before - self.marks.len();
        if gone > 0 {
            self.save();
        }
        gone
    }

    /// Writes the marks. A failure is logged; the marks stay in memory.
    fn save(&self) {
        let file = MarksFile {
            version: VERSION,
            marks: self.marks.iter().map(|(id, at)| (*id, *at)).collect(),
        };
        let written = serde_json::to_vec(&file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(e) = written {
            tracing::warn!("m3u: defect marks not written ({e})");
        }
    }
}

/// `bytes` to `path` through a temp file beside it, so a write that fails
/// leaves the old file whole.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
