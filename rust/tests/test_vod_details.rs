//! A film's and a series' details (vod/details.rs, spec 2026-10-15 §4.3):
//! asked when a page opens, kept as a file, fresh for a day (series) or 30
//! days (films), two at a time with the newest first.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::Portal;
use bongo_player::vod::details::{dir_for, file_name, DetailState, DetailStore, SERIES_FRESH_SECS};
use bongo_player::vod::{Kind, TitleId};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::query_param;
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: i64 = 1_792_022_400;
const DAY: i64 = 86_400;

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

fn series(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

fn portal_of(server: &MockServer) -> Arc<dyn Portal> {
    let uri = url::Url::parse(&server.uri()).unwrap();
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "secret".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    }))
}

/// The portal answers a film's details with this plot, after `delay_ms`.
async fn film_answers(server: &MockServer, id: i64, plot: &str, delay_ms: u64) {
    let body = format!(
        r#"{{"info":{{"plot":"{plot}","direct_source":"http://h/movie/u/secret/{id}.mkv"}}}}"#
    );
    Mock::given(query_param("action", "get_vod_info"))
        .and(query_param("vod_id", id.to_string().as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .set_delay(Duration::from_millis(delay_ms)),
        )
        .mount(server)
        .await;
}

/// Lets the store take in answers until `done` says so (2 s at most).
async fn drain_until(store: &mut DetailStore, mut done: impl FnMut(&DetailStore) -> bool) {
    for _ in 0..400 {
        store.drain(NOW);
        if done(store) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the answers never came");
}

fn plot(state: DetailState) -> Option<(String, bool)> {
    match state {
        DetailState::Ready {
            details,
            refreshing,
        } => Some((details.plot.clone(), refreshing)),
        _ => None,
    }
}

/// How many times the portal was asked for these details.
async fn asked(server: &MockServer, action: &str) -> usize {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| {
            r.url
                .query()
                .unwrap_or("")
                .contains(&format!("action={action}"))
        })
        .count()
}

#[tokio::test]
async fn a_page_asks_once_and_keeps_the_answer_as_a_file_without_the_account() {
    let server = MockServer::start().await;
    film_answers(&server, 5001, "A hacker.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    assert_eq!(store.get(film(5001)), DetailState::Loading, "never asked");
    store.request(film(5001), NOW);
    assert_eq!(store.get(film(5001)), DetailState::Loading);
    drain_until(&mut store, |s| plot(s.get(film(5001))).is_some()).await;
    assert_eq!(
        plot(store.get(film(5001))),
        Some(("A hacker.".into(), false))
    );
    let text = std::fs::read_to_string(dir.path().join(file_name(film(5001)))).unwrap();
    assert!(text.contains("A hacker."));
    assert!(
        !text.contains("secret") && !text.contains("direct_source"),
        "{text}"
    );
    // A second page open within the month: the file, no question.
    let mut again = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    again.request(film(5001), NOW + 29 * DAY);
    assert_eq!(
        plot(again.get(film(5001))),
        Some(("A hacker.".into(), false)),
        "in the same call"
    );
    assert_eq!(again.in_flight(), 0);
    assert_eq!(asked(&server, "get_vod_info").await, 1);
}

#[tokio::test]
async fn an_old_file_shows_while_the_portal_is_asked_again() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"info":{"plot":"New."}}"#))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file_name(series(4321)));
    let old = serde_json::json!({"version": 1, "fetched_at": NOW - SERIES_FRESH_SECS - 1, "details": {"plot": "Old."}});
    std::fs::write(&path, old.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(4321), NOW);
    assert_eq!(
        plot(store.get(series(4321))),
        Some(("Old.".into(), true)),
        "older than a day"
    );
    drain_until(&mut store, |s| {
        plot(s.get(series(4321))) == Some(("New.".into(), false))
    })
    .await;
}

#[tokio::test]
async fn a_series_file_of_less_than_a_day_and_a_film_file_of_less_than_a_month_are_fresh() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (id, age) in [
        (series(1), SERIES_FRESH_SECS),
        (film(2), 30 * DAY),
        (film(3), -DAY),
    ] {
        let file = serde_json::json!({"version": 1, "fetched_at": NOW - age, "details": {"plot": "Kept."}});
        std::fs::write(dir.path().join(file_name(id)), file.to_string()).unwrap();
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in [series(1), film(2), film(3)] {
        store.request(id, NOW);
        assert_eq!(plot(store.get(id)), Some(("Kept.".into(), false)), "{id:?}");
    }
    assert_eq!(store.in_flight(), 0, "a time from the future counts as now");
}

#[tokio::test]
async fn two_at_a_time_and_the_newest_request_first() {
    let server = MockServer::start().await;
    // 1 and 2 answer apart, so 4 and 3 start in two drains, in that order.
    for id in 1..=4 {
        let delay = if id == 2 { 600 } else { 300 };
        film_answers(&server, id, &format!("Film {id}."), delay).await;
    }
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=4 {
        store.request(film(id), NOW);
    }
    assert_eq!(store.in_flight(), 2);
    drain_until(&mut store, |s| {
        (1..=4).all(|id| plot(s.get(film(id))).is_some())
    })
    .await;
    let order: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| {
            r.url
                .query_pairs()
                .find(|(k, _)| k == "vod_id")
                .map(|(_, v)| v.into_owned())
        })
        .collect();
    assert_eq!(
        &order[2..],
        ["4", "3"],
        "the newest waiting request first: {order:?}"
    );
}

