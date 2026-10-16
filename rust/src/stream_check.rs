//! The check before a defect mark (spec 2026-10-15 §4.2). An M3U channel that
//! failed before its first picture, and that another channel's picture then
//! confirmed, is not marked at once: its address is asked once more. One GET
//! with the channel's own headers, the status only, a short limit, and as mpv
//! asks: header values as bytes. Only proof of a dead stream marks it: a
//! status of 400 or more after redirects, no connection, no answer in time. A
//! live one (2xx, 3xx) drops the suspicion, so a Wi-Fi drop of under two
//! minutes marks no good channel. What it cannot find out is not marked
//! either (better a dead channel too many in the list than a good one lost):
//! an address it cannot ask, a reply it cannot use (a redirect that never
//! ends, an `ICY 200 OK`, a hang-up after the connection: for https, after the
//! handshake), and a certificate or TLS handshake it could not verify. A
//! hang-up during an https handshake counts as no connection, so it marks,
//! unless TLS reports it as an error (an alert, a certificate, plain HTTP on a
//! TLS port): then the handshake could not be verified, and nothing is marked.
//! The check verifies certificates (reqwest's default); a failure there is no
//! proof of a dead stream. The M3U maturity round turns verification on for
//! mpv too. At most one check per channel and [`AT_ONCE`] at a time, on the
//! app's tokio runtime; the answers wait in a slot the app drains once per
//! frame. The app never asks the server of the stream that plays
//! ([`same_server`]): not when a suspect is confirmed, and not later either,
//! when the user moves to that server while its check waits or is on its way
//! ([`StreamChecker::forget_server`]). An address never reaches a log line or a
//! `Debug` print: a paid list's addresses hold credentials.

use parking_lot::Mutex;
use reqwest::header::HeaderValue;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

/// How long a check waits for the connection and the answer's headers.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks on their way at once.
pub const AT_ONCE: usize = 4;

/// What mpv sends as its User-Agent when a channel names none: the vendored
/// libmpv's default (`tests/test_mpv_smoke.rs` pins it). The check says the
/// same, so the server answers it as it answered the player.
pub const MPV_USER_AGENT: &str = "libmpv";

/// What a channel's address answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// 2xx or 3xx after the redirects: it answers again.
    Answers(u16),
    /// A status of 400 or more after the redirects.
    Status(u16),
    /// No connection: refused, reset while connecting, a name that does not
    /// resolve, a hang-up during an https handshake. (A handshake that fails
    /// with a TLS error, a certificate it cannot verify or plain HTTP on a TLS
    /// port, is [`Answer::Unchecked`]; a hang-up after the connection, for
    /// https after the handshake, is [`Answer::Failed`].)
    ConnectFailed,
    /// No headers within [`CHECK_TIMEOUT`].
    TimedOut,
    /// A reply it could not use: a redirect that never ends, an answer that is
    /// no HTTP it reads (Shoutcast's `ICY 200 OK`), a hang-up before any
    /// reply once the connection is made. Not found out, so no mark: the
    /// server did answer, or may have, and what the check cannot read is no
    /// proof of a dead stream.
    Failed,
    /// Not asked, or not found out, so no mark then (spec §4.2): no http(s)
    /// address, or no HTTP client; an address it cannot build a request for (a
    /// bad host or port) or a redirect to one that is no http(s); a header
    /// value that cannot be sent (a control character); or a certificate or
    /// TLS handshake it could not verify.
    Unchecked,
}

impl Answer {
    /// Whether the channel gets the mark: only on proof that its stream is
    /// dead (a status of 400 or more, no connection, no answer in time). Not
    /// when it answers again, and not when the check could not find out
    /// ([`Answer::Failed`], [`Answer::Unchecked`]): better a dead channel too
    /// many in the list than a good one lost (spec §4.2).
    pub fn is_dead(self) -> bool {
        matches!(
            self,
            Answer::Status(_) | Answer::ConnectFailed | Answer::TimedOut
        )
    }

