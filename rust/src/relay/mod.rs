//! Local relay that turns a portal's live HLS stream into one continuous
//! MPEG-TS stream for mpv.
//!
//! Played directly, the portal's HLS dies on the first refused playlist
//! refresh (HTTP 509 -> libav gives up -> eof -> frozen picture). Through the
//! relay mpv plays `http://127.0.0.1:<port>/live/<id>.ts`: segments are fetched
//! here strictly in media-sequence order and written to that one never-ending
//! response. A refusal is retried (same URL, then a fresh token via the
//! portal) while mpv keeps playing from the cushion, so it never sees an
//! error. Bonus: the portal is opened once instead of mpv's twice, and the
//! credentials stay out of mpv's URLs and logs.

pub mod playlist;

use bytes::Bytes;
use parking_lot::Mutex;
use playlist::{Playlist, Segment};
use reqwest::Url;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Chunks queued between fetching and the player: ~64 MB at reqwest's usual
/// 16 KB, room for the whole cushion even on UHD. Bounded, so a paused player
/// also pauses the fetching instead of growing memory.
const CHANNEL_CHUNKS: usize = 4096;

/// Below this many seconds of media ahead of playback the relay logs a
/// warning: a slow segment could then show as a hiccup.
const LOW_CUSHION_S: f64 = 5.0;

/// Tuning knobs. `Default` is what the app uses; tests shrink the timings.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Segments behind the live edge a session starts at: its cushion. The
    /// next segment can arrive up to a segment plus a poll interval later, so
    /// 3 keeps ~14 s in hand where 2 could dip to ~4 s.
    pub start_behind: usize,
    /// Floor for the playlist poll interval (normally half a target duration).
    pub poll_min: Duration,
    /// Pauses between retries of a refused playlist or segment request.
    pub retry_delays: Vec<Duration>,
    /// Without a usable playlist for this long the stream ends, so the app's
    /// own recovery takes over.
    pub give_up_after: Duration,
    /// A request or download that stalls this long counts as failed.
    pub idle_timeout: Duration,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            start_behind: 3,
            poll_min: Duration::from_secs(1),
            retry_delays: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
            ],
            give_up_after: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(10),
        }
    }
}

/// Handle to the running relay; cheap to clone.
#[derive(Clone)]
pub struct Relay {
    inner: Arc<Inner>,
}

struct Inner {
    port: u16,
    cfg: RelayConfig,
    client: reqwest::Client,
    routes: Mutex<Routes>,
}

/// Local stream ids <-> upstream playlist URLs. Grows by one entry per
/// distinct channel watched, so it stays tiny.
#[derive(Default)]
struct Routes {
    ids: HashMap<String, u64>,
    urls: HashMap<u64, String>,
}

