//! The check before a defect mark (stream_check.rs): one GET of a confirmed
//! suspect's address, with its own headers, the status only; and which
//! addresses are on one server.

use bongo_player::m3u::with_headers;
use bongo_player::stream_check::{
    ask, http_client, same_server, Answer, Check, Checked, StreamChecker, AT_ONCE, MPV_USER_AGENT,
};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 15 October 2026, 00:00 UTC.
const FAILED: i64 = 1_792_022_400;

/// A server on this machine for one connection: `serve` gets it. A raw socket
/// says what real servers do and wiremock cannot: an `ICY 200 OK` reply, a
/// hang-up, headers and then nothing.
fn serving(serve: impl FnOnce(TcpStream) + Send + 'static) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            serve(stream);
        }
    });
    port
}

/// Reads what the client sends first: its request, or its TLS ClientHello.
fn read_first(stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
}

/// Reads on until the client hangs up and says whether it did within 3 s. (A
/// socket closed with unread data is reset on Windows, so a server reads on.)
fn until_hung_up(stream: &mut TcpStream) -> bool {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) => return !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        }
    }
}

/// A server that answers every request with `status`, after `delay`.
async fn answering(status: u16, delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status).set_delay(delay))
        .mount(&server)
        .await;
    server
}

/// A port on this machine that nothing listens on.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn check(stream_id: i64, url: String) -> Check {
    Check {
        stream_id,
        name: format!("K{stream_id}"),
        failed_unix: FAILED,
        url,
    }
}

