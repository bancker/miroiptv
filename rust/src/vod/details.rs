//! A film's and a series' details (spec 2026-10-15 §4.3), asked of the portal
//! when their page opens: on the tokio runtime, at most 2 at a time, the
//! newest request first. Each answer is kept as a file with only what the
//! pages show (`config\cache\vod\<host>_<port>\<s|m><id>.json`, a
//! `VodDetails`), and without an image address that has a query: `redact`
//! sees no token there (memory keeps it, so the page shows it). A series' file
//! is asked again once it is older than 24 hours, a film's after 30 days; the
//! old file shows meanwhile. Opening a page reads its file in the same call, so
//! it shows in the frame that opens it.

use super::cache::portal_key;
use super::{write_atomic_with, Kind, TitleId};
use crate::args::XtreamCreds;
use crate::portal::{Portal, VodDetails};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A series' details are asked again after this, in seconds: a day.
pub const SERIES_FRESH_SECS: i64 = 24 * 60 * 60;
/// A film's after this: 30 days.
pub const FILM_FRESH_SECS: i64 = 30 * 24 * 60 * 60;
/// Requests to the portal at the same time.
pub const AT_ONCE: usize = 2;
/// Requests that wait for a place; an older one is dropped (its page is long
/// closed, and opening it asks again).
pub const WAITING: usize = 8;
/// Titles whose details are kept in memory; an older one is read from its
/// file again when its page opens.
pub const IN_MEMORY: usize = 64;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct DetailFile {
    version: u32,
    /// When the portal gave them, unix seconds.
    fetched_at: i64,
    details: VodDetails,
}

/// What a page has of its title's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailState {
    /// Asked for, nothing to show yet; or never asked.
    Loading,
    /// There they are. `refreshing`: an old file shows while the portal is
    /// asked again.
    Ready {
        details: Arc<VodDetails>,
        refreshing: bool,
    },
    /// The portal gave none and no file had them: "Details konden niet geladen
    /// worden. Enter probeert opnieuw" (`DetailStore::retry`).
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Idle,
    Waiting,
    InFlight,
    Failed,
}

struct Entry {
    details: Option<Arc<VodDetails>>,
    fetch: Fetch,
    /// When its page last asked, as a count: the least recent leave memory.
    asked: u64,
}

/// What came back from the portal.
struct Arrived {
    id: TitleId,
    answer: Result<VodDetails, String>,
}

/// The details of the titles whose pages were opened.
pub struct DetailStore {
    portal: Arc<dyn Portal>,
    /// Where the files are; none keeps none.
    dir: Option<PathBuf>,
    entries: HashMap<TitleId, Entry>,
    /// Waiting for a place, the newest last.
    waiting: Vec<TitleId>,
    in_flight: usize,
    asks: u64,
    done: Arc<Mutex<Vec<Arrived>>>,
}

/// The folder of the details of the portal with these credentials, in
/// `cache_dir`; none without a portal.
pub fn dir_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf> {
    let c = creds?;
    Some(cache_dir.join("vod").join(portal_key(&c.host, c.port)))
}

/// A title's file name: `s4321.json`, `m5001.json`.
pub fn file_name(id: TitleId) -> String {
    let kind = match id.kind {
        Kind::Series => 's',
        Kind::Film => 'm',
    };
    format!("{kind}{}.json", id.id)
}

/// The id as the log names it: `s4321`, `m5001`.
fn log_name(id: TitleId) -> String {
    file_name(id).trim_end_matches(".json").to_owned()
}

/// How long `id`'s details stay fresh.
fn fresh_secs(id: TitleId) -> i64 {
    match id.kind {
        Kind::Series => SERIES_FRESH_SECS,
        Kind::Film => FILM_FRESH_SECS,
    }
}