#[tokio::test]
async fn a_failed_fetch_without_a_file_says_so_and_enter_asks_again() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    film_answers(&server, 7, "Second time.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(film(7), NOW);
    drain_until(&mut store, |s| s.get(film(7)) == DetailState::Failed).await;
    store.request(film(7), NOW);
    assert_eq!(
        store.get(film(7)),
        DetailState::Failed,
        "opening again asks nothing"
    );
    store.retry(film(7), NOW);
    drain_until(&mut store, |s| plot(s.get(film(7))).is_some()).await;
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_old_details() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(509))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let old = serde_json::json!({"version": 1, "fetched_at": NOW - 31 * DAY, "details": {"plot": "Old."}});
    std::fs::write(dir.path().join(file_name(film(8))), old.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(8), NOW);
    drain_until(&mut store, |s| {
        s.in_flight() == 0 && plot(s.get(film(8))) == Some(("Old.".into(), false))
    })
    .await;
}

#[tokio::test]
async fn a_broken_file_is_asked_again_and_written_anew() {
    let server = MockServer::start().await;
    film_answers(&server, 9, "Fixed.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file_name(film(9)));
    std::fs::write(&path, b"{not json").unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(9), NOW);
    assert_eq!(store.get(film(9)), DetailState::Loading);
    drain_until(&mut store, |s| plot(s.get(film(9))).is_some()).await;
    for _ in 0..100 {
        if std::fs::read_to_string(&path).is_ok_and(|t| t.contains("Fixed.")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the file was not written anew");
}

#[tokio::test]
async fn only_the_eight_newest_requests_wait() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"info":{"plot":"x"}}"#)
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=12 {
        store.request(film(id), NOW);
    }
    // 1 and 2 are on their way; 3 and 4 were dropped from the 10 that waited.
    drain_until(&mut store, |s| {
        s.in_flight() == 0 && (5..=12).all(|id| plot(s.get(film(id))).is_some())
    })
    .await;
    assert_eq!(store.get(film(3)), DetailState::Loading);
    assert_eq!(asked(&server, "get_vod_info").await, 10);
}

#[tokio::test]
async fn an_image_address_with_a_query_shows_but_is_not_kept() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"plot":"Kids vanish.","backdrop_path":"https://cdn.example/b.jpg?token=secret"},
        "seasons":[{"season_number":1,"cover":"https://cdn.example/s1.jpg?token=secret"}],
        "episodes":{"1":[{"id":"7","episode_num":1,"info":{"movie_image":"https://cdn.example/e.jpg?token=secret"}}]}}"#;
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(4321), NOW);
    drain_until(&mut store, |s| plot(s.get(series(4321))).is_some()).await;
    let DetailState::Ready { details, .. } = store.get(series(4321)) else {
        panic!("no details");
    };
    assert_eq!(details.backdrop, "https://cdn.example/b.jpg?token=secret");
    assert!(details.seasons[0].cover.contains('?') && details.episodes[0].still.contains('?'));
    // The file is written before the answer is handed over.
    let text = std::fs::read_to_string(dir.path().join(file_name(series(4321)))).unwrap();
    assert!(
        text.contains("Kids vanish.") && !text.contains("token="),
        "redact sees no token there: {text}"
    );
}

#[tokio::test]
async fn a_file_from_the_far_past_or_the_far_future_is_read_without_a_panic() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (id, fetched_at) in [(film(10), i64::MIN), (film(11), i64::MAX)] {
        let file = serde_json::json!({"version": 1, "fetched_at": fetched_at, "details": {"plot": "Kept."}});
        std::fs::write(dir.path().join(file_name(id)), file.to_string()).unwrap();
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(10), NOW);
    store.request(film(11), NOW);
    assert_eq!(
        plot(store.get(film(10))),
        Some(("Kept.".into(), true)),
        "the far past: asked again"
    );
    assert_eq!(
        plot(store.get(film(11))),
        Some(("Kept.".into(), false)),
        "a time from the future counts as now"
    );
    assert_eq!(store.in_flight(), 1);
}

#[test]
fn the_folder_and_files_are_named_after_the_portal_and_the_title() {
    let creds = XtreamCreds {
        username: "user".into(),
        password: "secret".into(),
        host: "pandora.example".into(),
        port: 8080,
    };
    let dir = dir_for(Path::new("C:/cfg/cache"), Some(&creds)).unwrap();
    assert_eq!(
        dir,
        Path::new("C:/cfg/cache")
            .join("vod")
            .join("pandora.example_8080")
    );
    assert!(dir_for(Path::new("C:/cfg/cache"), None).is_none());
    assert_eq!(file_name(series(4321)), "s4321.json");
    assert_eq!(file_name(film(5001)), "m5001.json");
}
