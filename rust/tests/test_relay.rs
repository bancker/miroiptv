//! The relay against a mock portal: live playlists that advance per reload,
//! refusals (HTTP 509) and edge tokens that die. What mpv receives must be the
//! segments in order, exactly once, as one stream.
use bongo_player::relay::{Relay, RelayConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Plays its responses in order, then repeats the last one.
struct Script {
    calls: AtomicUsize,
    steps: Vec<ResponseTemplate>,
}

impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        self.steps[i.min(self.steps.len() - 1)].clone()
    }
}

fn script(steps: Vec<ResponseTemplate>) -> Script {
    Script {
        calls: AtomicUsize::new(0),
        steps,
    }
}

/// Live playlist window: `n` one-second segments from sequence `first`.
fn playlist(first: u64, n: u64) -> ResponseTemplate {
    let mut s = format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{first}\n");
    for seq in first..first + n {
        s += &format!("#EXTINF:1.0,\n/seg/{seq}.ts\n");
    }
    ResponseTemplate::new(200).set_body_string(s)
}

fn refused() -> ResponseTemplate {
    ResponseTemplate::new(509).set_body_string("Bandwidth Limit Exceeded")
}

fn redirect(server: &MockServer, to: &str) -> ResponseTemplate {
    ResponseTemplate::new(302).insert_header("Location", format!("{}{to}", server.uri()))
}

/// Segment bodies name themselves, so the received stream shows order and
/// duplicates: "S14;S15;...".
struct Segments;

impl Respond for Segments {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let seq = req
            .url
            .path()
            .trim_start_matches("/seg/")
            .trim_end_matches(".ts");
        ResponseTemplate::new(200).set_body_string(format!("S{seq};"))
    }
}

async fn portal(server: &MockServer, portal_steps: Vec<ResponseTemplate>) {
    Mock::given(method("GET"))
        .and(path("/live/u/p/1.m3u8"))
        .respond_with(script(portal_steps))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/seg/\d+\.ts$"))
        .respond_with(Segments)
        .mount(server)
        .await;
}

async fn edge(server: &MockServer, at: &str, steps: Vec<ResponseTemplate>) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(script(steps))
        .mount(server)
        .await;
}

fn fast() -> RelayConfig {
    RelayConfig {
        start_behind: 2,
        poll_min: Duration::from_millis(50),
        retry_delays: vec![Duration::from_millis(20), Duration::from_millis(20)],
        give_up_after: Duration::from_millis(500),
        idle_timeout: Duration::from_secs(2),
        vod_timeout: Duration::from_secs(10),
        prefetch_ttl: Duration::from_secs(5),
        busy_wait: Duration::from_secs(1),
    }
}

/// Requests the mock server saw for `path` so far.
async fn hits(server: &MockServer, path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == path)
        .count()
}

/// Play the relay's local URL like mpv does and collect the body until `want`
/// bytes arrived, the relay closed the stream, or 10 s passed.
async fn play(local: &str, want: usize) -> (String, bool) {
    let rest = local.trim_start_matches("http://");
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut conn = tokio::net::TcpStream::connect(host).await.unwrap();
    conn.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nRange: bytes=0-\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    let mut closed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 4096];
    loop {
        let body_len = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| raw.len() - i - 4);
        if body_len.is_some_and(|n| n >= want) {
            break;
        }
        match tokio::time::timeout_at(deadline, conn.read(&mut buf)).await {
            Ok(Ok(0)) => {
                closed = true;
                break;
            }
            Ok(Ok(n)) => raw.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    assert!(head.starts_with("HTTP/1.1 200"), "relay answered: {head}");
    (body.to_string(), closed)
}

/// A whole (non-streaming) answer from the relay: head and body.
async fn fetch(local: &str) -> (String, String) {
    let rest = local.trim_start_matches("http://");
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut conn = tokio::net::TcpStream::connect(host).await.unwrap();
    conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), conn.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (format!("{head}\r\n"), body.to_string())
}

