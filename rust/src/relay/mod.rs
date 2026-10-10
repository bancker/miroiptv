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
use tokio::sync::{mpsc, oneshot, watch};
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
    /// How long a catch-up / VOD playlist may take to arrive: the portal
    /// generates it per request (over 10 s for older programmes) and asking
    /// again starts that over. mpv's own network timeout.
    pub vod_timeout: Duration,
    /// How long a prefetch waits for the player to connect before stopping.
    pub prefetch_ttl: Duration,
    /// How long starting a session keeps trying while the portal answers
    /// "busy": 407 for ~10-13 s after catch-up (the old session still counts),
    /// 503 when flipping channels gets rate limited.
    pub busy_wait: Duration,
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
            vod_timeout: Duration::from_secs(60),
            prefetch_ttl: Duration::from_secs(10),
            busy_wait: Duration::from_secs(20),
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

/// Local stream ids <-> upstream playlist URLs (one entry per distinct
/// channel watched, so tiny), plus the prefetch waiting for its player and
/// the catch-up playlist being served.
#[derive(Default)]
struct Routes {
    ids: HashMap<String, u64>,
    urls: HashMap<u64, String>,
    pending: HashMap<u64, Upstream>,
    vod: Option<Vod>,
}

impl Routes {
    fn id_for(&mut self, upstream: &str) -> u64 {
        let next = self.ids.len() as u64;
        let id = *self.ids.entry(upstream.to_string()).or_insert(next);
        self.urls.insert(id, upstream.to_string());
        id
    }

    /// A new load: stop fetching for the ones before it that no player is
    /// playing yet - each holds the account's only portal session.
    fn supersede(&mut self) {
        for (_, old) in self.pending.drain() {
            old.task.abort();
        }
        if let Some(old) = self.vod.take() {
            old.task.abort();
        }
    }
}