/// Drains `checker` until `n` answers came (5 s at most).
async fn answers(checker: &mut StreamChecker, n: usize) -> Vec<Checked> {
    let mut got = Vec::new();
    for _ in 0..1000 {
        got.extend(checker.drain());
        if got.len() >= n {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{} of {n} answers came", got.len());
}

#[tokio::test]
async fn an_address_that_answers_200_answers_again() {
    let server = answering(200, Duration::ZERO).await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/live.m3u8", server.uri())).await;
    assert_eq!(answer, Answer::Answers(200));
    assert!(!answer.is_dead());
}

#[tokio::test]
async fn an_error_status_after_redirects_is_dead_and_a_redirect_to_a_live_one_is_not() {
    let server = MockServer::start().await;
    let to = |place: &str| ResponseTemplate::new(302).insert_header("Location", place);
    Mock::given(path("/dead"))
        .respond_with(to("/gone"))
        .mount(&server)
        .await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/moved"))
        .respond_with(to("/here"))
        .mount(&server)
        .await;
    Mock::given(path("/here"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let dead = ask(&client, &format!("{}/dead", server.uri())).await;
    assert_eq!(dead, Answer::Status(404));
    assert!(dead.is_dead());
    assert_eq!(dead.kind(), "http 404");
    let moved = ask(&client, &format!("{}/moved", server.uri())).await;
    assert_eq!(moved, Answer::Answers(200));
}

#[tokio::test]
async fn a_closed_port_is_a_failed_connection() {
    let client = http_client().unwrap();
    let url = format!("http://127.0.0.1:{}/live.m3u8", closed_port());
    let answer = ask(&client, &url).await;
    assert_eq!(answer, Answer::ConnectFailed);
    assert!(answer.is_dead());
    assert_eq!(answer.kind(), "connect failed");
}

#[tokio::test]
async fn a_server_that_speaks_no_tls_on_an_https_address_is_not_a_dead_one() {
    // Plain HTTP where TLS is expected fails the handshake the way a
    // certificate nobody can verify does (the same error chain). That is no
    // proof of a dead stream: the check verifies certificates, and a failure
    // there marks nothing.
    let port = serving(|mut stream| {
        read_first(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        until_hung_up(&mut stream);
    });
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("https://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Unchecked);
    assert!(!answer.is_dead(), "no proof, no mark");
}

// ---- a hang-up during an https handshake: the server took the connection, so
// it is there; the check could not find out more ----

#[tokio::test]
async fn an_https_server_that_hangs_up_after_the_client_hello_is_not_a_dead_stream() {
    let port = serving(|mut stream| {
        read_first(&mut stream);
        // Dropped with the ClientHello read: the handshake ends there.
    });
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("https://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Unchecked);
    assert!(!answer.is_dead(), "reached: no proof, no mark");
}

#[tokio::test]
async fn an_https_server_that_hangs_up_at_once_is_not_a_dead_stream() {
    let port = serving(drop);
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("https://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Unchecked);
    assert!(!answer.is_dead(), "reached: no proof, no mark");
}

#[tokio::test]
async fn an_http_server_that_hangs_up_at_once_is_a_failed_request() {
    // The same server over http: the hang-up comes after the connection, as
    // the one after the request does.
    let port = serving(drop);
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("http://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Failed);
    assert!(!answer.is_dead());
}

#[tokio::test]
async fn a_closed_port_over_https_is_a_failed_connection() {
    let client = http_client().unwrap();
    let url = format!("https://127.0.0.1:{}/live.m3u8", closed_port());
    let answer = ask(&client, &url).await;
    assert_eq!(answer, Answer::ConnectFailed);
    assert!(answer.is_dead());
}

// ---- a busy answer (the user, 2026-10-16): 429, 503 and 509 prove nothing ----

#[tokio::test]
async fn a_busy_answer_is_not_a_dead_stream() {
    let client = http_client().unwrap();
    for status in [429, 503, 509] {
        let server = answering(status, Duration::ZERO).await;
        let answer = ask(&client, &format!("{}/live.m3u8", server.uri())).await;
        assert_eq!(answer, Answer::Busy(status), "{status}");
        assert!(!answer.is_dead(), "{status}: no proof, no mark");
        assert_eq!(answer.kind(), format!("busy, http {status}"));
    }
}

#[tokio::test]
async fn every_other_status_of_400_or_more_is_still_a_dead_stream() {
    let client = http_client().unwrap();
    for status in [400, 403, 404, 410, 500, 502] {
        let server = answering(status, Duration::ZERO).await;
        let answer = ask(&client, &format!("{}/live.m3u8", server.uri())).await;
        assert_eq!(answer, Answer::Status(status), "{status}");
        assert!(answer.is_dead(), "{status}");
    }
}

// ---- only proof of a dead stream marks (spec 4.2: better a dead channel too
// many in the list than a good one lost) ----

#[test]
fn only_proof_of_a_dead_stream_is_dead() {
    // A status of 400 or more but a busy one, no connection, no answer in time.
    for proof in [
        Answer::Status(404),
        Answer::Status(500),
        Answer::ConnectFailed,
        Answer::TimedOut,
    ] {
        assert!(proof.is_dead(), "{proof:?}");
    }
    // An answer, a busy one, and every case the check could not find out.
    for no_proof in [
        Answer::Answers(200),
        Answer::Answers(302),
        Answer::Busy(429),
        Answer::Busy(503),
        Answer::Busy(509),
        Answer::Failed,
        Answer::Unchecked,
    ] {
        assert!(!no_proof.is_dead(), "{no_proof:?}");
    }
}

#[tokio::test]
async fn an_icy_reply_is_not_a_dead_stream() {
    // A Shoutcast radio answers `ICY 200 OK`, which is no HTTP that reqwest
    // reads: the server did answer, the check cannot tell what.
    let port = serving(|mut stream| {
        read_first(&mut stream);
        let _ = stream.write_all(b"ICY 200 OK\r\nicy-name: Radio\r\nicy-metaint: 16000\r\n\r\n");
        until_hung_up(&mut stream);
    });
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("http://127.0.0.1:{port}/stream")).await;
    assert_eq!(answer, Answer::Failed);
    assert!(!answer.is_dead(), "the server answered: no proof, no mark");
}

#[tokio::test]
async fn a_hang_up_before_any_reply_is_not_a_dead_stream() {
    let port = serving(|mut stream| {
        read_first(&mut stream);
        // Dropped with the request read: the client sees the end of the
        // connection and no reply.
    });
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("http://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Failed);
    assert!(!answer.is_dead(), "no reply is no proof of a dead stream");
}

#[tokio::test]
async fn a_redirect_that_never_ends_is_not_a_dead_stream() {
    // Only 3xx ever came, and a 3xx drops the suspicion (spec 4.2).
    let server = MockServer::start().await;
    Mock::given(path("/round"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/round"))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/round", server.uri())).await;
    assert_eq!(answer, Answer::Failed);
    assert!(!answer.is_dead(), "only 3xx came: no proof, no mark");
}

#[tokio::test]
async fn an_address_it_cannot_build_a_request_for_is_not_asked() {
    // No server needed: nothing is sent.
    let client = http_client().unwrap();
    for url in [
        "http://exa mple.com/x",
        "http://host:99999/x",
        "http://{{hostname}}/x",
    ] {
        let answer = ask(&client, url).await;
        assert_eq!(answer, Answer::Unchecked, "{url}");
        assert!(!answer.is_dead(), "{url}");
    }
}

#[tokio::test]
async fn a_redirect_to_an_address_that_is_no_web_address_is_not_asked() {
    // The server only redirected, to a place the check cannot ask (mpv can
    // play it).
    let server = MockServer::start().await;
    Mock::given(path("/elsewhere"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "rtmp://a.example/live/9"),
        )
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/elsewhere", server.uri())).await;
    assert_eq!(answer, Answer::Unchecked);
    assert!(!answer.is_dead());
}

// ---- the body is never read ----

#[tokio::test]
async fn the_body_is_never_read_so_a_stream_that_never_ends_answers_at_once() {
    // A live stream: the status line and the headers, then nothing more for
    // ever, and the connection stays open.
    let hung_up = Arc::new(AtomicBool::new(false));
    let seen = hung_up.clone();
    let port = serving(move |mut stream| {
        read_first(&mut stream);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\n\r\n",
        );
        seen.store(until_hung_up(&mut stream), Ordering::SeqCst);
    });
    let client = http_client().unwrap();
    let url = format!("http://127.0.0.1:{port}/live.ts");
    // Well under CHECK_TIMEOUT (5 s): a check that waited for the body would
    // sit there until that limit, or for ever.
    let asked = tokio::time::timeout(Duration::from_secs(2), ask(&client, &url)).await;
    assert_eq!(
        asked.ok(),
        Some(Answer::Answers(200)),
        "the check waited for a body that never comes"
    );
    // The response was dropped unread: the connection closes, and nothing of
    // the stream is downloaded.
    for _ in 0..200 {
        if hung_up.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        hung_up.load(Ordering::SeqCst),
        "the connection stayed open after the answer"
    );
}

// (No headers within the time is a timeout: tested on the check's own client in
// stream_check.rs, whose short-limit constructor is private.)

#[tokio::test]
async fn the_check_sends_the_channels_own_user_agent_and_referer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twee.m3u8"))
        .and(header("user-agent", "Bongo/1.0"))
        .and(header("referer", "https://www.example.com/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // Anything else gets wiremock's 404, which the check would call dead.
    let url = with_headers(
        format!("{}/twee.m3u8", server.uri()),
        Some("Bongo/1.0"),
        Some("https://www.example.com/"),
    );
    let client = http_client().unwrap();
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
}

#[tokio::test]
async fn without_a_user_agent_of_its_own_the_check_sends_mpvs() {
    let server = MockServer::start().await;
    Mock::given(header("user-agent", MPV_USER_AGENT))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/een.m3u8", server.uri())).await;
    assert_eq!(answer, Answer::Answers(200));
}

#[tokio::test]
async fn a_user_agent_that_is_not_ascii_is_sent_as_its_bytes() {
    let server = answering(200, Duration::ZERO).await;
    // mpv hands a "Zürich-TV" User-Agent to the server as its UTF-8 bytes.
    let url = with_headers(
        format!("{}/zurich.m3u8", server.uri()),
        Some("Zürich-TV"),
        None,
    );
    let client = http_client().unwrap();
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
    let asked = server.received_requests().await.unwrap();
    let agent = asked[0].headers.get("user-agent").map(|v| v.as_bytes());
    assert_eq!(agent, Some("Zürich-TV".as_bytes()));
}

#[tokio::test]
async fn a_redirect_hands_the_address_on_to_nobody() {
    let server = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/b"))
        .mount(&server)
        .await;
    Mock::given(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let url = format!("{}/a?token=geheim", server.uri());
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
    let asked = server.received_requests().await.unwrap();
    assert_eq!(asked.len(), 2);
    assert!(
        asked[1].headers.get("referer").is_none(),
        "no Referer of the check's own: {:?}",
        asked[1].headers
    );
}

#[test]
fn what_the_log_says_of_an_answer_names_no_address() {
    assert_eq!(Answer::Status(404).kind(), "http 404");
    assert_eq!(Answer::Answers(302).kind(), "http 302");
    assert_eq!(Answer::Busy(503).kind(), "busy, http 503");
    assert_eq!(Answer::ConnectFailed.kind(), "connect failed");
    assert_eq!(Answer::TimedOut.kind(), "timeout");
    assert_eq!(Answer::Failed.kind(), "request failed");
    assert_eq!(
        Answer::Unchecked.kind(),
        "an address it cannot ask or a certificate it could not verify"
    );
    assert!(!Answer::Answers(302).is_dead());
    assert!(!Answer::Unchecked.is_dead(), "no check, no mark");
}

#[tokio::test]
async fn four_checks_at_a_time_and_the_next_when_one_answers() {
    let server = answering(404, Duration::from_millis(300)).await;
    let mut checker = StreamChecker::new();
    for id in 1..=6 {
        checker.start(check(id, format!("{}/{id}", server.uri())));
    }
    assert_eq!(checker.in_flight(), AT_ONCE);
    assert!(checker.checking(6), "the sixth waits for a place");
    let got = answers(&mut checker, 6).await;
    let mut ids: Vec<i64> = got.iter().map(|c| c.stream_id).collect();
    ids.sort();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
    assert!(got
        .iter()
        .all(|c| c.answer == Answer::Status(404) && c.failed_unix == FAILED));
    assert_eq!(checker.in_flight(), 0);
    let asked = server.received_requests().await.unwrap();
    assert_eq!(asked.len(), 6, "one request per channel");
}

#[tokio::test]
async fn one_check_per_channel() {
    let server = answering(404, Duration::from_millis(100)).await;
    let mut checker = StreamChecker::new();
    checker.start(check(1, format!("{}/1", server.uri())));
    checker.start(check(1, format!("{}/1", server.uri())));
    assert_eq!(checker.in_flight(), 1);
    assert_eq!(answers(&mut checker, 1).await.len(), 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(checker.drain().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_forgotten_or_reset_check_answers_nothing() {
    let server = answering(404, Duration::from_millis(200)).await;
    let mut checker = StreamChecker::new();
    for id in 1..=3 {
        checker.start(check(id, format!("{}/{id}", server.uri())));
    }
    checker.forget(1);
    assert!(!checker.checking(1));
    assert_eq!(checker.in_flight(), 2);
    checker.reset();
    assert_eq!(checker.in_flight(), 0);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(checker.drain().is_empty(), "their answers are dropped");
}

#[tokio::test]
async fn a_forgotten_check_that_starts_again_answers_once_with_its_second_address() {
    // `forget` aborts the first check's task, so its late answer cannot even
    // come here. The ticket that guards an answer already in the slot is tested
    // in stream_check.rs, which can see the slot.
    let slow = answering(200, Duration::from_millis(500)).await;
    let fast = answering(404, Duration::ZERO).await;
    let mut checker = StreamChecker::new();
    checker.start(check(1, format!("{}/1", slow.uri())));
    // Let the first check get under way: its request is with the slow server.
    tokio::time::sleep(Duration::from_millis(100)).await;
    checker.forget(1);
    checker.start(check(1, format!("{}/1", fast.uri())));
    let got = answers(&mut checker, 1).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].answer, Answer::Status(404), "the second address's");
    // Past the moment the slow server would have answered.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        checker.drain().is_empty(),
        "the first check's late Answers(200) never lands"
    );
    assert_eq!(checker.in_flight(), 0);
}

#[tokio::test]
async fn a_forgotten_check_gives_its_place_to_the_next_at_once() {
    let server = answering(404, Duration::from_millis(300)).await;
    let mut checker = StreamChecker::new();
    for id in 1..=5 {
        checker.start(check(id, format!("{}/{id}", server.uri())));
    }
    assert_eq!(checker.in_flight(), AT_ONCE, "the fifth waits");
    checker.forget(1);
    // No drain in between: no place stays free while a check waits.
    assert_eq!(checker.in_flight(), AT_ONCE, "the fifth took the place");
    let got = answers(&mut checker, 4).await;
    let mut ids: Vec<i64> = got.iter().map(|c| c.stream_id).collect();
    ids.sort();
    assert_eq!(ids, [2, 3, 4, 5]);
    assert_eq!(checker.in_flight(), 0);
}

#[tokio::test]
async fn an_address_that_is_no_web_address_is_not_asked() {
    let mut checker = StreamChecker::new();
    checker.start(check(9, "rtmp://a.example/live/9".into()));
    let got = answers(&mut checker, 1).await;
    assert_eq!(got[0].answer, Answer::Unchecked);
}

// ---- one server or two (the app never asks the server of what plays) ----

#[test]
fn the_same_host_and_port_with_another_path_is_the_same_server() {
    assert!(same_server(
        "http://tv.example:8080/live/1.m3u8",
        "http://tv.example:8080/live/2.ts?token=abc"
    ));
}

#[test]
fn another_port_is_another_server() {
    assert!(!same_server(
        "http://tv.example:8080/1.m3u8",
        "http://tv.example:8081/1.m3u8"
    ));
}

#[test]
fn a_default_port_written_out_is_the_port_left_out() {
    assert!(same_server(
        "http://tv.example:80/1.m3u8",
        "http://tv.example/2.m3u8"
    ));
    assert!(same_server(
        "https://tv.example/1.m3u8",
        "https://tv.example:443/2.m3u8"
    ));
}

#[test]
fn the_letter_case_of_the_host_does_not_count() {
    assert!(same_server(
        "http://TV.Example/1.m3u8",
        "http://tv.example/2.m3u8"
    ));
}

#[test]
fn the_headers_after_a_pipe_do_not_count() {
    // Right after the port: read as part of the address, they would make an
    // address that cannot be read.
    let headed = with_headers(
        "http://tv.example:8080".into(),
        Some("Bongo/1.0"),
        Some("https://www.example.com/"),
    );
    assert!(same_server(&headed, "http://tv.example:8080/live/2.m3u8"));
}

#[test]
fn another_host_is_another_server() {
    assert!(!same_server("http://a.example/1", "http://b.example/1"));
}

#[test]
fn localhost_and_127_0_0_1_are_two_servers_on_one_port() {
    // The pair the e2e scenario M-defect relies on (plan ruling 27): one
    // channel under localhost, the others under 127.0.0.1, on one mock port.
    assert!(!same_server(
        "http://localhost:8080/m3uplay/dead.m3u8",
        "http://127.0.0.1:8080/m3uplay/ua.m3u8"
    ));
}

#[test]
fn http_and_https_on_one_host_are_two_servers() {
    assert!(!same_server(
        "http://tv.example/1.m3u8",
        "https://tv.example/1.m3u8"
    ));
}

#[test]
fn an_address_that_cannot_be_read_is_no_server() {
    assert!(!same_server("tv.example/1.m3u8", "tv.example/1.m3u8"));
    assert!(!same_server("http://tv.example/1.m3u8", ""));
}