/// Status line the relay answers the player with.
async fn status(local: &str) -> String {
    let rest = local.trim_start_matches("http://");
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut conn = tokio::net::TcpStream::connect(host).await.unwrap();
    conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(10), conn.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Request the relay's local URL like mpv does, then close the connection
/// after `after` without waiting for an answer: a zap away.
async fn hang_up_after(local: &str, after: Duration) {
    let rest = local.trim_start_matches("http://");
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut conn = tokio::net::TcpStream::connect(host).await.unwrap();
    conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(after).await;
}

#[tokio::test]
async fn streams_segments_in_order_across_reloads() {
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(
        &server,
        "/edge/a/1",
        vec![playlist(10, 6), playlist(11, 6), playlist(12, 6)],
    )
    .await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, _) = play(&local, "S14;S15;S16;S17;".len()).await;
    assert_eq!(body, "S14;S15;S16;S17;");
}

#[tokio::test]
async fn rides_out_refused_reloads_without_gap_or_repeat() {
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(
        &server,
        "/edge/a/1",
        vec![
            playlist(10, 6),
            refused(),
            refused(),
            playlist(11, 6),
            playlist(12, 6),
        ],
    )
    .await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, _) = play(&local, "S14;S15;S16;S17;".len()).await;
    assert_eq!(body, "S14;S15;S16;S17;");
}

#[tokio::test]
async fn takes_a_fresh_token_from_the_portal_when_the_edge_one_dies() {
    let server = MockServer::start().await;
    portal(
        &server,
        vec![
            redirect(&server, "/edge/a/1"),
            redirect(&server, "/edge/b/1"),
        ],
    )
    .await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6), refused()]).await;
    edge(&server, "/edge/b/1", vec![playlist(11, 6), playlist(12, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, _) = play(&local, "S14;S15;S16;S17;".len()).await;
    assert_eq!(body, "S14;S15;S16;S17;");
}

#[tokio::test]
async fn starts_fetching_before_the_player_connects() {
    // While mpv still tears down the previous stream, the portal round trips
    // should already be running instead of starting once it connects.
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6), playlist(11, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        hits(&server, "/live/u/p/1.m3u8").await,
        1,
        "portal not asked yet"
    );
    let (body, _) = play(&local, "S14;S15;S16;".len()).await;
    assert_eq!(body, "S14;S15;S16;");
}

