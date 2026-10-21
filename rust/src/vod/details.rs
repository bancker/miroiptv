//! A film's and a series' details (spec 2026-10-15 §4.3), asked of the portal
//! when their page opens: on the tokio runtime, at most 2 at a time, the
//! newest request first. Each answer is kept as a file with only what the
//! pages show (`config\cache\vod\<host>_<port>\<s|m><id>.json`, a
//! `VodDetails`), and without an image address that has a query: `redact`
//! sees no token there (memory keeps it, so the page shows it), nor a trailer
//! that `redact` would change (a stream address with an account in it). A
//! series' file is asked again once it is older than 24 hours, a film's after
//! 30 days; the old file shows meanwhile, and so do details held in memory.
//! Opening a page reads its file in the same call, so it shows in the frame
//! that opens it.
//!
//! An answer that holds nothing a page shows is no answer ([`holds_nothing`]):
//! it is not kept, a first fetch ends as `Failed` (the page offers Enter), a
//! refresh keeps the old details, and a file of that kind from an older build
//! is not shown either.
//!
//! A refresh that fails keeps the old details and rests ([`REFRESH_REST_SECS`]):
//! the page may open again and again, and the portal is not asked for them
//! meanwhile.
//!
//! The log names a title as `<s|m><id>` and never an address, an account or a
//! folder (the folder holds the Windows user name); an error is logged in its
//! `Display` form, whose `Debug` form can hold the portal's IP and port.

use super::cache::portal_key;
use super::{write_atomic_with, Kind, TitleId};
use crate::args::XtreamCreds;
use crate::portal::{EpisodeInfo, Portal, SeasonInfo, VodDetails};
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

/// A refresh that failed, or came back with nothing, is not asked again for
/// this long, however often the page opens meanwhile (the old details keep
/// showing). The portal throttles bursts, and a page that asks on every frame
/// would otherwise ask again the moment each failure came in. A title with no
/// details at all is not asked again until Enter (`retry`), as before.
pub const REFRESH_REST_SECS: i64 = 10 * 60;

const VERSION: u32 = 1;

/// Every write of a details file goes through the same temporary file
/// (`write_atomic_with`: `<name>.json.tmp`), so two writers of one title at
/// once would write into each other and leave a torn file. A store never has
/// two (a title is asked once at a time, and its place is held until its file
/// is written), but the store of a source that was edited or switched away from
/// leaves its requests running while a new store over the same folder starts
/// its own. Held by `save` alone, which runs on the blocking pool: no frame
/// waits for it.
static SAVING: Mutex<()> = Mutex::new(());

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
    /// The portal gave none (or an answer with nothing a page shows) and no
    /// file had them: "Details konden niet geladen worden. Enter probeert
    /// opnieuw" (`DetailStore::retry`).
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
    /// When the portal gave `details`, unix seconds: the file's own
    /// `fetched_at`, or the time the request that fetched them started.
    fetched_at: i64,
    /// No refresh of these details before this unix time: the last one failed
    /// ([`REFRESH_REST_SECS`]).
    rest_until: i64,
    fetch: Fetch,
    /// When its page last asked, as a count: the least recent leave memory.
    asked: u64,
}