impl Relay {
    /// Bind a free loopback port and serve on the current tokio runtime.
    /// Loopback only: no firewall prompt, nothing reachable from outside.
    pub fn start(cfg: RelayConfig) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        let port = listener.local_addr()?.port();
        let client = reqwest::Client::builder()
            .user_agent(format!("{}/{}", crate::APP_ID, env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(std::io::Error::other)?;
        let inner = Arc::new(Inner {
            port,
            cfg,
            client,
            routes: Mutex::new(Routes::default()),
        });
        tokio::spawn(accept_loop(listener, inner.clone()));
        Ok(Self { inner })
    }

    /// Local URL for mpv that plays the live HLS playlist at `upstream`.
    pub fn local_url(&self, upstream: &str) -> String {
        let mut routes = self.inner.routes.lock();
        let next = routes.ids.len() as u64;
        let id = *routes.ids.entry(upstream.to_string()).or_insert(next);
        routes.urls.insert(id, upstream.to_string());
        format!("http://127.0.0.1:{}/live/{id}.ts", self.inner.port)
    }
}

async fn accept_loop(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        match listener.accept().await {
            Ok((conn, _)) => {
                tokio::spawn(serve(conn, inner.clone()));
            }
            Err(e) => {
                warn!("relay: accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn serve(mut conn: TcpStream, inner: Arc<Inner>) {
    let Some(path) = read_request_path(&mut conn).await else {
        return;
    };
    let upstream = path
        .strip_prefix("/live/")
        .and_then(|p| p.strip_suffix(".ts"))
        .and_then(|id| id.parse::<u64>().ok())
        .and_then(|id| inner.routes.lock().urls.get(&id).cloned());
    match upstream {
        Some(upstream) => session(&inner, &upstream, conn).await,
        None => {
            let _ = conn
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    }
}

/// Path of a `GET` request; headers (mpv's Range, Icy-MetaData...) are ignored.
async fn read_request_path(conn: &mut TcpStream) -> Option<String> {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > 8192 {
            return None;
        }
        let n = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut request_line = head.lines().next()?.split_whitespace();
    (request_line.next()? == "GET").then_some(())?;
    request_line.next().map(str::to_string)
}

/// One player connection: answer 200 once a first playlist is in, then copy
/// whatever the fetcher produces until the player hangs up (zap, quit) or the
/// fetcher gives up.
async fn session(inner: &Arc<Inner>, portal: &str, mut conn: TcpStream) {
    let t0 = Instant::now();
    let Some((edge, pl)) = first_playlist(inner, portal).await else {
        warn!("relay: no playlist from the portal - refusing the stream");
        let _ = conn
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return;
    };
    let ok = b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    if conn.write_all(ok).await.is_err() {
        return;
    }
    let (tx, mut rx) = mpsc::channel::<Bytes>(CHANNEL_CHUNKS);
    let fetcher = tokio::spawn(fetch_loop(inner.clone(), portal.to_string(), edge, pl, tx));
    let mut first = true;
    while let Some(chunk) = rx.recv().await {
        if first {
            info!(
                "relay: first bytes to the player after {} ms",
                t0.elapsed().as_millis()
            );
            first = false;
        }
        if conn.write_all(&chunk).await.is_err() {
            break; // player hung up
        }
    }
    fetcher.abort();
}

/// Keep fetching new segments in sequence order, refreshing the playlist in
/// between, until the player hangs up or the portal is gone for good.
async fn fetch_loop(
    inner: Arc<Inner>,
    portal: String,
    mut edge: Url,
    mut pl: Playlist,
    tx: mpsc::Sender<Bytes>,
) {
    let cfg = &inner.cfg;
    let Some(mut next) = pl.start_seq(cfg.start_behind) else {
        return;
    };
    info!(
        "relay: live session starts at segment {} ({} behind the edge)",
        next, cfg.start_behind
    );
    let mut last_good = Instant::now();
    // Media handed out vs. time since playback could start: the cushion.
    let started = Instant::now();
    let mut media_sent = 0.0;
    loop {
        let todo: Vec<Segment> = pl.from(next).to_vec();
        for seg in todo {
            if seg.seq > next {
                warn!(
                    "relay: fell behind the playlist - skipped {} segment(s)",
                    seg.seq - next
                );
            }
            let ahead = media_sent - started.elapsed().as_secs_f64();
            if media_sent > 0.0 && ahead < LOW_CUSHION_S {
                warn!(
                    "relay: cushion low ({:.1} s) before segment {}",
                    ahead, seg.seq
                );
            }
            if let Ok(url) = edge.join(&seg.uri) {
                match fetch_segment(&inner, url, &tx).await {
                    Sent::PlayerGone => return,
                    Sent::Failed => warn!("relay: segment {} failed - skipped", seg.seq),
                    Sent::Done => media_sent += seg.duration,
                }
            }
            next = seg.seq + 1;
        }
        if pl.ended || tx.is_closed() {
            return;
        }
        let poll = Duration::from_secs_f64(pl.target_duration.max(0.0) / 2.0).max(cfg.poll_min);
        tokio::time::sleep(poll).await;
        match reload(&inner, &portal, &mut edge).await {
            Some(p) => {
                pl = p;
                last_good = Instant::now();
            }
            None if last_good.elapsed() >= cfg.give_up_after => {
                warn!(
                    "relay: no playlist for {} s - ending the stream",
                    last_good.elapsed().as_secs()
                );
                return;
            }
            None => {}
        }
    }
}

/// The session's first playlist, via the portal (which redirects to an edge
/// server), retried like any refusal.
async fn first_playlist(inner: &Inner, portal: &str) -> Option<(Url, Playlist)> {
    let url = Url::parse(portal).ok()?;
    if let Some(found) = get_playlist(inner, url.clone()).await {
        return Some(found);
    }
    for delay in &inner.cfg.retry_delays {
        tokio::time::sleep(*delay).await;
        if let Some(found) = get_playlist(inner, url.clone()).await {
            return Some(found);
        }
    }
    None
}

/// Refresh on the edge URL we were redirected to; when refused, retry it,
/// then take a fresh token from the portal.
async fn reload(inner: &Inner, portal: &str, edge: &mut Url) -> Option<Playlist> {
    let t0 = Instant::now();
    if let Some((_, pl)) = get_playlist(inner, edge.clone()).await {
        return Some(pl);
    }
    for delay in &inner.cfg.retry_delays {
        tokio::time::sleep(*delay).await;
        if let Some((_, pl)) = get_playlist(inner, edge.clone()).await {
            info!(
                "relay: playlist refresh recovered on retry after {} ms",
                t0.elapsed().as_millis()
            );
            return Some(pl);
        }
    }
    let (fresh, pl) = get_playlist(inner, Url::parse(portal).ok()?).await?;
    info!(
        "relay: playlist refresh recovered with a fresh token after {} ms",
        t0.elapsed().as_millis()
    );
    *edge = fresh;
    Some(pl)
}

/// GET a playlist, following redirects. Returns the final URL (segment URIs
/// are relative to it) with the parsed playlist. URLs are never logged: the
/// portal's carry the credentials.
async fn get_playlist(inner: &Inner, url: Url) -> Option<(Url, Playlist)> {
    let timeout = inner.cfg.idle_timeout;
    let resp = tokio::time::timeout(timeout, inner.client.get(url).send())
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        warn!("relay: playlist request -> HTTP {}", resp.status().as_u16());
        return None;
    }
    let final_url = resp.url().clone();
    let text = tokio::time::timeout(timeout, resp.text())
        .await
        .ok()?
        .ok()?;
    Some((final_url, playlist::parse(&text)?))
}

enum Sent {
    Done,
    Failed,
    PlayerGone,
}

/// Download one segment and pass its bytes on as they arrive. A refused or
/// stalled request is retried while nothing went out yet; after that a broken
/// download is not restarted (it would repeat bytes) and its rest is skipped.
async fn fetch_segment(inner: &Inner, url: Url, tx: &mpsc::Sender<Bytes>) -> Sent {
    let cfg = &inner.cfg;
    let mut delays = cfg.retry_delays.iter();
    loop {
        match tokio::time::timeout(cfg.idle_timeout, inner.client.get(url.clone()).send()).await {
            Ok(Ok(mut resp)) if resp.status().is_success() => loop {
                match tokio::time::timeout(cfg.idle_timeout, resp.chunk()).await {
                    Ok(Ok(Some(chunk))) => {
                        if tx.send(chunk).await.is_err() {
                            return Sent::PlayerGone;
                        }
                    }
                    Ok(Ok(None)) => return Sent::Done,
                    _ => return Sent::Failed,
                }
            },
            Ok(Ok(resp)) => warn!("relay: segment request -> HTTP {}", resp.status().as_u16()),
            _ => warn!("relay: segment request failed"),
        }
        match delays.next() {
            Some(delay) => tokio::time::sleep(*delay).await,
            None => return Sent::Failed,
        }
    }
}