#[tokio::test]
async fn a_newer_load_cancels_an_unclaimed_prefetch() {
    // Fast zapping must not leave sessions running on the portal.
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    Mock::given(method("GET"))
        .and(path("/live/u/p/2.m3u8"))
        .respond_with(script(vec![redirect(&server, "/edge/b/2")]))
        .mount(&server)
        .await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;
    edge(&server, "/edge/b/2", vec![playlist(20, 6), playlist(21, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let local_b = relay.local_url(&format!("{}/live/u/p/2.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        hits(&server, "/edge/a/1").await <= 1,
        "superseded prefetch kept polling"
    );
    // B kept prefetching meanwhile (its playlist moved on to 26 once), so the
    // player gets that whole cushion at once, in order.
    let (body, _) = play(&local_b, "S24;S25;S26;".len()).await;
    assert_eq!(body, "S24;S25;S26;");
}

#[tokio::test]
async fn an_unclaimed_prefetch_stops_by_itself() {
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(RelayConfig {
        prefetch_ttl: Duration::from_millis(200),
        ..fast()
    })
    .unwrap();
    relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        hits(&server, "/edge/a/1").await <= 1,
        "nobody connected, yet it kept polling"
    );
}

#[tokio::test]
async fn stops_polling_as_soon_as_the_player_hangs_up() {
    // The portal keeps one session per account and the newest wins: a session
    // that outlives its player would fight the one being watched.
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, _) = play(&local, "S14;S15;".len()).await; // connection dropped here
    assert_eq!(body, "S14;S15;");
    let after_hangup = hits(&server, "/edge/a/1").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        hits(&server, "/edge/a/1").await,
        after_hangup,
        "kept polling after the player left"
    );
}

#[tokio::test]
async fn a_player_that_leaves_before_the_stream_starts_stops_the_session() {
    // Flipping channels: mpv drops the connection while the portal is still
    // slow to answer. Nobody wants that stream any more.
    let server = MockServer::start().await;
    portal(
        &server,
        vec![redirect(&server, "/edge/a/1").set_delay(Duration::from_millis(400))],
    )
    .await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    hang_up_after(&local, Duration::from_millis(100)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        hits(&server, "/edge/a/1").await,
        0,
        "started a stream for a player that left"
    );
}

fn busy() -> ResponseTemplate {
    // What the portal answers when flipping channels gets rate limited.
    ResponseTemplate::new(503)
}

#[tokio::test]
async fn a_stream_the_portal_refuses_fails_fast_and_is_remembered() {
    // RTL 4 UHD: this account gets 407 for it, every time, at once.
    let server = MockServer::start().await;
    portal(&server, vec![ResponseTemplate::new(407)]).await;
    let upstream = format!("{}/live/u/p/1.m3u8", server.uri());

    let relay = Relay::start(RelayConfig {
        busy_wait: Duration::from_secs(5),
        ..fast()
    })
    .unwrap();
    assert!(!relay.refused(&upstream));
    let local = relay.local_url(&upstream);
    let t0 = std::time::Instant::now();
    assert!(status(&local).await.starts_with("HTTP/1.1 403"));
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "took {:?}",
        t0.elapsed()
    );
    assert_eq!(hits(&server, "/live/u/p/1.m3u8").await, 1, "asked again");
    assert!(relay.refused(&upstream), "the app needs to know");
}

#[tokio::test]
async fn waits_until_the_portal_lets_a_new_session_in() {
    let server = MockServer::start().await;
    portal(
        &server,
        vec![
            busy(),
            busy(),
            busy(),
            busy(),
            busy(),
            redirect(&server, "/edge/a/1"),
        ],
    )
    .await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, _) = play(&local, "S14;S15;".len()).await;
    assert_eq!(body, "S14;S15;");
}

#[tokio::test]
async fn a_channel_that_is_gone_fails_fast() {
    let server = MockServer::start().await;
    portal(&server, vec![ResponseTemplate::new(404)]).await;

    let relay = Relay::start(RelayConfig {
        busy_wait: Duration::from_secs(5),
        ..fast()
    })
    .unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let t0 = std::time::Instant::now();
    assert!(status(&local).await.starts_with("HTTP/1.1 502"));
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "took {:?}",
        t0.elapsed()
    );
}

const CATCHUP: &str = "/timeshift/u/p/35/2026-10-10:19-20/9.m3u8";

/// A finished (catch-up) playlist with relative segment URIs.
fn vod_playlist() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(
        "#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXT-X-PLAYLIST-TYPE:VOD\n\
         #EXTINF:60.0,\n/ts/a/0.ts?token=t\n#EXTINF:60.0,\n/ts/a/1.ts?token=t\n\
         #EXT-X-ENDLIST\n",
    )
}

async fn catch_up(server: &MockServer, delay: Duration) {
    Mock::given(method("GET"))
        .and(path(CATCHUP))
        .respond_with(redirect(server, "/ts/a/index.m3u8").set_delay(delay))
        .mount(server)
        .await;
    edge(server, "/ts/a/index.m3u8", vec![vod_playlist()]).await;
}

