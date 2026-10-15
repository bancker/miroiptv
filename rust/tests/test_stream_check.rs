//! The check before a defect mark (stream_check.rs): one GET of a confirmed
//! suspect's address, with its own headers, the status only; and which
//! addresses are on one server.

use bongo_player::m3u::with_headers;
use bongo_player::stream_check::{
    ask, http_client, same_server, Answer, Check, Checked, StreamChecker, AT_ONCE, MPV_USER_AGENT,
};
use std::io::{Read, Write};
use std::time::Duration;
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 15 October 2026, 00:00 UTC.
const FAILED: i64 = 1_792_022_400;

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
    // proof of a dead stream: the check verifies certificates, mpv does not
    // yet, so it marks nothing.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        // Read the ClientHello before answering, and read on until the client
        // hangs up: a socket closed with unread data is reset on Windows.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    });
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("https://127.0.0.1:{port}/live.m3u8")).await;
    assert_eq!(answer, Answer::Unchecked);
    assert!(!answer.is_dead(), "no proof, no mark");
}

#[tokio::test]
async fn no_headers_within_the_time_is_a_timeout() {
    let server = answering(200, Duration::from_secs(3)).await;
    // The check's own client waits CHECK_TIMEOUT (5 s); this one 200 ms.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let answer = ask(&client, &format!("{}/slow", server.uri())).await;
    assert_eq!(answer, Answer::TimedOut);
    assert!(answer.is_dead());
    assert_eq!(answer.kind(), "timeout");
}

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
