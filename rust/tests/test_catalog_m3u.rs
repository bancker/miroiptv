use bongo_player::catalog::{guide_channels_of, CatalogNotice, CatalogStatus, CatalogStore};
use bongo_player::epg::Epg;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::{Catalog, Episode, LiveChannel, Portal, PortalError};
use bongo_player::sources::SourceKind;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST: &str = "#EXTM3U\n\
#EXTINF:-1,NL | NPO 1\nhttps://a.example/npo1.m3u8\n\
#EXTINF:-1,NL | NPO 1 HD\nhttps://a.example/npo1hd.m3u8\n\
#EXTINF:-1,NL | RTL 4\nhttps://a.example/rtl4.m3u8\n";

const OTHER: &str = "#EXTM3U\n#EXTINF:-1,Kanaal Drie\nhttps://c.example/drie.m3u8\n";

fn store_of(text: &str) -> CatalogStore {
    let (portal, catalog) = M3uPortal::from_text(text).unwrap();
    CatalogStore::from_catalog(Arc::new(portal), catalog)
}

fn names(store: &CatalogStore) -> Vec<String> {
    store.zap_channels().into_iter().map(|c| c.name).collect()
}

/// Waits (2 s at most) for what the background fetch does.
async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// An M3U store at a URL served by `server`, with `saved` as its saved list.
fn url_store(server: &MockServer, dir: &std::path::Path, saved: Option<&str>) -> Arc<CatalogStore> {
    let cache = dir.join("m3u-2.m3u");
    if let Some(text) = saved {
        std::fs::write(&cache, text).unwrap();
    }
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
    Arc::new(CatalogStore::new(Arc::new(portal)))
}

async fn server(status: u16, body: &str, delay_ms: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_string(body)
                .set_delay(Duration::from_millis(delay_ms)),
        )
        .mount(&server)
        .await;
    server
}

#[test]
fn an_m3u_list_zaps_in_file_order_and_merges_nothing() {
    let store = store_of(LIST);
    assert_eq!(store.kind(), SourceKind::M3u);
    // An Xtream catalog would merge NPO 1 and NPO 1 HD into one entry.
    assert_eq!(names(&store), ["NL | NPO 1", "NL | NPO 1 HD", "NL | RTL 4"]);
}

#[test]
fn an_m3u_channel_has_no_quality_ladder_and_is_never_adult() {
    let store = store_of(LIST);
    let npo1 = store.zap_channels()[0].stream_id;
    assert!(store.quality_nav(npo1).ladder.is_empty());
    assert!(!store.is_adult_url("http://h/live/u/p/3.m3u8"));
    store.set_adult_unlocked(true);
    assert_eq!(store.zap_channels().len(), 3, "the PIN changes nothing");
}

#[tokio::test]
async fn a_url_source_shows_its_saved_list_then_the_fresh_one() {
    let server = server(200, OTHER, 400).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), Some(LIST));
    store.spawn_fetch();
    wait_for("the saved list", || store.is_loaded()).await;
    assert_eq!(
        names(&store).len(),
        3,
        "the saved list, before the download"
    );
    assert_eq!(store.status(), CatalogStatus::Loaded);
    wait_for("the fresh list", || names(&store) == ["Kanaal Drie"]).await;
    assert_eq!(store.revision(), 2);
    assert_eq!(store.take_notice(), None);
}

#[tokio::test]
async fn the_same_list_again_changes_nothing() {
    let server = server(200, LIST, 0).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), Some(LIST));
    store.spawn_fetch();
    wait_for("the saved list", || store.is_loaded()).await;
    // The download that follows is the same bytes: give it time to come and go.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let asked = server.received_requests().await.unwrap_or_default();
    assert_eq!(asked.len(), 1, "the download happened");
    assert_eq!(store.status(), CatalogStatus::Loaded);
    assert_eq!(store.revision(), 1, "only the saved list was put");
    assert_eq!(store.take_notice(), None);
}

#[tokio::test]
async fn a_failed_download_keeps_the_saved_list_and_says_it_was_not_updated() {
    let server = server(503, "busy", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), Some(LIST));
    store.spawn_fetch();
    wait_for("the notice", || {
        store.status() == CatalogStatus::Loaded
            && store.take_notice() == Some(CatalogNotice::NotUpdated)
    })
    .await;
    assert_eq!(names(&store).len(), 3);
    assert_eq!(store.take_notice(), None, "told once");
}

