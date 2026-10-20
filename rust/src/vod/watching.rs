//! What was watched (spec 2026-10-15 §4.5): where each film version and
//! episode stopped and whether it was seen, the user's list ("Mijn lijst")
//! and thumbs. One file per source, beside its favorites (`watching.json`,
//! `watching-<id>.json`): numbers and times only, no names, no addresses.
//!
//! A position under 60 s is "not started"; from 90 % of the length on, or at
//! the end of the file, it is seen. Writes go through a temporary file, at
//! most one every 5 s, and always at a stop (`flush`). A file that cannot be
//! read is kept as `watching.json.bad`; the store starts empty and says so
//! once in the log.

use super::{write_atomic_with, Kind, TitleId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Under this a position is "not started".
pub const STARTED_SECS: f64 = 60.0;
/// From this share of its length on, a film or an episode is seen.
pub const SEEN_SHARE: f64 = 0.9;
/// Writes are this far apart at least; a stop writes at once.
pub const WRITE_EVERY: Duration = Duration::from_secs(5);

const VERSION: u32 = 1;

/// What plays: a film's version by its stream id (`m:<id>`), or an episode by
/// its id (`e:<id>`). A film title's progress is that of its most recent
/// version ([`Watching::latest`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    Film(i64),
    Episode(i64),
}

impl Item {
    fn key(self) -> String {
        match self {
            Item::Film(id) => format!("m:{id}"),
            Item::Episode(id) => format!("e:{id}"),
        }
    }

    fn of_key(key: &str) -> Option<Item> {
        let (kind, id) = key.split_once(':')?;
        let id = id.parse().ok()?;
        match kind {
            "m" => Some(Item::Film(id)),
            "e" => Some(Item::Episode(id)),
            _ => None,
        }
    }
}

/// The episode an item is: for "Verder kijken" and the next episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeOf {
    pub series: i64,
    pub season: u32,
    pub number: u32,
}

/// Where an item stopped, and whether it was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Seconds.
    pub pos: u32,
    /// Its length in seconds; 0: unknown.
    pub dur: u32,
    /// When, unix seconds.
    pub at: i64,
    pub done: bool,
    pub episode: Option<EpisodeOf>,
}

/// A thumb: 👍 or 👎.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thumb {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
struct WatchFile {
    version: u32,
    #[serde(default)]
    items: BTreeMap<String, ItemEntry>,
    #[serde(default)]
    list: Vec<ListEntry>,
    #[serde(default)]
    thumbs: BTreeMap<String, i8>,
}

#[derive(Serialize, Deserialize)]
struct ItemEntry {
    pos: u32,
    dur: u32,
    at: i64,
    done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    series: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct ListEntry {
    k: String,
    id: i64,
    at: i64,
}

/// A title as the list and the thumbs name it: `s:<id>` or `m:<id>`.
fn title_key(t: TitleId) -> String {
    format!("{}:{}", kind_letter(t.kind), t.id)
}

fn kind_letter(kind: Kind) -> &'static str {
    match kind {
        Kind::Series => "s",
        Kind::Film => "m",
    }
}

fn kind_of(letter: &str) -> Option<Kind> {
    match letter {
        "s" => Some(Kind::Series),
        "m" => Some(Kind::Film),
        _ => None,
    }
}

fn title_of_key(key: &str) -> Option<TitleId> {
    let (k, id) = key.split_once(':')?;
    Some(TitleId {
        kind: kind_of(k)?,
        id: id.parse().ok()?,
    })
}

/// What one source watched, its list and its thumbs.
pub struct Watching {
    path: PathBuf,
    items: HashMap<Item, Progress>,
    /// The list, in the order of adding, with when.
    list: Vec<(TitleId, i64)>,
    thumbs: HashMap<TitleId, Thumb>,
    dirty: bool,
    last_write: Option<Instant>,
    /// The last write failed: the next failure is not logged again.
    failing: bool,
}