    /// What the log says of it, never the address: "http 404", "connect
    /// failed", "timeout". `Unchecked` names both kinds of its causes (an
    /// address it cannot ask, a certificate it could not verify), as one
    /// answer cannot tell them apart.
    pub fn kind(self) -> String {
        match self {
            Answer::Answers(status) | Answer::Status(status) => format!("http {status}"),
            Answer::ConnectFailed => "connect failed".to_owned(),
            Answer::TimedOut => "timeout".to_owned(),
            Answer::Failed => "request failed".to_owned(),
            Answer::Unchecked => {
                "an address it cannot ask or a certificate it could not verify".to_owned()
            }
        }
    }
}

/// A check to make. No `Debug`: the address of a paid list holds credentials.
pub struct Check {
    pub stream_id: i64,
    /// For the app's log line.
    pub name: String,
    /// When the channel failed: its mark keeps this time.
    pub failed_unix: i64,
    /// What the channel plays, with its headers (`url|User-Agent=…`).
    pub url: String,
}

/// A check's answer, for the app; no address in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub stream_id: i64,
    pub name: String,
    pub failed_unix: i64,
    pub answer: Answer,
}

/// Where an address is: its scheme, its host in lower case and its port (the
/// scheme's default when it names none). What [`same_server`] compares.
type Server = (String, String, Option<u16>);

/// A check on its way.
struct Running {
    /// The ticket its answer must carry.
    ticket: u64,
    /// Aborted when the check is dropped.
    task: tokio::task::JoinHandle<()>,
    /// Where its address is (`None`: an address that cannot be read), for
    /// [`StreamChecker::forget_server`].
    server: Option<Server>,
    /// For the app's log, should the check be dropped.
    name: String,
}

/// The checks of the source in use.
pub struct StreamChecker {
    /// Built on the first check, not before: a session without one pays
    /// nothing for it.
    client: Option<reqwest::Client>,
    /// The client could not be built (warned once): no check asks then.
    client_failed: bool,
    /// Checks on their way, by stream id.
    in_flight: HashMap<i64, Running>,
    /// Checks waiting for one of the [`AT_ONCE`] places.
    waiting: VecDeque<Check>,
    next_ticket: u64,
    /// Answers that came, with their tickets, until `drain` takes them.
    done: Arc<Mutex<Vec<(u64, Checked)>>>,
}