/// What came back from the portal.
struct Arrived {
    id: TitleId,
    answer: Result<VodDetails, String>,
    /// When the request started, unix seconds.
    fetched_at: i64,
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

/// Whether `details` hold nothing a page shows: a series with no info, seasons
/// or episodes (what a portal gives for a series it knows nothing of), a film
/// with no info. Every field of [`VodDetails`] counts but one: the TMDB id is
/// the key of a later supplement and no page shows it. A season or an episode
/// with nothing in it is no tab and no row. The pattern names every field, so
/// a field added to `VodDetails` does not build until someone has said whether
/// a page shows it. A series with info and no episodes yet (an announced one)
/// is a page.
fn holds_nothing(details: &VodDetails) -> bool {
    let VodDetails {
        plot,
        cast,
        director,
        genre,
        released,
        year,
        age,
        duration,
        rating,
        trailer,
        backdrop,
        tmdb: _,
        width,
        height,
        hdr,
        channels,
        seasons,
        episodes,
    } = details;
    plot.is_empty()
        && cast.is_empty()
        && director.is_empty()
        && genre.is_empty()
        && released.is_empty()
        && *year == 0
        && age.is_empty()
        && *duration == 0
        && *rating == 0
        && trailer.is_empty()
        && backdrop.is_empty()
        && *width == 0
        && *height == 0
        && !*hdr
        && *channels == 0
        && seasons.iter().all(|s| *s == SeasonInfo::default())
        && episodes.iter().all(|e| *e == EpisodeInfo::default())
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
    /// portal is asked when there are no details or they are too old, those in
    /// memory as well as those of a file. A request that waits becomes the
    /// newest. Details whose refresh failed are not asked for again until the
    /// rest is over ([`REFRESH_REST_SECS`]).
    pub fn request(&mut self, id: TitleId, now_unix: i64) {
        self.asks += 1;
        let asked = self.asks;
        let file = match self.entries.get(&id) {
            Some(e) if e.details.is_some() || e.fetch != Fetch::Idle => None,
            _ => Some(self.read_file(id)),
        };
        let entry = self.entries.entry(id).or_insert(Entry {
            details: None,
            fetched_at: 0,
            rest_until: 0,
            fetch: Fetch::Idle,
            asked,
        });
        entry.asked = asked;
        if let Some(Some((details, fetched_at))) = file {
            entry.details = Some(Arc::new(details));
            entry.fetched_at = fetched_at;
        }
        // A time from the future (a clock that was wrong) counts as now.
        let age = now_unix.saturating_sub(entry.fetched_at.min(now_unix));
        let stale =
            entry.details.is_none() || (age > fresh_secs(id) && now_unix >= entry.rest_until);
        if stale && matches!(entry.fetch, Fetch::Idle | Fetch::Waiting) {
            self.wait(id);
        }
        self.forget_old();
        self.start_waiting(now_unix);
    }

    /// Enter on a page whose details failed: asks again. For a fresh key press,
    /// not for a key repeat or a frame: it asks as often as it is called, as
    /// soon as the last answer is in.
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
        for Arrived {
            id,
            answer,
            fetched_at,
        } in arrived
        {
            self.in_flight = self.in_flight.saturating_sub(1);
            let Some(e) = self.entries.get_mut(&id) else {
                continue;
            };
            match answer {
                Ok(details) => {
                    e.details = Some(Arc::new(details));
                    e.fetched_at = fetched_at;
                    e.fetch = Fetch::Idle;
                }
                // An old file keeps showing, and rests; without one the page says so.
                Err(_) if e.details.is_some() => {
                    e.fetch = Fetch::Idle;
                    e.rest_until = now_unix.saturating_add(REFRESH_REST_SECS);
                }
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
                // Nothing a page shows: no answer, and no file of it.
                Ok(details) if holds_nothing(&details) => {
                    tracing::warn!(
                        "vod: details of {} not fetched (the answer holds no details)",
                        log_name(id)
                    );
                    Err("the answer holds no details".to_owned())
                }
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
                        // The write runs on the blocking pool, its failure is
                        // logged here: an io error names no folder.
                        let kept = tokio::task::spawn_blocking(move || save(&path, &file)).await;
                        if let Ok(Err(e)) = kept {
                            tracing::warn!("vod: details of {} not kept ({e})", log_name(id));
                        }
                    }
                    Ok(details)
                }
                Err(e) => {
                    tracing::warn!("vod: details of {} not fetched ({e})", log_name(id));
                    Err(e.to_string())
                }
            };
            done.lock().push(Arrived {
                id,
                answer,
                fetched_at: now_unix,
            });
        });
    }

    /// `id`'s file and when it was fetched; none when there is none, or it is
    /// no details file of this version, or it holds no details (an older build
    /// kept the answer of a portal that had none): it is asked again and
    /// written anew.
    fn read_file(&self, id: TitleId) -> Option<(VodDetails, i64)> {
        let path = self.dir.as_ref()?.join(file_name(id));
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice::<DetailFile>(&bytes) {
            Ok(file) if file.version == VERSION && !holds_nothing(&file.details) => {
                Some((file.details, file.fetched_at))
            }
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
/// shows the image. A trailer is emptied too when `redact` would change it: it
/// is the YouTube code as the portal gave it, and a stream address with the
/// account in it (`/movie/<user>/<pass>/…`, `user:pass@`) is no code.
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
    if crate::redact::redact(&details.trailer) != details.trailer.as_str() {
        details.trailer.clear();
    }
    details
}

/// Writes `file` to `path` through a temporary file (`write_atomic_with`), one
/// write at a time in the process ([`SAVING`]).
fn save(path: &Path, file: &DetailFile) -> std::io::Result<()> {
    let _one_at_a_time = SAVING.lock();
    write_atomic_with(path, |out| {
        serde_json::to_writer(out, file).map_err(std::io::Error::other)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default details with one change.
    fn with(change: impl FnOnce(&mut VodDetails)) -> VodDetails {
        let mut details = VodDetails::default();
        change(&mut details);
        details
    }

    /// What a page shows is every field of the details but the TMDB id; each
    /// one alone makes a page.
    #[test]
    fn each_field_a_page_shows_makes_a_page_and_the_tmdb_id_alone_does_not() {
        assert!(holds_nothing(&VodDetails::default()));
        let tmdb = with(|d| d.tmdb = 603);
        assert!(holds_nothing(&tmdb), "the TMDB id is on no page");
        let season = |change: fn(&mut SeasonInfo)| {
            let mut season = SeasonInfo::default();
            change(&mut season);
            vec![season]
        };
        let pages = [
            ("plot", with(|d| d.plot = "x".into())),
            ("cast", with(|d| d.cast = "x".into())),
            ("director", with(|d| d.director = "x".into())),
            ("genre", with(|d| d.genre = "x".into())),
            ("released", with(|d| d.released = "x".into())),
            ("year", with(|d| d.year = 1999)),
            ("age", with(|d| d.age = "16".into())),
            ("duration", with(|d| d.duration = 60)),
            ("rating", with(|d| d.rating = 1)),
            ("trailer", with(|d| d.trailer = "x".into())),
            ("backdrop", with(|d| d.backdrop = "x".into())),
            ("width", with(|d| d.width = 1)),
            ("height", with(|d| d.height = 1)),
            ("hdr", with(|d| d.hdr = true)),
            ("channels", with(|d| d.channels = 2)),
            (
                "a season by its number",
                with(|d| d.seasons = season(|s| s.number = 1)),
            ),
            (
                "a season by its name",
                with(|d| d.seasons = season(|s| s.name = "x".into())),
            ),
            (
                "a season by its cover",
                with(|d| d.seasons = season(|s| s.cover = "x".into())),
            ),
            (
                "an episode",
                with(|d| {
                    d.episodes = vec![EpisodeInfo {
                        id: "7".into(),
                        ..EpisodeInfo::default()
                    }]
                }),
            ),
        ];
        for (what, details) in pages {
            assert!(!holds_nothing(&details), "{what} is something a page shows");
        }
        // A season or an episode with nothing in it is no tab and no row.
        let hollow = with(|d| {
            d.seasons = vec![SeasonInfo::default()];
            d.episodes = vec![EpisodeInfo::default()];
        });
        assert!(holds_nothing(&hollow));
    }

    /// The stores of a source that was edited share its folder, and the requests
    /// of the old one run on: two writers of one title at once must not write
    /// into each other's temporary file (`<name>.json.tmp`).
    #[test]
    fn saves_of_one_file_at_once_all_succeed_and_leave_it_whole() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let id = TitleId {
            kind: Kind::Series,
            id: 1,
        };
        let path = dir.path().join(file_name(id));
        let file = |episodes: usize| DetailFile {
            version: VERSION,
            fetched_at: 1,
            details: VodDetails {
                plot: "A plot.".into(),
                episodes: (0..episodes)
                    .map(|i| EpisodeInfo {
                        id: i.to_string(),
                        plot: "y".repeat(200),
                        ..EpisodeInfo::default()
                    })
                    .collect(),
                ..VodDetails::default()
            },
        };
        let failed = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for episodes in [400, 1500, 400, 1500] {
                let (path, failed, file) = (&path, &failed, &file);
                scope.spawn(move || {
                    let file = file(episodes);
                    for _ in 0..25 {
                        if save(path, &file).is_err() {
                            failed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        assert_eq!(
            failed.load(Ordering::SeqCst),
            0,
            "saves failed: the writers wrote into each other's temporary file"
        );
        let bytes = std::fs::read(&path).unwrap();
        let read: DetailFile = serde_json::from_slice(&bytes).expect("the file is whole");
        let n = read.details.episodes.len();
        assert!(n == 400 || n == 1500, "{n} episodes: a mix of two files");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