impl Watching {
    /// What the file at `path` holds. A missing file holds nothing; one that
    /// cannot be read is kept as `<name>.bad`, and the store starts empty.
    pub fn load(path: PathBuf) -> Self {
        let mut me = Self {
            path,
            items: HashMap::new(),
            list: Vec::new(),
            thumbs: HashMap::new(),
            dirty: false,
            last_write: None,
            failing: false,
        };
        let bytes = match std::fs::read(&me.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return me,
            Err(e) => {
                tracing::warn!(
                    "vod: {} unreadable ({e}) - starting empty",
                    file_name(&me.path)
                );
                return me;
            }
        };
        let file = match serde_json::from_slice::<WatchFile>(&bytes) {
            Ok(file) if file.version == VERSION => file,
            Ok(file) => {
                me.keep_as_bad(&format!("version {}", file.version));
                return me;
            }
            Err(e) => {
                me.keep_as_bad(&e.to_string());
                return me;
            }
        };
        for (key, e) in file.items {
            let Some(item) = Item::of_key(&key) else {
                continue;
            };
            let episode = e.series.map(|series| EpisodeOf {
                series,
                season: e.s.unwrap_or(0),
                number: e.n.unwrap_or(0),
            });
            me.items.insert(
                item,
                Progress {
                    pos: e.pos,
                    dur: e.dur,
                    at: e.at,
                    done: e.done,
                    episode,
                },
            );
        }
        for e in file.list {
            if let Some(kind) = kind_of(&e.k) {
                let t = TitleId { kind, id: e.id };
                if !me.list.iter().any(|(had, _)| *had == t) {
                    me.list.push((t, e.at));
                }
            }
        }
        for (key, value) in file.thumbs {
            let thumb = match value {
                1 => Thumb::Up,
                -1 => Thumb::Down,
                _ => continue,
            };
            if let Some(t) = title_of_key(&key) {
                me.thumbs.insert(t, thumb);
            }
        }
        me
    }

    /// Moves the unreadable file aside as `<name>.bad` (over an older one)
    /// and says so once.
    fn keep_as_bad(&self, why: &str) {
        let bad = self.path.with_extension("json.bad");
        let kept = std::fs::rename(&self.path, &bad).is_ok();
        tracing::warn!(
            "vod: {} unreadable ({why}) - {} - starting empty",
            file_name(&self.path),
            if kept {
                "kept as .bad"
            } else {
                "left as it is"
            }
        );
    }

    /// Where `item` stopped.
    pub fn progress(&self, item: Item) -> Option<&Progress> {
        self.items.get(&item)
    }

    /// The most recent progress of `items`: a film title shows its versions'
    /// latest, so a new default version keeps where an older one stopped.
    pub fn latest(&self, items: impl IntoIterator<Item = Item>) -> Option<(Item, &Progress)> {
        items
            .into_iter()
            .filter_map(|i| Some((i, self.items.get(&i)?)))
            .max_by_key(|(_, p)| p.at)
    }

    /// The player says `item` is at `pos` of `dur` seconds (`now_unix`).
    /// Under 60 s an item is not started: one in progress goes, one that was
    /// seen stays seen. From 90 % on it is seen; under that, one that was seen
    /// is watched again and in progress. The same place again changes nothing.
    pub fn played(
        &mut self,
        item: Item,
        pos: f64,
        dur: f64,
        episode: Option<EpisodeOf>,
        now_unix: i64,
    ) {
        if !pos.is_finite() || pos < 0.0 {
            return;
        }
        let old = self.items.get(&item).copied();
        if pos < STARTED_SECS {
            if old.is_some_and(|p| !p.done) {
                self.items.remove(&item);
                self.dirty = true;
            }
            return;
        }
        let dur = if dur.is_finite() && dur > 0.0 {
            dur as u32
        } else {
            old.map_or(0, |p| p.dur)
        };
        let seen = dur > 0 && pos >= SEEN_SHARE * f64::from(dur);
        let new = Progress {
            pos: pos as u32,
            dur,
            at: now_unix,
            done: seen,
            episode: episode.or(old.and_then(|p| p.episode)),
        };
        let same = |p: &Progress| {
            p.pos == new.pos && p.dur == new.dur && p.done == new.done && p.episode == new.episode
        };
        if !old.is_some_and(|p| same(&p)) {
            self.items.insert(item, new);
            self.dirty = true;
        }
    }

