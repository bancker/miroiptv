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
    }
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
