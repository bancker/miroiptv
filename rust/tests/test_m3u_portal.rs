use bongo_player::m3u::split_headers;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::{Catalog, Portal, PortalError};
use bongo_player::sources::SourceKind;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST: &str = "#EXTM3U x-tvg-url=\"https://epg.example/guide.xml.gz\"\n\
#EXTINF:-1 tvg-id=\"Een.nl\" tvg-logo=\"https://logos.example/een.png\" group-title=\"Algemeen\",Kanaal Een\n\
https://a.example/een.m3u8\n\
#EXTINF:-1 group-title=\"Algemeen\",Kanaal Twee\n\
#EXTVLCOPT:http-user-agent=Bongo/1.0\n\
https://b.example/twee.m3u8\n";

const OTHER: &str = "#EXTM3U\n#EXTINF:-1,Kanaal Drie\nhttps://c.example/drie.m3u8\n";

async fn serving(body: &str, status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv.m3u"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    server
}

fn names(catalog: &Catalog) -> Vec<&str> {
    catalog.live.iter().map(|c| c.name.as_str()).collect()
}

#[tokio::test]
async fn a_url_source_downloads_saves_and_plays_its_list() {
    let server = serving(LIST, 200).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache").join("m3u-2.m3u");
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache.clone());
    assert_eq!(portal.kind(), SourceKind::M3u);
    assert!(portal.saved_catalog().await.is_none(), "nothing saved yet");
    let catalog = portal.fetch_catalog().await.unwrap();
    assert_eq!(names(&catalog), ["Kanaal Een", "Kanaal Twee"]);
    assert!(catalog.movies.is_empty() && catalog.series.is_empty());
    assert_eq!(
        std::fs::read_to_string(&cache).unwrap(),
        LIST,
        "kept as it came"
    );
    // What each channel plays, with its headers; and back again.
    let twee = catalog.live[1].stream_id;
    let url = portal.live_stream_url(twee);
    let (address, headers) = split_headers(&url);
    assert_eq!(address, "https://b.example/twee.m3u8");
    assert_eq!(headers.user_agent.as_deref(), Some("Bongo/1.0"));
    assert_eq!(portal.live_stream_id(&url), Some(twee));
    // The list was asked for as the portal is: with the app's User-Agent.
    let asked = &server.received_requests().await.unwrap()[0];
    let agent = asked.headers.get("user-agent").unwrap().to_str().unwrap();
    assert!(agent.starts_with("BongoPlayer/"), "{agent}");
}

#[tokio::test]
async fn the_saved_list_shows_first_and_the_same_download_is_nothing_new() {
    let server = serving(LIST, 200).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, LIST).unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache.clone());
    let saved = portal.saved_catalog().await.expect("the saved list");
    assert_eq!(names(&saved), ["Kanaal Een", "Kanaal Twee"]);
    let een = saved.live[0].stream_id;
    assert!(
        !portal.live_stream_url(een).is_empty(),
        "it plays before the download"
    );
    assert!(matches!(
        portal.fetch_catalog().await,
        Err(PortalError::Unchanged)
    ));
    // Another list comes: it replaces the saved one.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/tv.m3u"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OTHER))
        .mount(&server)
        .await;
    let fresh = portal.fetch_catalog().await.unwrap();
    assert_eq!(names(&fresh), ["Kanaal Drie"]);
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), OTHER);
    assert!(portal.live_stream_url(een).is_empty(), "Een is gone");
}

#[tokio::test]
async fn a_refused_download_keeps_the_saved_file() {
    let server = serving("Service Unavailable", 503).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, LIST).unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache.clone());
    assert!(portal.saved_catalog().await.is_some());
    assert!(matches!(
        portal.fetch_catalog().await,
        Err(PortalError::Status(503))
    ));
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), LIST);
}

#[tokio::test]
async fn a_failed_download_names_no_address() {
    // Nothing listens on port 1: the request itself fails.
    let dir = tempfile::tempdir().unwrap();
    let address = "http://bob:s3cr3t@127.0.0.1:1/private/tv.m3u?token=t0k3n";
    let portal = M3uPortal::new(2, address, dir.path().join("m3u-2.m3u"));
    let error = portal.fetch_catalog().await.unwrap_err().to_string();
    for secret in ["bob", "s3cr3t", "t0k3n", "private"] {
        assert!(!error.contains(secret), "{secret} in {error:?}");
    }
}