impl Default for StreamChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamChecker {
    /// No check yet; nothing is built.
    pub fn new() -> Self {
        Self {
            client: None,
            client_failed: false,
            in_flight: HashMap::new(),
            waiting: VecDeque::new(),
            next_ticket: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Asks the address of `check` once, in the background: now when fewer
    /// than [`AT_ONCE`] run, else when a place frees. Not a second time while
    /// a check of that channel runs or waits.
    ///
    /// Needs a tokio runtime context, as it spawns the check: the app's main
    /// thread holds `rt.enter()` (`main.rs`), and a headless test enters one.
    pub fn start(&mut self, check: Check) {
        if self.checking(check.stream_id) {
            return;
        }
        self.waiting.push_back(check);
        self.fill();
    }

    /// The answers that came since the last call, of the checks not dropped
    /// since; their places go to checks that wait. Once per frame.
    ///
    /// Needs a tokio runtime context, as it spawns the checks that wait (see
    /// [`StreamChecker::start`]).
    pub fn drain(&mut self) -> Vec<Checked> {
        let arrived = std::mem::take(&mut *self.done.lock());
        let mut answers = Vec::new();
        for (ticket, checked) in arrived {
            let ours = self
                .in_flight
                .get(&checked.stream_id)
                .is_some_and(|running| running.ticket == ticket);
            if ours {
                self.in_flight.remove(&checked.stream_id);
                answers.push(checked);
            }
        }
        self.fill();
        answers
    }

    /// Drops channel `stream_id`'s check: it showed a picture, and its answer
    /// means nothing now. A place it held goes to the check that waits at
    /// once.
    ///
    /// Needs a tokio runtime context, as it may spawn that check (see
    /// [`StreamChecker::start`]).
    pub fn forget(&mut self, stream_id: i64) {
        if let Some(running) = self.in_flight.remove(&stream_id) {
            running.task.abort();
        }
        self.waiting.retain(|c| c.stream_id != stream_id);
        self.fill();
    }

    /// Drops every check, waiting or on its way, whose address is on the
    /// server of `url` (see [`same_server`]): the stream that plays from now on
    /// is there, and a paid list's portal allows one session per account, so
    /// the check would take that session or be refused and mark a channel that
    /// works (ruling 28b). The app calls it on every load, so a check that
    /// waited for a place, or has not answered yet, does not ask the server the
    /// user moved to. Gives the stream id and the name of each check it
    /// dropped, in the order of the ids, for the log; their answers are
    /// ignored. A place freed goes to the check that waits at once. Nothing to
    /// look at while no check runs or waits.
    ///
    /// Needs a tokio runtime context, as it may spawn that check (see
    /// [`StreamChecker::start`]).
    pub fn forget_server(&mut self, url: &str) -> Vec<(i64, String)> {
        let mut dropped = Vec::new();
        if self.in_flight.is_empty() && self.waiting.is_empty() {
            return dropped;
        }
        // An address that cannot be read is on no server, so never on this one.
        let Some(server) = server_of(url) else {
            return dropped;
        };
        self.in_flight.retain(|&stream_id, running| {
            if running.server.as_ref() != Some(&server) {
                return true;
            }
            running.task.abort();
            dropped.push((stream_id, std::mem::take(&mut running.name)));
            false
        });
        self.waiting.retain(|check| {
            if server_of(&check.url).as_ref() != Some(&server) {
                return true;
            }
            dropped.push((check.stream_id, check.name.clone()));
            false
        });
        dropped.sort_unstable_by_key(|&(stream_id, _)| stream_id);
        self.fill();
        dropped
    }

    /// Drops every check: another source is in use, and their answers are
    /// ignored.
    pub fn reset(&mut self) {
        for (_, running) in self.in_flight.drain() {
            running.task.abort();
        }
        self.waiting.clear();
    }

    /// Whether a check of channel `stream_id` runs or waits.
    pub fn checking(&self, stream_id: i64) -> bool {
        self.in_flight.contains_key(&stream_id)
            || self.waiting.iter().any(|c| c.stream_id == stream_id)
    }

    /// Checks on their way, not counting the ones that wait.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Starts the checks that wait while a place is free, so between two calls
    /// no check waits while a place is free, and the checks go in the order
    /// they were started.
    fn fill(&mut self) {
        while self.in_flight.len() < AT_ONCE {
            let Some(next) = self.waiting.pop_front() else {
                break;
            };
            self.launch(next);
        }
    }

    fn launch(&mut self, check: Check) {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let client = self.http_client();
        let done = self.done.clone();
        let stream_id = check.stream_id;
        let server = server_of(&check.url);
        let name = check.name.clone();
        let task = tokio::spawn(async move {
            let web = check.url.starts_with("http://") || check.url.starts_with("https://");
            let answer = match client.filter(|_| web) {
                Some(client) => ask(&client, &check.url).await,
                None => Answer::Unchecked,
            };
            let checked = Checked {
                stream_id: check.stream_id,
                name: check.name,
                failed_unix: check.failed_unix,
                answer,
            };
            done.lock().push((ticket, checked));
        });
        let running = Running {
            ticket,
            task,
            server,
            name,
        };
        self.in_flight.insert(stream_id, running);
    }

    /// The HTTP client, built on the first check. `None` when it cannot be
    /// built (warned once): the checks then answer `Unchecked`.
    fn http_client(&mut self) -> Option<reqwest::Client> {
        if self.client.is_none() && !self.client_failed {
            match http_client() {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    self.client_failed = true;
                    tracing::warn!(
                        "m3u: no HTTP client for the defect check ({e}) - no channel is marked"
                    );
                }
            }
        }
        self.client.clone()
    }
}

/// The check's HTTP client: [`CHECK_TIMEOUT`] for the connection and the
/// headers (the body is never read, so the limit ends there), redirects
/// followed, and no `Referer` of its own on a redirect (that would hand a
/// tokenized address on to another server). It verifies certificates
/// (reqwest's default). A failure is no proof of a dead stream, so it marks
/// nothing ([`Answer::Unchecked`]).
pub fn http_client() -> reqwest::Result<reqwest::Client> {
    client_with(CHECK_TIMEOUT)
}

/// [`http_client`] with a wait of `limit` for the connection and the headers:
/// a test makes it short.
fn client_with(limit: Duration) -> reqwest::Result<reqwest::Client> {
    builder(limit).build()
}

/// The check's client builder. Apart so that a test can read its two limits
/// off it: reqwest has no getter for them, but its `Debug` output names them.
fn builder(limit: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(limit)
        .timeout(limit)
        .referer(false)
}

/// Asks `url` once, with the headers its `|…` suffix names
/// ([`crate::m3u::split_headers`], as `player::loadfile_args` hands them to
/// mpv), and mpv's own User-Agent when it names none. The header values go as
/// their bytes, as mpv sends them, so a "Zürich-TV" User-Agent is no error; a
/// value that still cannot be sent (a control character) is not asked. Only
/// the status counts: the body (a live stream never ends) is not read. A
/// certificate or TLS handshake it could not verify is `Unchecked`, not dead.
pub async fn ask(client: &reqwest::Client, url: &str) -> Answer {
    let (address, headers) = crate::m3u::split_headers(url);
    let agent = headers.user_agent.as_deref().unwrap_or(MPV_USER_AGENT);
    let Ok(agent) = HeaderValue::from_bytes(agent.as_bytes()) else {
        return Answer::Unchecked;
    };
    let mut request = client
        .get(address)
        .header(reqwest::header::USER_AGENT, agent);
    if let Some(referrer) = &headers.referrer {
        let Ok(referrer) = HeaderValue::from_bytes(referrer.as_bytes()) else {
            return Answer::Unchecked;
        };
        request = request.header(reqwest::header::REFERER, referrer);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            if status >= 400 {
                Answer::Status(status)
            } else {
                Answer::Answers(status)
            }
        }
        Err(e) => answer_of_error(&e),
    }
}