    /// The end of `item`'s file was reached: it is seen.
    pub fn finished(&mut self, item: Item, dur: f64, episode: Option<EpisodeOf>, now_unix: i64) {
        let old = self.items.get(&item).copied();
        let dur = if dur.is_finite() && dur > 0.0 {
            dur as u32
        } else {
            old.map_or(0, |p| p.dur)
        };
        self.items.insert(
            item,
            Progress {
                pos: dur.max(old.map_or(0, |p| p.pos)),
                dur,
                at: now_unix,
                done: true,
                episode: episode.or(old.and_then(|p| p.episode)),
            },
        );
        self.dirty = true;
    }

    /// Whether title `t` is in the list.
    pub fn in_list(&self, t: TitleId) -> bool {
        self.list.iter().any(|(had, _)| *had == t)
    }

    /// ＋: puts `t` in the list, or takes it off when it is there. Whether it is
    /// in the list now.
    pub fn toggle_list(&mut self, t: TitleId, now_unix: i64) -> bool {
        self.dirty = true;
        if let Some(i) = self.list.iter().position(|(had, _)| *had == t) {
            self.list.remove(i);
            false
        } else {
            self.list.push((t, now_unix));
            true
        }
    }

    /// The list, the latest added first.
    pub fn list(&self) -> Vec<TitleId> {
        let mut list = self.list.clone();
        list.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        list.into_iter().map(|(t, _)| t).collect()
    }

    /// The thumb `t` has.
    pub fn thumb(&self, t: TitleId) -> Option<Thumb> {
        self.thumbs.get(&t).copied()
    }

    /// 👍 or 👎 pressed for `t`: it gets that thumb, or loses it when it had
    /// it already; 👍 and 👎 exclude each other. The thumb it has now.
    pub fn press_thumb(&mut self, t: TitleId, thumb: Thumb) -> Option<Thumb> {
        self.dirty = true;
        if self.thumbs.get(&t) == Some(&thumb) {
            self.thumbs.remove(&t);
            None
        } else {
            self.thumbs.insert(t, thumb);
            Some(thumb)
        }
    }

    /// Something changed that the file does not hold yet.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes the file when something changed and the last write is 5 s ago
    /// (`now`): what the player reports every second is written every 5 s.
    pub fn save_if_due(&mut self, now: Instant) {
        let due = self
            .last_write
            .is_none_or(|t| now.saturating_duration_since(t) >= WRITE_EVERY);
        if self.dirty && due {
            self.write(now);
        }
    }

    /// Writes the file now when something changed: a stop.
    pub fn flush(&mut self) {
        if self.dirty {
            self.write(Instant::now());
        }
    }

    fn write(&mut self, now: Instant) {
        self.last_write = Some(now);
        let file = WatchFile {
            version: VERSION,
            items: self
                .items
                .iter()
                .map(|(item, p)| {
                    let entry = ItemEntry {
                        pos: p.pos,
                        dur: p.dur,
                        at: p.at,
                        done: p.done,
                        series: p.episode.map(|e| e.series),
                        s: p.episode.map(|e| e.season),
                        n: p.episode.map(|e| e.number),
                    };
                    (item.key(), entry)
                })
                .collect(),
            list: self
                .list
                .iter()
                .map(|(t, at)| ListEntry {
                    k: kind_letter(t.kind).to_owned(),
                    id: t.id,
                    at: *at,
                })
                .collect(),
            thumbs: self
                .thumbs
                .iter()
                .map(|(t, thumb)| {
                    let value = match thumb {
                        Thumb::Up => 1,
                        Thumb::Down => -1,
                    };
                    (title_key(*t), value)
                })
                .collect(),
        };
        let written = write_atomic_with(&self.path, |out| {
            serde_json::to_writer_pretty(out, &file).map_err(std::io::Error::other)
        });
        match written {
            Ok(()) => {
                self.dirty = false;
                self.failing = false;
            }
            Err(e) => {
                if !self.failing {
                    tracing::warn!("vod: {} not written ({e})", file_name(&self.path));
                }
                self.failing = true;
            }
        }
    }
}

/// The file's name, for the log: its folder is the user's profile.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}