#[tokio::test]
async fn serves_a_catch_up_playlist_to_both_of_mpvs_opens_from_one_portal_request() {
    // mpv opens a URL twice (stream layer, then the HLS demuxer): straight
    // from the portal that is two playlist generations of ~2.5 s each.
    let server = MockServer::start().await;
    catch_up(&server, Duration::ZERO).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.vod_url(&format!("{}{CATCHUP}", server.uri()));
    let (head, first) = fetch(&local).await;
    let (_, second) = fetch(&local).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        first.contains(&format!("\n{}/ts/a/0.ts?token=t\n", server.uri())),
        "{first}"
    );
    assert_eq!(first, second);
    assert_eq!(hits(&server, "/ts/a/index.m3u8").await, 1);
}

#[tokio::test]
async fn waits_for_a_slow_catch_up_playlist_instead_of_asking_again() {
    // Generating an older programme's playlist took the portal over 10 s;
    // each new request starts the generation over.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CATCHUP))
        .respond_with(redirect(&server, "/ts/a/index.m3u8"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ts/a/index.m3u8"))
        .respond_with(vod_playlist().set_delay(Duration::from_millis(2500)))
        .mount(&server)
        .await;

    let relay = Relay::start(fast()).unwrap(); // idle_timeout 2 s
    let local = relay.vod_url(&format!("{}{CATCHUP}", server.uri()));
    let (head, body) = fetch(&local).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(body.contains("#EXT-X-ENDLIST"), "{body}");
    assert_eq!(
        hits(&server, "/ts/a/index.m3u8").await,
        1,
        "generation restarted"
    );
}

#[tokio::test]
async fn hands_a_still_growing_playlist_to_the_player_as_is() {
    // Only a finished playlist can be served from a copy.
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;
    let upstream = format!("{}/live/u/p/1.m3u8", server.uri());

    let relay = Relay::start(fast()).unwrap();
    let (head, _) = fetch(&relay.vod_url(&upstream)).await;
    assert!(head.starts_with("HTTP/1.1 302"), "{head}");
    assert!(
        head.contains(&format!("\r\nLocation: {upstream}\r\n")),
        "{head}"
    );
}

#[tokio::test]
async fn a_catch_up_the_portal_cannot_give_answers_502() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CATCHUP))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.vod_url(&format!("{}{CATCHUP}", server.uri()));
    assert!(status(&local).await.starts_with("HTTP/1.1 502"));
}

#[tokio::test]
async fn a_newer_load_cancels_a_catch_up_still_being_fetched() {
    let server = MockServer::start().await;
    catch_up(&server, Duration::from_millis(400)).await;
    portal(&server, vec![redirect(&server, "/edge/a/1")]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    relay.vod_url(&format!("{}{CATCHUP}", server.uri()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(
        hits(&server, "/ts/a/index.m3u8").await,
        0,
        "kept fetching a catch-up nobody wants"
    );
}

#[tokio::test]
async fn a_catch_up_load_cancels_an_unclaimed_live_prefetch() {
    let server = MockServer::start().await;
    catch_up(&server, Duration::ZERO).await;
    portal(
        &server,
        vec![redirect(&server, "/edge/a/1").set_delay(Duration::from_millis(400))],
    )
    .await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6)]).await;

    let relay = Relay::start(fast()).unwrap();
    relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    relay.vod_url(&format!("{}{CATCHUP}", server.uri()));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(
        hits(&server, "/edge/a/1").await,
        0,
        "kept prefetching a channel nobody wants"
    );
}

#[tokio::test]
async fn closes_the_stream_once_the_portal_is_gone_for_good() {
    // Lets the app's own recovery (and its toast) take over.
    let server = MockServer::start().await;
    portal(&server, vec![redirect(&server, "/edge/a/1"), refused()]).await;
    edge(&server, "/edge/a/1", vec![playlist(10, 6), refused()]).await;

    let relay = Relay::start(fast()).unwrap();
    let local = relay.local_url(&format!("{}/live/u/p/1.m3u8", server.uri()));
    let (body, closed) = play(&local, usize::MAX).await;
    assert_eq!(body, "S14;S15;");
    assert!(closed, "stream must end, not hang");
}
