//! An M3U list as a portal: the channels of one list, played straight from
//! their servers (the relay is the Xtream portal's). No films, series, EPG or
//! catch-up in round 1.
//!
//! A list at a URL is kept in a file once it came (`Storage::m3u_cache_path`):
//! the next start shows that saved list at once (`saved_catalog`) while the
//! fresh one downloads (`fetch_catalog`). A list in a file is read as it is.
//! Reading happens on a blocking thread: 11,000 channels must not hold up a
//! tokio worker, where the relay runs.

use super::{Catalog, Episode, Portal, PortalError};
use crate::epg::Epg;
use crate::m3u::{self, Playlist, Stream};
use crate::sources::{SourceId, SourceKind};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the download of a list may take: iptv-org's is 2.55 MB.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a list comes from. No `Debug`: the address is a credential.
#[derive(Clone, PartialEq, Eq)]
enum Address {
    Url(String),
    File(PathBuf),
}

/// What the catalog's stream ids play, and what round 2 will want. No `Debug`:
/// the stream URLs of a paid list hold credentials.
#[derive(Default)]
struct Lookup {
    streams: HashMap<i64, Stream>,
    epg_url: Option<String>,
}

pub struct M3uPortal {
    id: SourceId,
    address: Address,
    /// Where the list of a URL is kept once it came.
    cache: PathBuf,
    client: reqwest::Client,
    lookup: RwLock<Arc<Lookup>>,
    /// The saved list was read and is what shows: a download that is the same
    /// brings nothing new.
    saved_shown: AtomicBool,
}

impl M3uPortal {
    /// The portal of M3U source `id`. `address` is an http(s) URL or a file;
    /// the list of a URL is kept at `cache`.
    pub fn new(id: SourceId, address: &str, cache: PathBuf) -> Self {
        let address = if crate::sources::is_web_address(address) {
            Address::Url(address.trim().to_owned())
        } else {
            Address::File(PathBuf::from(address.trim()))
        };
        let client = reqwest::Client::builder()
            .user_agent(format!("{}/{}", crate::APP_ID, env!("CARGO_PKG_VERSION")))
            .timeout(DOWNLOAD_TIMEOUT)
            .build()
            .expect("build reqwest client");
        Self {
            id,
            address,
            cache,
            client,
            lookup: RwLock::new(Arc::new(Lookup::default())),
            saved_shown: AtomicBool::new(false),
        }
    }

    /// A portal whose list is `text`, read already, and the catalog it gives;
    /// `None` when `text` is no list. The app's tests start M3U sources so.
    pub fn from_text(text: &str) -> Option<(Self, Catalog)> {
        let portal = Self::new(0, "", PathBuf::new());
        let catalog = portal.install(m3u::read(text)?);
        Some((portal, catalog))
    }

    /// The logo (`tvg-logo`) of a stream id, kept for round 2.
    pub fn logo(&self, stream_id: i64) -> Option<String> {
        self.lookup.read().streams.get(&stream_id)?.logo.clone()
    }

    /// The list's XMLTV guide (`x-tvg-url`), kept for round 2.
    pub fn epg_url(&self) -> Option<String> {
        self.lookup.read().epg_url.clone()
    }

    /// Makes `playlist` what the stream ids play; its channels for the catalog.
    fn install(&self, playlist: Playlist) -> Catalog {
        let Playlist {
            live,
            streams,
            epg_url,
        } = playlist;
        *self.lookup.write() = Arc::new(Lookup { streams, epg_url });
        Catalog {
            live,
            ..Catalog::default()
        }
    }

    /// The list at `url`, as bytes. A refusal is its status; any other failure
    /// is reqwest's without the URL, which may hold a password.
    async fn download(&self, url: &str) -> Result<Vec<u8>, PortalError> {
        let answer = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| PortalError::Http(e.without_url()))?;
        if !answer.status().is_success() {
            return Err(PortalError::Status(answer.status().as_u16()));
        }
        let body = answer
            .bytes()
            .await
            .map_err(|e| PortalError::Http(e.without_url()))?;
        Ok(body.to_vec())
    }
}

/// A downloaded list: `Unchanged` when it is the saved one and that one shows
/// (`check_saved`), else read and, once read, kept at `cache`. A list that
/// cannot be kept still plays; the next start then has none to show at once.
fn read_download(
    bytes: Vec<u8>,
    cache: &Path,
    check_saved: bool,
) -> Result<(Playlist, usize), PortalError> {
    if check_saved && std::fs::read(cache).is_ok_and(|saved| saved == bytes) {
        return Err(PortalError::Unchanged);
    }
    let playlist = m3u::read(&String::from_utf8_lossy(&bytes)).ok_or(PortalError::NotAList)?;
    if let Err(e) = write_atomic(cache, &bytes) {
        tracing::warn!("m3u: list not saved: {}", e);
    }
    Ok((playlist, bytes.len()))
}