#[tokio::test]
async fn an_html_page_with_200_keeps_the_saved_list_and_says_so() {
    let server = server(200, "<html><body>Captive portal</body></html>", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), Some(LIST));
    store.spawn_fetch();
    wait_for("the notice", || {
        store.take_notice() == Some(CatalogNotice::NotAList)
    })
    .await;
    assert_eq!(store.status(), CatalogStatus::Loaded);
    assert_eq!(names(&store).len(), 3);
}

#[tokio::test]
async fn without_a_saved_list_a_failed_download_is_the_empty_state() {
    let server = server(503, "busy", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), None);
    store.spawn_fetch();
    wait_for("the failure", || {
        matches!(store.status(), CatalogStatus::Failed(_))
    })
    .await;
    assert_eq!(
        store.status(),
        CatalogStatus::Failed("http status 503".into())
    );
    assert!(!store.is_loaded());
    assert_eq!(store.take_notice(), None, "the empty state says it");
}

#[tokio::test]
async fn without_a_saved_list_an_html_page_is_the_empty_state_and_says_so() {
    let server = server(200, "<!DOCTYPE html>", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let store = url_store(&server, dir.path(), None);
    store.spawn_fetch();
    wait_for("the failure", || {
        matches!(store.status(), CatalogStatus::Failed(_))
    })
    .await;
    assert_eq!(
        store.status(),
        CatalogStatus::Failed("not a channel list".into())
    );
    assert_eq!(store.take_notice(), Some(CatalogNotice::NotAList));
}

// ---- beyond the brief's eight: the refetch, and Xtream as it was ----

#[tokio::test]
async fn f5_on_a_shown_list_replaces_it_or_keeps_it_with_a_notice() {
    // The state after the first fetch: the list shows, and the portal knows it
    // as its saved one. A refetch has a list showing already, so it does not
    // read the saved file again.
    let server = server(200, OTHER, 0).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("m3u-2.m3u");
    std::fs::write(&cache, LIST).unwrap();
    let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
    let saved = portal.saved_catalog().await.expect("the saved list");
    let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), saved));
    assert_eq!(names(&store).len(), 3);
    assert_eq!(store.revision(), 1);

    // A changed list replaces the one that shows: one more revision, no more.
    store.spawn_fetch();
    wait_for("the fresh list", || names(&store) == ["Kanaal Drie"]).await;
    assert_eq!(store.revision(), 2, "the saved file was not put again");
    assert_eq!(store.status(), CatalogStatus::Loaded);
    assert_eq!(store.take_notice(), None);

    // The provider is down: the list that shows stays, and the app is told.
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    store.spawn_fetch();
    wait_for("the notice", || {
        store.take_notice() == Some(CatalogNotice::NotUpdated)
    })
    .await;
    assert_eq!(store.status(), CatalogStatus::Loaded);
    assert_eq!(names(&store), ["Kanaal Drie"]);
    assert_eq!(store.revision(), 2);
}

/// An Xtream portal (the default kind) that answers its fetches from a script.
struct ScriptedPortal(std::sync::Mutex<VecDeque<Result<Catalog, PortalError>>>);

impl ScriptedPortal {
    fn new(answers: impl IntoIterator<Item = Result<Catalog, PortalError>>) -> Self {
        Self(std::sync::Mutex::new(answers.into_iter().collect()))
    }
}

#[async_trait::async_trait]
impl Portal for ScriptedPortal {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .expect("one answer per fetch")
    }
    async fn fetch_epg(&self, _: i64) -> Result<Epg, PortalError> {
        Ok(Epg::new(Vec::new()))
    }
    async fn fetch_day_epg(&self, _: i64) -> Result<Epg, PortalError> {
        Ok(Epg::new(Vec::new()))
    }
    async fn fetch_series_episodes(&self, _: i64) -> Result<Vec<Episode>, PortalError> {
        Ok(Vec::new())
    }
    fn live_stream_url(&self, id: i64) -> String {
        format!("http://h/live/u/p/{id}.m3u8")
    }
    fn live_stream_id(&self, _: &str) -> Option<i64> {
        None
    }
    fn movie_stream_url(&self, _: i64, _: &str) -> String {
        String::new()
    }
    fn series_stream_url(&self, _: &str, _: &str) -> String {
        String::new()
    }
    fn catchup_url(&self, _: i64, _: chrono::DateTime<chrono::Utc>, _: u32) -> String {
        String::new()
    }
}

fn xtream_catalog() -> Catalog {
    let channel = |stream_id, name: &str| LiveChannel {
        stream_id,
        name: name.into(),
        category_id: None,
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult: false,
        stream_icon: None,
    };
    Catalog {
        live: vec![
            channel(1, "NL | NPO 1 HD"),
            channel(2, "NL | NPO 1 FHD"),
            channel(3, "NL | RTL 4"),
        ],
        ..Catalog::default()
    }
}

