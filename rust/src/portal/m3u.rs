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
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the download of a list may take: iptv-org's is 2.55 MB.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// The most a list may hold, downloaded or in a file: a big paid list is some
/// tens of MB. More is no channel list (a video picked by mistake), and to read
/// it whole could take more memory than there is, which aborts the release
/// build at every start. A saved list is never bigger: only a list that was
/// read is kept.
const MAX_LIST_BYTES: u64 = 128 * 1024 * 1024;

/// A NUL byte this far into what came says it is no text (a video, an
/// archive), so no list: a list holds none.
const NUL_SCAN_BYTES: usize = 1024;

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
    /// The saved list is what shows: it was read at the start, or the list that
    /// shows was saved. A download that is the same brings nothing new. A list
    /// that could not be saved clears it: the file then holds an older one.
    saved_shown: AtomicBool,
    /// More than this is no list ([`MAX_LIST_BYTES`]; a test makes it small).
    max_bytes: u64,
    /// Set by `Portal::retire`: a download that is still under way then saves
    /// nothing. A list is saved while this is held, so a retire waits for a
    /// save that has begun, and no save begins after it.
    retired: Arc<Mutex<bool>>,
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
            max_bytes: MAX_LIST_BYTES,
            retired: Arc::new(Mutex::new(false)),
        }
    }

    /// This portal with a cap of `max_bytes`, so that a test needs no 128 MB.
    #[cfg(test)]
    fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
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

    /// The list at `url`, as bytes. A refusal is its status. More than
    /// `max_bytes` is no list: a Content-Length that says so is enough, and no
    /// more of the body is read than the cap. Any other failure is reqwest's
    /// without the URL, which may hold a password.
    async fn download(&self, url: &str) -> Result<Vec<u8>, PortalError> {
        let mut answer = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| PortalError::Http(e.without_url()))?;
        if !answer.status().is_success() {
            return Err(PortalError::Status(answer.status().as_u16()));
        }
        let max = self.max_bytes;
        if answer.content_length().is_some_and(|n| n > max) {
            return Err(too_big(self.id, max));
        }
        // Room at once for what the server says comes (within the cap, as
        // checked): no copy on the way.
        let mut body = Vec::with_capacity(answer.content_length().unwrap_or(0) as usize);
        while let Some(chunk) = answer
            .chunk()
            .await
            .map_err(|e| PortalError::Http(e.without_url()))?
        {
            if !append_within(&mut body, &chunk, max as usize) {
                return Err(too_big(self.id, max));
            }
        }
        Ok(body)
    }
}

/// What a list bigger than `max` bytes is, with a word in the log first.
fn too_big(id: SourceId, max: u64) -> PortalError {
    tracing::warn!(
        "m3u: list of source {} is over {} KB - no channel list",
        id,
        max / 1024
    );
    PortalError::NotAList
}

/// `chunk` after `body`, unless that would make it longer than `max`: then
/// false, and `body` is as it was. The room it takes grows as a vector's does,
/// but never past `max`.
fn append_within(body: &mut Vec<u8>, chunk: &[u8], max: usize) -> bool {
    let len = body.len() + chunk.len();
    if len > max {
        return false;
    }
    if len > body.capacity() {
        let room = len.max(body.capacity().saturating_mul(2)).min(max);
        body.reserve_exact(room - body.len());
    }
    body.extend_from_slice(chunk);
    true
}