/// A list in a file. The error names the file. A web address with a mistyped
/// scheme (`htps://user:pass@…`) is taken for a file too, so the text goes
/// through `redact`.
fn read_file(path: &Path) -> Result<(Playlist, usize), PortalError> {
    let bytes = std::fs::read(path).map_err(|e| {
        let said = format!("{}: {}", path.display(), e);
        PortalError::File(crate::redact::redact(&said).into_owned())
    })?;
    let playlist = m3u::read(&String::from_utf8_lossy(&bytes)).ok_or(PortalError::NotAList)?;
    Ok((playlist, bytes.len()))
}

/// `bytes` to `path` through a temp file beside it, so a write that fails
/// leaves the old list whole.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("m3u.tmp");
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// What a blocking reader that panicked or was cancelled leaves.
fn reader_lost(e: tokio::task::JoinError) -> PortalError {
    PortalError::Shape(format!("list reader: {e}"))
}

#[async_trait::async_trait]
impl Portal for M3uPortal {
    fn kind(&self) -> SourceKind {
        SourceKind::M3u
    }

    async fn saved_catalog(&self) -> Option<Catalog> {
        if !matches!(self.address, Address::Url(_)) {
            return None;
        }
        let t0 = Instant::now();
        let path = self.cache.clone();
        let (playlist, size) = tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(path).ok()?;
            let playlist = m3u::read(&String::from_utf8_lossy(&bytes))?;
            Some((playlist, bytes.len()))
        })
        .await
        .ok()??;
        let channels = playlist.live.len();
        let catalog = self.install(playlist);
        self.saved_shown.store(true, Ordering::Release);
        tracing::info!(
            "m3u: saved list of source {} ({} channels, {} KB) in {} ms",
            self.id,
            channels,
            size / 1024,
            t0.elapsed().as_millis()
        );
        Some(catalog)
    }

    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        let t0 = Instant::now();
        let read = match &self.address {
            Address::Url(url) => match self.download(url).await {
                Ok(bytes) => {
                    let cache = self.cache.clone();
                    let check_saved = self.saved_shown.load(Ordering::Acquire);
                    tokio::task::spawn_blocking(move || read_download(bytes, &cache, check_saved))
                        .await
                        .unwrap_or_else(|e| Err(reader_lost(e)))
                }
                Err(e) => Err(e),
            },
            Address::File(path) => {
                let path = path.clone();
                tokio::task::spawn_blocking(move || read_file(&path))
                    .await
                    .unwrap_or_else(|e| Err(reader_lost(e)))
            }
        };
        match read {
            Ok((playlist, size)) => {
                let channels = playlist.live.len();
                let catalog = self.install(playlist);
                tracing::info!(
                    "m3u: list of source {} read ({} channels, {} KB) in {} ms",
                    self.id,
                    channels,
                    size / 1024,
                    t0.elapsed().as_millis()
                );
                Ok(catalog)
            }
            Err(PortalError::Unchanged) => {
                tracing::info!("m3u: list of source {} unchanged", self.id);
                Err(PortalError::Unchanged)
            }
            Err(e) => {
                tracing::warn!("m3u: list of source {} not read: {}", self.id, e);
                Err(e)
            }
        }
    }

    async fn fetch_epg(&self, _stream_id: i64) -> Result<Epg, PortalError> {
        Ok(Epg::new(Vec::new()))
    }

    async fn fetch_day_epg(&self, _stream_id: i64) -> Result<Epg, PortalError> {
        Ok(Epg::new(Vec::new()))
    }

    async fn fetch_series_episodes(&self, _series_id: i64) -> Result<Vec<Episode>, PortalError> {
        Ok(Vec::new())
    }

    fn live_stream_url(&self, stream_id: i64) -> String {
        self.lookup
            .read()
            .streams
            .get(&stream_id)
            .map(|s| s.url.clone())
            .unwrap_or_default()
    }

    fn live_stream_id(&self, url: &str) -> Option<i64> {
        // A list may put one address under two names: the lowest id answers, so
        // the answer is the same from one start to the next.
        self.lookup
            .read()
            .streams
            .iter()
            .filter(|(_, s)| s.url == url)
            .map(|(id, _)| *id)
            .min()
    }

    fn movie_stream_url(&self, _stream_id: i64, _container_ext: &str) -> String {
        String::new()
    }

    fn series_stream_url(&self, _episode_id: &str, _container_ext: &str) -> String {
        String::new()
    }

    fn catchup_url(
        &self,
        _stream_id: i64,
        _start: chrono::DateTime<chrono::Utc>,
        _duration_min: u32,
    ) -> String {
        String::new()
    }
}