#[tokio::test]
async fn an_xtream_catalog_still_merges_and_a_failed_refetch_stays_as_it_was() {
    // The branches the M3U list takes must leave the portal's alone: it merges
    // variants, and a fetch that fails is the status, with no notice, also
    // when a catalog shows (the catalog it had stays).
    let portal = ScriptedPortal::new([Ok(xtream_catalog()), Err(PortalError::Status(503))]);
    let store = Arc::new(CatalogStore::new(Arc::new(portal)));
    assert_eq!(store.kind(), SourceKind::Xtream);
    store.spawn_fetch();
    wait_for("the catalog", || store.status() == CatalogStatus::Loaded).await;
    assert_eq!(names(&store), ["NL | NPO 1 FHD", "NL | RTL 4"]);

    store.spawn_fetch();
    wait_for("the failure", || {
        matches!(store.status(), CatalogStatus::Failed(_))
    })
    .await;
    assert_eq!(
        store.status(),
        CatalogStatus::Failed("http status 503".into())
    );
    assert!(store.is_loaded(), "the catalog it had stays");
    assert_eq!(names(&store).len(), 2);
    assert_eq!(store.take_notice(), None, "a notice is an M3U list's");
}

#[test]
fn an_m3u_catalog_says_not_adult_by_its_kind_whatever_the_data_says() {
    // The reader flags nothing, so "not adult" would come out of the data too;
    // a flag that did get in must not change the answer: only an Xtream portal
    // has adult content. The Xtream store proves the flag would count.
    let flagged = || {
        let (portal, mut catalog) = M3uPortal::from_text(LIST).unwrap();
        catalog.live[0].is_adult = true;
        (portal, catalog)
    };
    let (portal, catalog) = flagged();
    let url = format!("http://h/live/u/p/{}.m3u8", catalog.live[0].stream_id);
    let m3u = CatalogStore::from_catalog(Arc::new(portal), catalog);
    assert!(!m3u.is_adult_url(&url));

    let (_, catalog) = flagged();
    let xtream = CatalogStore::from_catalog(Arc::new(ScriptedPortal::new([])), catalog);
    assert!(xtream.is_adult_url(&url), "the flag counts for Xtream");
}

/// A list of `n` channels in iptv-org's shape: a tvg-id, a group and a logo
/// each, and the quality in the name.
fn list_of(n: usize) -> String {
    let mut text = String::from("#EXTM3U\n");
    for i in 0..n {
        text.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"Kanaal{i}.xx@SD\" tvg-logo=\"https://logos.example/{i}.png\" \
             group-title=\"Groep {}\",Kanaal {i} (1080p)\nhttps://cdn{}.example.net/live/{i}/index.m3u8\n",
            i % 183,
            i % 9
        ));
    }
    text
}

/// Calls `work` `n` times and prints the first call (as a start makes it), the
/// best and the mean, in ms. It asserts no speed: a busy machine would fail it.
fn print_timing(what: &str, n: usize, mut work: impl FnMut()) {
    let ms: Vec<f64> = (0..n)
        .map(|_| {
            let t0 = std::time::Instant::now();
            work();
            t0.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let best = ms.iter().copied().fold(f64::MAX, f64::min);
    let mean = ms.iter().sum::<f64>() / n as f64;
    println!(
        "{what}: first {:.3} ms, best {best:.3} ms, mean {mean:.3} ms ({n} calls)",
        ms[0]
    );
}

/// Timed by hand: what an M3U catalog of iptv-org's size costs where it is
/// stored (the guide's merge, once for every catalog put: the saved list and
/// the fresh one) and per up/down press (the zap list: every key repeat clones
/// the whole list).
/// `cargo test --release --test test_catalog_m3u -- --ignored --nocapture`
#[test]
#[ignore]
fn an_eleven_thousand_channel_m3u_catalog_timed() {
    let (portal, catalog) = M3uPortal::from_text(&list_of(11_262)).unwrap();
    assert_eq!(catalog.live.len(), 11_262);
    print_timing("guide_channels_of", 20, || {
        let guide = std::hint::black_box(guide_channels_of(std::hint::black_box(&catalog.live)));
        assert!(guide.is_empty(), "an M3U list has no catch-up");
    });
    let store = CatalogStore::from_catalog(Arc::new(portal), catalog);
    print_timing("zap_channels", 100, || {
        assert_eq!(std::hint::black_box(store.zap_channels()).len(), 11_262);
    });
}