/// A finished playlist (catch-up, VOD) fetched once and served from memory:
/// mpv opens a URL twice, and each open at the portal generates the playlist
/// anew (~2.5 s, far longer when the portal is busy).
struct Vod {
    id: u64,
    state: watch::Receiver<VodState>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
enum VodState {
    Fetching,
    /// The finished playlist, its URIs made absolute.
    Ready(Bytes),
    /// Not a finished playlist: the player gets it from the portal itself.
    Growing,
    Failed,
}

/// The upstream half of a session: fetching runs in `task` and queues its
/// output in `rx`; `ready` reports whether a first playlist came in.
struct Upstream {
    started: Instant,
    ready: oneshot::Receiver<bool>,
    rx: mpsc::Receiver<Bytes>,
    task: tokio::task::JoinHandle<()>,
}

fn spawn_upstream(inner: Arc<Inner>, portal: String) -> Upstream {
    let (tx, rx) = mpsc::channel::<Bytes>(CHANNEL_CHUNKS);
    let (ready_tx, ready) = oneshot::channel();
    let task = tokio::spawn(async move {
        match first_playlist(&inner, &portal, inner.cfg.idle_timeout).await {
            Some(first) => {
                let _ = ready_tx.send(true);
                fetch_loop(inner, portal, first.url, first.playlist, tx).await;
            }
            None => {
                let _ = ready_tx.send(false);
            }
        }
    });
    Upstream {
        started: Instant::now(),
        ready,
        rx,
        task,
    }
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
    /// Fetching starts right away, while mpv is still tearing down the
    /// previous stream, so the portal round trips overlap that work instead
    /// of following it. A newer load supersedes a prefetch nobody claimed,
    /// and an unclaimed one stops after `prefetch_ttl`.
    pub fn local_url(&self, upstream: &str) -> String {
        let up = spawn_upstream(self.inner.clone(), upstream.to_string());
        let started = up.started;
        let id = {
            let mut routes = self.inner.routes.lock();
            let id = routes.id_for(upstream);
            routes.supersede();
            routes.pending.insert(id, up);
            id
        };
        let weak = Arc::downgrade(&self.inner);
        let ttl = self.inner.cfg.prefetch_ttl;
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let Some(inner) = weak.upgrade() else { return };
            let mut routes = inner.routes.lock();
            if routes
                .pending
                .get(&id)
                .is_some_and(|p| p.started == started)
            {
                if let Some(stale) = routes.pending.remove(&id) {
                    stale.task.abort();
                }
            }
        });
        format!("http://127.0.0.1:{}/live/{id}.ts", self.inner.port)
    }

    /// Local URL for mpv that plays the catch-up / VOD playlist at `upstream`
    /// from one portal request instead of two. Segments still come straight
    /// from the portal's servers, so seeking works as before.
    pub fn vod_url(&self, upstream: &str) -> String {
        let (tx, state) = watch::channel(VodState::Fetching);
        let inner = self.inner.clone();
        let portal = upstream.to_string();
        let task = tokio::spawn(async move {
            let t0 = Instant::now();
            let state = match first_playlist(&inner, &portal, inner.cfg.vod_timeout).await {
                Some(f) if f.playlist.ended => {
                    info!(
                        "relay: catch-up playlist in {} ms ({} segments)",
                        t0.elapsed().as_millis(),
                        f.playlist.segments.len()
                    );
                    VodState::Ready(playlist::absolutize(&f.text, &f.url).into())
                }
                Some(_) => VodState::Growing,
                None => VodState::Failed,
            };
            let _ = tx.send(state);
        });
        let id = {
            let mut routes = self.inner.routes.lock();
            let id = routes.id_for(upstream);
            routes.supersede();
            routes.vod = Some(Vod { id, state, task });
            id
        };
        format!("http://127.0.0.1:{}/vod/{id}.m3u8", self.inner.port)
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
    let vod_id = path
        .strip_prefix("/vod/")
        .and_then(|p| p.strip_suffix(".m3u8"))
        .and_then(|id| id.parse::<u64>().ok());
    if let Some(id) = vod_id {
        let found = {
            let routes = inner.routes.lock();
            routes.vod.as_ref().filter(|v| v.id == id).map(|v| {
                (
                    v.state.clone(),
                    routes.urls.get(&id).cloned().unwrap_or_default(),
                )
            })
        };
        return serve_vod(conn, found).await;
    }
    let id = path
        .strip_prefix("/live/")
        .and_then(|p| p.strip_suffix(".ts"))
        .and_then(|id| id.parse::<u64>().ok());
    let (upstream, prefetched) = match id {
        Some(id) => {
            let mut routes = inner.routes.lock();
            (routes.urls.get(&id).cloned(), routes.pending.remove(&id))
        }
        None => (None, None),
    };
    match upstream {
        Some(upstream) => {
            let up = prefetched.unwrap_or_else(|| spawn_upstream(inner.clone(), upstream));
            session(up, conn).await
        }
        None => {
            let _ = conn
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    }
}

/// Answer one of mpv's opens of a catch-up playlist, once it is fetched.
async fn serve_vod(mut conn: TcpStream, found: Option<(watch::Receiver<VodState>, String)>) {
    let (state, upstream) = match found {
        Some((mut rx, upstream)) => {
            let state = rx
                .wait_for(|s| !matches!(s, VodState::Fetching))
                .await
                .map(|s| s.clone())
                .unwrap_or(VodState::Failed); // superseded while fetching
            (state, upstream)
        }
        None => (VodState::Failed, String::new()),
    };
    let reply = match state {
        VodState::Ready(body) => [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\n\
                 Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
            &body,
        ]
        .concat(),
        VodState::Growing => format!(
            "HTTP/1.1 302 Found\r\nLocation: {upstream}\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n"
        )
        .into_bytes(),
        VodState::Fetching | VodState::Failed => {
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        }
    };
    let _ = conn.write_all(&reply).await;
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
/// whatever the upstream produces until the player hangs up (zap, quit) or
/// the upstream gives up.
async fn session(mut up: Upstream, mut conn: TcpStream) {
    let ready = tokio::select! {
        ready = &mut up.ready => matches!(ready, Ok(true)),
        _ = hung_up(&mut conn) => {
            up.task.abort(); // zapped on while the portal was still answering
            return;
        }
    };
    if !ready {
        warn!("relay: no playlist from the portal - refusing the stream");
        let _ = conn
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        up.task.abort();
        return;
    }
    let ok = b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    if conn.write_all(ok).await.is_ok() {
        let (mut from_player, mut to_player) = conn.split();
        let mut first = true;
        loop {
            // Waiting for the next chunk can take a segment's length; the
            // portal must not be polled for a player that already left.
            let chunk = tokio::select! {
                chunk = up.rx.recv() => chunk,
                _ = hung_up(&mut from_player) => None,
            };
            let Some(chunk) = chunk else { break };
            if first {
                info!(
                    "relay: first bytes to the player {} ms after the load",
                    up.started.elapsed().as_millis()
                );
                first = false;
            }
            if to_player.write_all(&chunk).await.is_err() {
                break; // player hung up
            }
        }
    }
    up.task.abort();
}

/// Resolves once the player closes the connection: mpv sends nothing after
/// its request, so anything but more data means it is gone.
async fn hung_up(from_player: &mut (impl AsyncReadExt + Unpin)) {
    let mut scratch = [0u8; 64];
    while matches!(from_player.read(&mut scratch).await, Ok(n) if n > 0) {}
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
/// server). Retried like any refusal, and for up to `busy_wait` while the
/// portal says it is busy - a channel that is really gone still fails fast.
async fn first_playlist(inner: &Inner, portal: &str, timeout: Duration) -> Option<Fetched> {
    let url = Url::parse(portal).ok()?;
    let cfg = &inner.cfg;
    let t0 = Instant::now();
    let mut retries = cfg.retry_delays.iter();
    let mut pause = cfg.poll_min;
    let mut refusals = 0;
    loop {
        match get_playlist(inner, url.clone(), timeout).await {
            Ok(found) => {
                if refusals > 0 {
                    info!(
                        "relay: portal let the session in after {} ms ({} refusals)",
                        t0.elapsed().as_millis(),
                        refusals
                    );
                }
                return Some(found);
            }
            Err(refused) => {
                refusals += 1;
                pause = match retries.next() {
                    Some(delay) => *delay,
                    None if matches!(refused, Refused::Busy)
                        && t0.elapsed() + pause < cfg.busy_wait =>
                    {
                        pause
                    }
                    None => return None,
                };
            }
        }
        tokio::time::sleep(pause).await;
    }
}

/// Refresh on the edge URL we were redirected to; when refused, retry it,
/// then take a fresh token from the portal.
async fn reload(inner: &Inner, portal: &str, edge: &mut Url) -> Option<Playlist> {
    let t0 = Instant::now();
    let timeout = inner.cfg.idle_timeout;
    if let Ok(f) = get_playlist(inner, edge.clone(), timeout).await {
        return Some(f.playlist);
    }
    for delay in &inner.cfg.retry_delays {
        tokio::time::sleep(*delay).await;
        if let Ok(f) = get_playlist(inner, edge.clone(), timeout).await {
            info!(
                "relay: playlist refresh recovered on retry after {} ms",
                t0.elapsed().as_millis()
            );
            return Some(f.playlist);
        }
    }
    let fresh = get_playlist(inner, Url::parse(portal).ok()?, timeout)
        .await
        .ok()?;
    info!(
        "relay: playlist refresh recovered with a fresh token after {} ms",
        t0.elapsed().as_millis()
    );
    *edge = fresh.url;
    Some(fresh.playlist)
}

/// Why a playlist request brought no playlist.
enum Refused {
    /// 407 (the previous session still counts), 503 / 429 (rate limited),
    /// 509 (session taken over): the portal is there, just not for us yet.
    Busy,
    /// Anything else: not found, timed out, not a playlist...
    Failed,
}

/// A playlist as fetched: where it ended up after redirects (its URIs are
/// relative to that), its text, and what parsing found in it.
struct Fetched {
    url: Url,
    text: String,
    playlist: Playlist,
}

/// GET a playlist, following redirects, within `timeout`. URLs are never
/// logged: the portal's carry the credentials.
async fn get_playlist(inner: &Inner, url: Url, timeout: Duration) -> Result<Fetched, Refused> {
    let resp = match tokio::time::timeout(timeout, inner.client.get(url).send()).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            warn!("relay: playlist request failed: {}", e.without_url());
            return Err(Refused::Failed);
        }
        Err(_) => {
            warn!(
                "relay: playlist request timed out after {} s",
                timeout.as_secs()
            );
            return Err(Refused::Failed);
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        warn!("relay: playlist request -> HTTP {}", status);
        return Err(match status {
            407 | 429 | 503 | 509 => Refused::Busy,
            _ => Refused::Failed,
        });
    }
    let url = resp.url().clone();
    match tokio::time::timeout(timeout, resp.text()).await {
        Ok(Ok(text)) => match playlist::parse(&text) {
            Some(playlist) => Ok(Fetched {
                url,
                text,
                playlist,
            }),
            None => {
                warn!("relay: playlist response is not a playlist");
                Err(Refused::Failed)
            }
        },
        _ => {
            warn!("relay: playlist download broke off");
            Err(Refused::Failed)
        }
    }
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