impl DetailStore {
    /// Details asked of `portal`, kept as files in `dir`.
    pub fn new(portal: Arc<dyn Portal>, dir: Option<PathBuf>) -> Self {
        Self {
            portal,
            dir,
            entries: HashMap::new(),
            waiting: Vec::new(),
            in_flight: 0,
            asks: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The page of `id` opens at `now_unix`: its file shows at once, and the
    /// portal is asked when there is none or it is too old.
    pub fn request(&mut self, id: TitleId, now_unix: i64) {
        self.asks += 1;
        let asked = self.asks;
        let file = match self.entries.get(&id) {
            Some(e) if e.details.is_some() || e.fetch != Fetch::Idle => None,
            _ => Some(self.read_file(id)),
        };
        let entry = self.entries.entry(id).or_insert(Entry {
            details: None,
            fetch: Fetch::Idle,
            asked,
        });
        entry.asked = asked;
        let mut stale = entry.details.is_none();
        if let Some(Some((details, fetched_at))) = file {
            entry.details = Some(Arc::new(details));
            // A time from the future (a clock that was wrong) counts as now.
            stale = now_unix.saturating_sub(fetched_at.min(now_unix)) > fresh_secs(id);
        }
        if stale && entry.fetch == Fetch::Idle {
            self.wait(id);
        }
        self.forget_old();
        self.start_waiting(now_unix);
    }

    /// Enter on a page whose details failed: asks again.
    pub fn retry(&mut self, id: TitleId, now_unix: i64) {
        if let Some(e) = self.entries.get_mut(&id) {
            if e.fetch == Fetch::Failed {
                e.fetch = Fetch::Idle;
                self.wait(id);
                self.start_waiting(now_unix);
            }
        }
    }

    /// What the page of `id` has.
    pub fn get(&self, id: TitleId) -> DetailState {
        let Some(e) = self.entries.get(&id) else {
            return DetailState::Loading;
        };
        match (&e.details, e.fetch) {
            (Some(details), fetch) => DetailState::Ready {
                details: details.clone(),
                refreshing: matches!(fetch, Fetch::Waiting | Fetch::InFlight),
            },
            (None, Fetch::Failed) => DetailState::Failed,
            (None, _) => DetailState::Loading,
        }
    }

    /// Requests on their way to the portal.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Takes in what came back and starts what waits; once per frame.
    pub fn drain(&mut self, now_unix: i64) {
        let arrived = std::mem::take(&mut *self.done.lock());
        for Arrived { id, answer } in arrived {
            self.in_flight = self.in_flight.saturating_sub(1);
            let Some(e) = self.entries.get_mut(&id) else {
                continue;
            };
            match answer {
                Ok(details) => {
                    e.details = Some(Arc::new(details));
                    e.fetch = Fetch::Idle;
                }
                // An old file keeps showing; without one the page says so.
                Err(_) if e.details.is_some() => e.fetch = Fetch::Idle,
                Err(_) => e.fetch = Fetch::Failed,
            }
        }
        self.start_waiting(now_unix);
    }

    /// Puts `id` at the newest end of the waiting requests.
    fn wait(&mut self, id: TitleId) {
        self.waiting.retain(|w| *w != id);
        self.waiting.push(id);
        if let Some(e) = self.entries.get_mut(&id) {
            e.fetch = Fetch::Waiting;
        }
        while self.waiting.len() > WAITING {
            let dropped = self.waiting.remove(0);
            if let Some(e) = self.entries.get_mut(&dropped) {
                e.fetch = Fetch::Idle;
            }
        }
    }

    /// Starts the newest waiting requests while a place is free.
    fn start_waiting(&mut self, now_unix: i64) {
        while self.in_flight < AT_ONCE {
            let Some(id) = self.waiting.pop() else {
                break;
            };
            if let Some(e) = self.entries.get_mut(&id) {
                e.fetch = Fetch::InFlight;
            }
            self.in_flight += 1;
            self.start(id, now_unix);
        }
    }

    /// Asks the portal for `id`'s details in the background; the answer is
    /// kept as its file before it is handed over.
    fn start(&self, id: TitleId, now_unix: i64) {
        let (portal, done) = (self.portal.clone(), self.done.clone());
        let path = self.dir.as_ref().map(|d| d.join(file_name(id)));
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            let answer = match id.kind {
                Kind::Film => portal.fetch_vod_info(id.id).await,
                Kind::Series => portal.fetch_series_info(id.id).await,
            };
            let answer = match answer {
                Ok(details) => {
                    tracing::info!(
                        "vod: details of {} fetched in {} ms",
                        log_name(id),
                        t0.elapsed().as_millis()
                    );
                    if let Some(path) = path {
                        let file = DetailFile {
                            version: VERSION,
                            fetched_at: now_unix,
                            details: without_query_images(details.clone()),
                        };
                        let _ = tokio::task::spawn_blocking(move || save(&path, &file, id)).await;
                    }
                    Ok(details)
                }
                Err(e) => {
                    tracing::warn!("vod: details of {} not fetched ({e})", log_name(id));
                    Err(e.to_string())
                }
            };
            done.lock().push(Arrived { id, answer });
        });
    }

    /// `id`'s file and when it was fetched; none when there is none, or it is
    /// no details file of this version (it is asked again and written anew).
    fn read_file(&self, id: TitleId) -> Option<(VodDetails, i64)> {
        let path = self.dir.as_ref()?.join(file_name(id));
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice::<DetailFile>(&bytes) {
            Ok(file) if file.version == VERSION => Some((file.details, file.fetched_at)),
            _ => None,
        }
    }

    /// Keeps the [`IN_MEMORY`] titles asked last; one that waits or is on its
    /// way stays.
    fn forget_old(&mut self) {
        while self.entries.len() > IN_MEMORY {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| matches!(e.fetch, Fetch::Idle | Fetch::Failed))
                .min_by_key(|(_, e)| e.asked)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.entries.remove(&id);
                }
                None => break,
            }
        }
    }
}

/// `details` as their file keeps them: an image address with a query is
/// emptied, since `redact` sees no token there. Memory keeps it, so the page
/// shows the image.
fn without_query_images(mut details: VodDetails) -> VodDetails {
    let clear = |url: &mut String| {
        if url.contains('?') {
            url.clear();
        }
    };
    clear(&mut details.backdrop);
    for s in &mut details.seasons {
        clear(&mut s.cover);
    }
    for e in &mut details.episodes {
        clear(&mut e.still);
    }
    details
}

/// Writes `file` to `path` through a temporary file (`write_atomic_with`); a
/// failure is logged.
fn save(path: &Path, file: &DetailFile, id: TitleId) {
    let written = write_atomic_with(path, |out| {
        serde_json::to_writer(out, file).map_err(std::io::Error::other)
    });
    if let Err(e) = written {
        tracing::warn!("vod: details of {} not kept ({e})", log_name(id));
    }
}