/// What a request that failed answers. No headers in time is `TimedOut`. An
/// address it could not build the request for (a bad host or port) or a
/// redirect to one that is no http(s) (reqwest's "builder" errors) was not
/// asked: `Unchecked`. So is a certificate or TLS handshake it could not
/// verify, which is no proof of a dead stream; that holds for a TLS error
/// after the handshake too. Without a connection otherwise it is
/// `ConnectFailed` (refused, reset, a name that does not resolve, a hang-up
/// during an https handshake), and any other failure is `Failed`: a redirect
/// that never ends, a reply that is no HTTP it reads, a hang-up before any
/// reply once the connection is made.
fn answer_of_error(err: &reqwest::Error) -> Answer {
    if err.is_timeout() {
        Answer::TimedOut
    } else if err.is_builder() {
        Answer::Unchecked
    } else {
        answer_of_failure(err, err.is_connect())
    }
}

/// [`answer_of_error`] for a failure that is no timeout, read from its error
/// chain (`connect`: reqwest calls it a connection failure). Apart so that a
/// chain made by hand can test it: a `reqwest::Error` cannot be made by hand.
fn answer_of_failure(err: &(dyn Error + 'static), connect: bool) -> Answer {
    if tls_problem(err) {
        Answer::Unchecked
    } else if connect {
        Answer::ConnectFailed
    } else {
        Answer::Failed
    }
}

/// Whether a TLS problem is in the chain of `err`. tokio-rustls hands every
/// rustls handshake and certificate error on as an `io::Error` of kind
/// `InvalidData`. hyper-rustls wraps that one in an `io::Error` of its own
/// (`io::Error::other`), and `io::Error::source()` skips the error it wraps, so
/// a walk by `source()` alone never meets it: an `io::Error` is stepped into
/// through `get_ref()`. (Measured on the real chains of a self-signed
/// certificate and of plain HTTP where TLS is expected: reqwest, hyper-util's
/// "client error (Connect)", an `io::Error` of kind `Other`, and in it the
/// `InvalidData` one.)
fn tls_problem(err: &(dyn Error + 'static)) -> bool {
    let mut link = Some(err);
    while let Some(e) = link {
        link = match e.downcast_ref::<std::io::Error>() {
            Some(io) if io.kind() == std::io::ErrorKind::InvalidData => return true,
            Some(io) => io.get_ref().map(|inner| inner as &(dyn Error + 'static)),
            None => e.source(),
        };
    }
    false
}

/// Whether addresses `a` and `b` are on one server: the same scheme, host (in
/// any letter case) and port, the scheme's default port filled in (http 80,
/// https 443). The path, the query and the headers after a Kodi `|` do not
/// count. An address that cannot be read is on no server, so never on the
/// same one. The app asks no address on the server of the stream that plays:
/// a paid list's portal allows one session per account (ruling 28b).
pub fn same_server(a: &str, b: &str) -> bool {
    match (server_of(a), server_of(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The scheme, the host in lower case and the port of `url` (the scheme's
/// default when it names none).
fn server_of(url: &str) -> Option<Server> {
    let address = url.split('|').next().unwrap_or(url);
    let parsed = reqwest::Url::parse(address).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default();
    Some((parsed.scheme().to_owned(), host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::io::{Error as IoError, ErrorKind};

    /// One link of an error chain, as reqwest and hyper hand them on: it says
    /// little itself, its cause is in `source()`. (A `reqwest::Error` cannot be
    /// made by hand.)
    #[derive(Debug)]
    struct Link(Option<Box<dyn Error + Send + Sync>>);

    impl Link {
        /// A chain that ends in an `io::Error` of `kind`.
        fn ending_in(kind: ErrorKind, what: &str) -> Self {
            Link(Some(Box::new(IoError::new(kind, what.to_owned()))))
        }
    }

    impl fmt::Display for Link {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("error sending request")
        }
    }

    impl Error for Link {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.0
                .as_deref()
                .map(|cause| cause as &(dyn Error + 'static))
        }
    }

    #[test]
    fn a_certificate_it_could_not_verify_is_not_a_dead_stream() {
        // tokio-rustls hands every rustls handshake and certificate error on as
        // an io::Error of kind InvalidData.
        let chain = Link::ending_in(
            ErrorKind::InvalidData,
            "invalid peer certificate: UnknownIssuer",
        );
        let answer = answer_of_failure(&chain, true);
        assert_eq!(answer, Answer::Unchecked);
        assert!(!answer.is_dead(), "no proof, no mark");
    }

    #[test]
    fn the_tls_error_is_found_inside_the_io_error_that_wraps_it() {
        // The real chain (measured on a self-signed certificate, and on plain
        // HTTP where TLS is expected): hyper-rustls wraps tokio-rustls's
        // InvalidData error in `io::Error::other`, and `io::Error::source()`
        // skips the error it wraps, so a walk by `source()` alone never meets
        // it. The walk steps into an io::Error through `get_ref()`.
        let tls = IoError::new(
            ErrorKind::InvalidData,
            "invalid peer certificate: UnknownIssuer",
        );
        let connect = Link(Some(Box::new(IoError::other(tls))));
        let request = Link(Some(Box::new(connect)));
        assert_eq!(answer_of_failure(&request, true), Answer::Unchecked);
    }

    #[test]
    fn an_io_error_of_kind_invalid_data_needs_no_message_to_count() {
        let chain = Link(Some(Box::new(IoError::from(ErrorKind::InvalidData))));
        assert_eq!(answer_of_failure(&chain, true), Answer::Unchecked);
    }

    #[test]
    fn every_other_connection_failure_is_a_failed_connection() {
        for (kind, what) in [
            (ErrorKind::ConnectionRefused, "connection refused"),
            (ErrorKind::ConnectionReset, "connection reset"),
            (ErrorKind::Other, "failed to lookup address information"),
        ] {
            let answer = answer_of_failure(&Link::ending_in(kind, what), true);
            assert_eq!(answer, Answer::ConnectFailed, "{kind:?}");
            assert!(answer.is_dead(), "{kind:?}");
        }
        // An io::Error with nothing wrapped in it ends the walk too.
        let bare = Link(Some(Box::new(IoError::from(ErrorKind::ConnectionRefused))));
        assert_eq!(answer_of_failure(&bare, true), Answer::ConnectFailed);
    }

    #[test]
    fn a_chain_with_no_io_error_in_it_is_the_plain_answer() {
        let chain = Link(None);
        assert_eq!(answer_of_failure(&chain, true), Answer::ConnectFailed);
        assert_eq!(answer_of_failure(&chain, false), Answer::Failed);
    }

    #[test]
    fn a_failure_that_is_no_connection_failure_is_a_failed_request_unless_it_is_tls() {
        let reset = Link::ending_in(ErrorKind::ConnectionReset, "connection reset");
        assert_eq!(answer_of_failure(&reset, false), Answer::Failed);
        // A TLS alert on the first read, after the handshake (a server that
        // wants a client certificate): no connect error to reqwest, and no dead
        // stream either.
        let alert = Link::ending_in(ErrorKind::InvalidData, "received fatal alert");
        assert_eq!(answer_of_failure(&alert, false), Answer::Unchecked);
    }

    /// A server that answers every request with `status`, after `delay`.
    async fn answering(status: u16, delay: Duration) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(status).set_delay(delay))
            .mount(&server)
            .await;
        server
    }

    fn check(stream_id: i64, url: String) -> Check {
        Check {
            stream_id,
            name: format!("K{stream_id}"),
            failed_unix: 0,
            url,
        }
    }

    #[tokio::test]
    async fn an_answer_in_the_slot_of_a_forgotten_check_never_lands() {
        // The one case the ticket decides, and aborting the task cannot: the
        // first check's answer is in the slot when its channel is forgotten and
        // started again, so the stale answer is there to be taken for the
        // second check's.
        let first = answering(200, Duration::ZERO).await;
        let second = answering(404, Duration::from_millis(300)).await;
        let mut checker = StreamChecker::new();
        checker.start(check(1, format!("{}/1", first.uri())));
        // Wait until its answer is in the slot, without draining it.
        for _ in 0..1000 {
            if !checker.done.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            checker.done.lock().len(),
            1,
            "the first answer is in the slot"
        );
        checker.forget(1);
        checker.start(check(1, format!("{}/1", second.uri())));
        assert!(
            checker.drain().is_empty(),
            "the answer of the forgotten check landed"
        );
        assert!(checker.checking(1), "the second check is still on its way");
        // Its own answer follows.
        let mut got = Vec::new();
        for _ in 0..1000 {
            got.extend(checker.drain());
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].answer, Answer::Status(404));
        assert_eq!(checker.in_flight(), 0);
    }

    // ---- the server of the stream that plays from now on ----

    #[tokio::test]
    async fn forgetting_a_server_drops_its_checks_waiting_or_on_their_way() {
        // Six slow checks: four on their way, two waiting. 1 and 5 are on `here`,
        // the server of the stream that plays from now on; 2, 3, 4 and 6 are not.
        let here = answering(404, Duration::from_millis(300)).await;
        let elsewhere = answering(404, Duration::from_millis(300)).await;
        let mut checker = StreamChecker::new();
        let servers = [&here, &elsewhere, &elsewhere, &elsewhere, &here, &elsewhere];
        for (id, server) in (1..).zip(servers) {
            checker.start(check(id, format!("{}/{id}", server.uri())));
        }
        assert_eq!(checker.in_flight(), AT_ONCE);
        assert!(checker.checking(5) && checker.checking(6), "two wait");
        // What plays has headers after a Kodi `|`, and a path of its own.
        let playing = format!("{}/live/9.m3u8|User-Agent=Bongo%2F1.0", here.uri());
        let dropped = checker.forget_server(&playing);
        assert_eq!(dropped, [(1, "K1".to_owned()), (5, "K5".to_owned())]);
        assert!(!checker.checking(1) && !checker.checking(5));
        for id in [2, 3, 4, 6] {
            assert!(checker.checking(id), "{id} stays");
        }
        // The place of check 1 went to the check that waited, 6.
        assert_eq!(checker.in_flight(), AT_ONCE);
        // Their answers come, and only theirs.
        let mut got = Vec::new();
        for _ in 0..1000 {
            got.extend(checker.drain().into_iter().map(|checked| checked.stream_id));
            if !checker.checking(2)
                && !checker.checking(3)
                && !checker.checking(4)
                && !checker.checking(6)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        got.sort_unstable();
        assert_eq!(got, [2, 3, 4, 6]);
        // Check 5 never started: nothing asked its path.
        let asked = here.received_requests().await.unwrap();
        let paths: Vec<&str> = asked.iter().map(|request| request.url.path()).collect();
        assert!(!paths.contains(&"/5"), "{paths:?}");
    }

    #[tokio::test]
    async fn forgetting_a_server_no_check_is_on_changes_nothing() {
        let server = answering(404, Duration::from_millis(200)).await;
        let mut checker = StreamChecker::new();
        // Nothing runs: nothing to look at.
        assert!(checker.forget_server("https://x.example/a.m3u8").is_empty());
        checker.start(check(1, format!("{}/1", server.uri())));
        let port = server.address().port();
        let others = [
            "https://x.example/a.m3u8".to_owned(),
            // The same host on another port, and with the other scheme's port.
            format!("http://127.0.0.1:{}/1", port.wrapping_add(1)),
            "https://127.0.0.1/1".to_owned(),
            // No address: it is on no server, so never on this one.
            "not an address".to_owned(),
            String::new(),
        ];
        for other in others {
            assert!(checker.forget_server(&other).is_empty(), "{other:?}");
            assert!(checker.checking(1), "{other:?}");
        }
        // Its own server, whatever the path: gone.
        let own = format!("{}/anything", server.uri());
        assert_eq!(checker.forget_server(&own), [(1, "K1".to_owned())]);
        assert!(!checker.checking(1));
    }

    #[tokio::test]
    async fn an_answer_already_in_the_slot_of_a_forgotten_server_never_lands() {
        let server = answering(404, Duration::ZERO).await;
        let mut checker = StreamChecker::new();
        checker.start(check(1, format!("{}/1", server.uri())));
        for _ in 0..1000 {
            if !checker.done.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(checker.done.lock().len(), 1, "the answer is in the slot");
        assert_eq!(checker.forget_server(&server.uri()), [(1, "K1".to_owned())]);
        assert!(
            checker.drain().is_empty(),
            "the answer of a dropped check landed"
        );
        assert_eq!(checker.in_flight(), 0);
    }

    // ---- the 5 s limit is the real client's ----

    #[test]
    fn the_client_waits_check_timeout_for_the_connection_and_for_the_headers() {
        // reqwest has no getter for its limits, but its Debug output names
        // them: both calls must be there, with the limit that was handed in.
        let limit = format!("{CHECK_TIMEOUT:?}");
        let pretty = format!("{:#?}", builder(CHECK_TIMEOUT));
        for name in ["connect_timeout", "timeout"] {
            let field = format!("{name}: {limit},");
            assert!(
                pretty.lines().any(|l| l.trim() == field),
                "{field}\n{pretty}"
            );
        }
        // And http_client() is that client.
        let built = builder(CHECK_TIMEOUT).build().unwrap();
        assert_eq!(
            format!("{:?}", http_client().unwrap()),
            format!("{built:?}")
        );
    }

    #[tokio::test]
    async fn no_headers_within_the_clients_limit_is_a_timeout() {
        let server = answering(200, Duration::from_secs(3)).await;
        // The check's client waits CHECK_TIMEOUT (5 s); this one, made the same
        // way, 200 ms.
        let client = client_with(Duration::from_millis(200)).unwrap();
        let started = std::time::Instant::now();
        let answer = ask(&client, &format!("{}/slow", server.uri())).await;
        assert_eq!(answer, Answer::TimedOut);
        assert!(answer.is_dead());
        assert_eq!(answer.kind(), "timeout");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the limit did not end the wait"
        );
    }
}