/// The bytes of the file at `path`, or `None` when it holds more than `max`.
/// Such a file is not read, and no more than `max` of one that grows while it
/// is read.
fn read_at_most(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > max {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(max + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

/// The list in `bytes`, if they are one. A NUL byte in the first KB says they
/// are no text (a video, an archive), and they are read no further.
fn read_list(bytes: &[u8]) -> Option<Playlist> {
    if bytes[..bytes.len().min(NUL_SCAN_BYTES)].contains(&0) {
        return None;
    }
    m3u::read(&String::from_utf8_lossy(bytes))
}

/// A downloaded list of source `id`: `Unchanged` when it is the saved one and
/// that one shows (`check_saved`), else read and, once read, kept at `cache`
/// unless the source is `retired`; whether it was. A list that cannot be kept
/// still plays; the next start then has none to show at once.
fn read_download(
    bytes: &[u8],
    id: SourceId,
    cache: &Path,
    retired: &Mutex<bool>,
    check_saved: bool,
) -> Result<(Playlist, usize, bool), PortalError> {
    if check_saved && std::fs::read(cache).is_ok_and(|saved| saved == bytes) {
        return Err(PortalError::Unchanged);
    }
    let playlist = read_list(bytes).ok_or(PortalError::NotAList)?;
    // Held while the list is written: a retire waits for that write, and once
    // it has come no other write begins (the app may drop the file then).
    let retired = retired.lock();
    if *retired {
        tracing::info!(
            "m3u: list of source {} not saved - the source is no longer in use",
            id
        );
        return Ok((playlist, bytes.len(), false));
    }
    let saved = match write_atomic(cache, bytes) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("m3u: list not saved: {}", e);
            false
        }
    };
    drop(retired);
    Ok((playlist, bytes.len(), saved))
}

/// A list in a file of source `id`, `max` bytes at most. The error names the
/// file. A web address with a mistyped scheme (`htps://user:pass@…`) is taken
/// for a file too, so the text goes through `redact`.
fn read_file(path: &Path, max: u64, id: SourceId) -> Result<(Playlist, usize), PortalError> {
    let bytes = read_at_most(path, max)
        .map_err(|e| {
            let said = format!("{}: {}", path.display(), e);
            PortalError::File(crate::redact::redact(&said).into_owned())
        })?
        .ok_or_else(|| too_big(id, max))?;
    let playlist = read_list(&bytes).ok_or(PortalError::NotAList)?;
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

    fn retire(&self) {
        *self.retired.lock() = true;
    }

    async fn saved_catalog(&self) -> Option<Catalog> {
        if !matches!(self.address, Address::Url(_)) {
            return None;
        }
        let t0 = Instant::now();
        let (path, max) = (self.cache.clone(), self.max_bytes);
        let (playlist, size) = tokio::task::spawn_blocking(move || {
            let bytes = read_at_most(&path, max).ok()??;
            let playlist = read_list(&bytes)?;
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
                    let (id, cache, retired) = (self.id, self.cache.clone(), self.retired.clone());
                    let check_saved = self.saved_shown.load(Ordering::Acquire);
                    tokio::task::spawn_blocking(move || {
                        read_download(&bytes, id, &cache, &retired, check_saved)
                    })
                    .await
                    .unwrap_or_else(|e| Err(reader_lost(e)))
                }
                Err(e) => Err(e),
            },
            Address::File(path) => {
                let (path, max, id) = (path.clone(), self.max_bytes, self.id);
                // A list in a file is not saved: there is no copy to be in step.
                tokio::task::spawn_blocking(move || read_file(&path, max, id))
                    .await
                    .unwrap_or_else(|e| Err(reader_lost(e)))
                    .map(|(playlist, size)| (playlist, size, false))
            }
        };
        match read {
            Ok((playlist, size, saved)) => {
                let channels = playlist.live.len();
                let catalog = self.install(playlist);
                // The saved file holds what shows now only if it was saved.
                self.saved_shown.store(saved, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The cap the tests give a portal: 1 KB.
    const SMALL: u64 = 1024;

    /// A list of `n` channels, about 55 bytes each.
    fn list_of(n: usize) -> String {
        let mut text = String::from("#EXTM3U\n");
        for i in 0..n {
            text.push_str(&format!(
                "#EXTINF:-1,Kanaal {i}\nhttps://a.example/{i}.m3u8\n"
            ));
        }
        text
    }

    /// A server that answers every GET with `body`, with a Content-Length or,
    /// `chunked`, without one.
    async fn serving(body: &str, chunked: bool) -> MockServer {
        let mut answer = ResponseTemplate::new(200).set_body_string(body);
        if chunked {
            answer = answer.insert_header("transfer-encoding", "chunked");
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(answer)
            .mount(&server)
            .await;
        server
    }

    /// What a read gave, to print: the channels, or the error.
    fn outcome(read: Result<Catalog, PortalError>) -> Result<usize, PortalError> {
        read.map(|c| c.live.len())
    }

    #[test]
    fn the_cap_is_128_mb() {
        assert_eq!(MAX_LIST_BYTES, 128 * 1024 * 1024);
        let portal = M3uPortal::new(2, "https://a.example/tv.m3u", PathBuf::new());
        assert_eq!(portal.max_bytes, MAX_LIST_BYTES);
    }

    #[tokio::test]
    async fn a_download_over_the_cap_is_no_list_with_a_content_length_or_without() {
        let big = list_of(40);
        assert!(big.len() as u64 > SMALL);
        for chunked in [false, true] {
            let server = serving(&big, chunked).await;
            let url = format!("{}/tv.m3u", server.uri());
            // The server's own answer: with a Content-Length, or without.
            let sent = reqwest::get(&url).await.unwrap().content_length();
            assert_eq!(sent.is_none(), chunked, "{sent:?}");
            let dir = tempfile::tempdir().unwrap();
            let cache = dir.path().join("m3u-2.m3u");
            let portal = M3uPortal::new(2, &url, cache.clone()).with_max_bytes(SMALL);
            let read = outcome(portal.fetch_catalog().await);
            assert!(
                matches!(read, Err(PortalError::NotAList)),
                "chunked {chunked}: {read:?}"
            );
            assert!(!cache.exists(), "nothing is saved");
        }
    }

    #[tokio::test]
    async fn a_download_within_the_cap_reads_as_before() {
        let fits = list_of(5);
        assert!(fits.len() as u64 <= SMALL);
        for chunked in [false, true] {
            let server = serving(&fits, chunked).await;
            let dir = tempfile::tempdir().unwrap();
            let url = format!("{}/tv.m3u", server.uri());
            let portal = M3uPortal::new(2, &url, dir.path().join("m3u-2.m3u"));
            let read = outcome(portal.with_max_bytes(SMALL).fetch_catalog().await);
            assert!(matches!(read, Ok(5)), "chunked {chunked}: {read:?}");
        }
    }

    #[tokio::test]
    async fn a_file_over_the_cap_is_no_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("groot.m3u");
        // What is in it would be a list: only its length can refuse it.
        std::fs::write(&file, list_of(40)).unwrap();
        let portal = M3uPortal::new(3, &file.display().to_string(), PathBuf::new());
        let read = outcome(portal.with_max_bytes(SMALL).fetch_catalog().await);
        assert!(matches!(read, Err(PortalError::NotAList)), "{read:?}");
    }

    #[test]
    fn the_length_of_a_file_decides_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, vec![b'x'; 2048]).unwrap();
        assert!(
            read_at_most(&file, 2047).unwrap().is_none(),
            "one byte over"
        );
        let read = read_at_most(&file, 2048).unwrap();
        assert_eq!(read.map(|b| b.len()), Some(2048), "the cap itself fits");
        assert!(read_at_most(&dir.path().join("weg.m3u"), 2048).is_err());
    }

    #[test]
    fn a_body_grows_up_to_the_cap_and_no_further() {
        let mut body = Vec::new();
        assert!(append_within(&mut body, &[1; 600], 1024));
        assert!(append_within(&mut body, &[2; 424], 1024));
        assert_eq!(body.len(), 1024);
        assert!(body.capacity() <= 1024, "room taken: {}", body.capacity());
        assert!(!append_within(&mut body, &[3], 1024));
        assert_eq!(body.len(), 1024, "as it was");
    }

    #[test]
    fn a_nul_is_looked_for_in_the_first_kb_only() {
        let list = list_of(30);
        assert!(list.len() > NUL_SCAN_BYTES);
        let mut late = list.clone().into_bytes();
        late.push(0);
        assert!(read_list(&late).is_some(), "a NUL after the first KB");
        let mut early = list.into_bytes();
        early[NUL_SCAN_BYTES - 1] = 0;
        assert!(read_list(&early).is_none(), "a NUL in the first KB");
    }

    #[tokio::test]
    async fn data_with_a_nul_in_its_first_kb_is_no_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.m3u");
        // A video's first bytes, then what reads as a channel.
        let mut bytes = b"#EXTM3U\n\0\0\0\x18ftypmp42\n".to_vec();
        bytes.extend_from_slice(b"#EXTINF:-1,A\nhttps://a.example/1.m3u8\n");
        std::fs::write(&file, &bytes).unwrap();
        let portal = M3uPortal::new(3, &file.display().to_string(), PathBuf::new());
        let read = outcome(portal.fetch_catalog().await);
        assert!(matches!(read, Err(PortalError::NotAList)), "{read:?}");
    }
}