#[tokio::test]
async fn an_html_page_with_200_is_no_list_and_the_saved_file_stays() {
    let server = serving(
        "<!DOCTYPE html><html><body>Captive portal</body></html>",
        200,
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, LIST).unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache.clone());
    let saved = portal.saved_catalog().await.unwrap();
    assert!(matches!(
        portal.fetch_catalog().await,
        Err(PortalError::NotAList)
    ));
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), LIST);
    let een = saved.live[0].stream_id;
    assert!(
        !portal.live_stream_url(een).is_empty(),
        "the saved list still plays"
    );
}

#[tokio::test]
async fn a_file_source_is_read_as_it_is_without_a_saved_copy() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mijn lijst.m3u");
    std::fs::write(&file, LIST).unwrap();
    let cache = dir.path().join("m3u-3.m3u");
    let portal = M3uPortal::new(3, &file.display().to_string(), cache.clone());
    assert!(portal.saved_catalog().await.is_none());
    let catalog = portal.fetch_catalog().await.unwrap();
    assert_eq!(names(&catalog), ["Kanaal Een", "Kanaal Twee"]);
    assert!(!cache.exists());
}

#[tokio::test]
async fn a_file_that_cannot_be_read_names_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("weg.m3u");
    let portal = M3uPortal::new(3, &file.display().to_string(), dir.path().join("m3u-3.m3u"));
    match portal.fetch_catalog().await {
        Err(PortalError::File(message)) => assert!(message.contains("weg.m3u"), "{message}"),
        other => panic!("{:?}", other.map(|c| c.live.len())),
    }
}

#[tokio::test]
async fn an_m3u_portal_has_no_epg_films_series_or_catch_up() {
    let (portal, catalog) = M3uPortal::from_text(LIST).unwrap();
    let id = catalog.live[0].stream_id;
    assert!(portal.fetch_epg(id).await.unwrap().entries().is_empty());
    assert!(portal.fetch_day_epg(id).await.unwrap().entries().is_empty());
    assert!(portal.fetch_series_episodes(1).await.unwrap().is_empty());
    assert_eq!(portal.movie_stream_url(1, "mp4"), "");
    assert_eq!(portal.series_stream_url("1", "mp4"), "");
    assert_eq!(portal.catchup_url(id, chrono::Utc::now(), 30), "");
    assert_eq!(
        portal.live_stream_url(12345),
        "",
        "an id the list does not have"
    );
    assert_eq!(
        portal.live_stream_id("https://elsewhere.example/x.m3u8"),
        None
    );
}

#[test]
fn round_two_keeps_the_logo_and_the_guide_url() {
    let (portal, catalog) = M3uPortal::from_text(LIST).unwrap();
    assert_eq!(
        portal.logo(catalog.live[0].stream_id).as_deref(),
        Some("https://logos.example/een.png")
    );
    assert_eq!(
        portal.epg_url().as_deref(),
        Some("https://epg.example/guide.xml.gz")
    );
    assert!(M3uPortal::from_text("<html></html>").is_none());
}

// ---- beyond the brief's nine: the log, the credentials, the odd lists ----

/// A log sink the test can read back.
#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What this thread logs from `start` until the capture is dropped (a
/// `#[tokio::test]` runs its futures on its own thread). The second dispatcher
/// keeps another test's thread, which has no subscriber, from silencing a log
/// line for this one: tracing works a line's interest out from whoever hits it
/// first, unless more than one dispatcher is registered.
struct LogCapture {
    buffer: LogBuffer,
    _second: tracing::Dispatch,
    _logging: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    fn start() -> Self {
        let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let buffer = LogBuffer::default();
        let sink = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        Self {
            buffer,
            _second: second,
            _logging: tracing::subscriber::set_default(subscriber),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.0.lock().unwrap()).into_owned()
    }
}

/// The log line that holds `start`, from `start` on; it must end in " ms" with
/// a number of milliseconds before it.
fn timed_line<'a>(log: &'a str, start: &str) -> &'a str {
    let line = log
        .lines()
        .find_map(|l| l.find(start).map(|at| &l[at..]))
        .unwrap_or_else(|| panic!("no line {start:?} in:\n{log}"));
    let millis = line[start.len()..].strip_suffix(" ms");
    assert!(
        millis.is_some_and(|m| m.parse::<u64>().is_ok()),
        "{line:?} does not end in a number of ms"
    );
    line
}

#[tokio::test]
async fn the_log_lines_read_as_the_scenarios_grep_them() {
    let server = serving(LIST, 200).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, LIST).unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
    let log = LogCapture::start();
    portal.saved_catalog().await.expect("the saved list");
    assert!(matches!(
        portal.fetch_catalog().await,
        Err(PortalError::Unchanged)
    ));
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/tv.m3u"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OTHER))
        .mount(&server)
        .await;
    portal.fetch_catalog().await.unwrap();
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/tv.m3u"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    assert!(matches!(
        portal.fetch_catalog().await,
        Err(PortalError::Status(503))
    ));
    let text = log.text();
    timed_line(&text, "m3u: saved list of source 2 (2 channels, 0 KB) in ");
    timed_line(&text, "m3u: list of source 2 read (1 channels, 0 KB) in ");
    assert!(
        text.lines()
            .any(|l| l.ends_with("m3u: list of source 2 unchanged")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.ends_with("m3u: list of source 2 not read: http status 503")),
        "{text}"
    );
}

#[tokio::test]
async fn a_failed_download_logs_no_address() {
    // Nothing listens on port 1: the request itself fails.
    let dir = tempfile::tempdir().unwrap();
    let address = "http://bob:s3cr3t@127.0.0.1:1/private/tv.m3u?token=t0k3n";
    let portal = M3uPortal::new(2, address, dir.path().join("m3u-2.m3u"));
    let log = LogCapture::start();
    assert!(portal.fetch_catalog().await.is_err());
    let text = log.text();
    assert!(text.contains("m3u: list of source 2 not read: "), "{text}");
    for secret in ["bob", "s3cr3t", "t0k3n", "private"] {
        assert!(!text.contains(secret), "{secret} in {text:?}");
    }
}

#[tokio::test]
async fn a_mistyped_web_address_is_read_as_a_file_and_names_no_password() {
    // "htps" is no web address, so it is taken for a file that is not there.
    let dir = tempfile::tempdir().unwrap();
    let address = "htps://bob:s3cr3t@host.example/tv.m3u?token=t0k3n";
    let portal = M3uPortal::new(3, address, dir.path().join("m3u-3.m3u"));
    let log = LogCapture::start();
    match portal.fetch_catalog().await {
        Err(PortalError::File(message)) => {
            for secret in ["bob", "s3cr3t", "t0k3n"] {
                assert!(!message.contains(secret), "{secret} in {message:?}");
            }
        }
        other => panic!("{:?}", other.map(|c| c.live.len())),
    }
    let text = log.text();
    for secret in ["bob", "s3cr3t", "t0k3n"] {
        assert!(!text.contains(secret), "{secret} in {text:?}");
    }
}

#[test]
fn an_address_listed_twice_answers_with_the_same_channel_every_time() {
    let list = "#EXTM3U\n\
#EXTINF:-1,Kanaal Een\nhttps://a.example/een.m3u8\n\
#EXTINF:-1,Kanaal Een HD\nhttps://a.example/een.m3u8\n";
    let (_, catalog) = M3uPortal::from_text(list).unwrap();
    let lowest = catalog.live.iter().map(|c| c.stream_id).min().unwrap();
    // Each portal has a hash map of its own, with an order of its own.
    for _ in 0..24 {
        let (portal, _) = M3uPortal::from_text(list).unwrap();
        assert_eq!(
            portal.live_stream_id("https://a.example/een.m3u8"),
            Some(lowest)
        );
    }
}

#[tokio::test]
async fn a_saved_file_that_is_no_list_shows_nothing_and_the_download_replaces_it() {
    let server = serving(LIST, 200).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, "<html>Captive portal</html>").unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache.clone());
    assert!(portal.saved_catalog().await.is_none());
    let catalog = portal.fetch_catalog().await.unwrap();
    assert_eq!(names(&catalog), ["Kanaal Een", "Kanaal Twee"]);
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), LIST);
}
