//! A film's and a series' details (vod/details.rs, spec 2026-10-15 §4.3):
//! asked when a page opens, kept as a file, fresh for a day (series) or 30
//! days (films), two at a time with the newest first.
//!
//! No test rests on a clock. What must come in the background is polled for
//! (`drain_until`, `until_asked`), with 30 s as the bound of a test that fails
//! and not as a margin; an answer that has to stay on its way is held by a
//! delay far longer than any test, so what happens meanwhile happens in one
//! order only.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::Portal;
use bongo_player::vod::details::{
    dir_for, file_name, DetailState, DetailStore, FILM_FRESH_SECS, REFRESH_REST_SECS,
    SERIES_FRESH_SECS,
};
use bongo_player::vod::{Kind, TitleId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wiremock::matchers::query_param;
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: i64 = 1_792_022_400;
const DAY: i64 = 86_400;
const HOUR: i64 = 3_600;
/// The delay of an answer that stays on its way for the whole test.
const STUCK_MS: u64 = 120_000;
/// The longest a test waits for something in the background. A test ends the
/// moment it comes: this only bounds a test that fails.
const PATIENCE: Duration = Duration::from_secs(30);

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

fn portal_on(port: u16, username: &str, password: &str) -> Arc<dyn Portal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: username.into(),
        password: password.into(),
        host: "127.0.0.1".into(),
        port,
    }))
}

fn port_of(server: &MockServer) -> u16 {
    url::Url::parse(&server.uri()).unwrap().port().unwrap()
}

fn portal_of(server: &MockServer) -> Arc<dyn Portal> {
    portal_on(port_of(server), "u", "secret")
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

/// The portal answers `action` for the title `id` (named by `key`) with `body`.
async fn answers(server: &MockServer, action: &str, key: &str, id: i64, body: &str) {
    Mock::given(query_param("action", action))
        .and(query_param(key, id.to_string().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

/// A details file as a build keeps it, fetched at `fetched_at`.
fn put_file(dir: &Path, id: TitleId, fetched_at: i64, details: serde_json::Value) {
    let file = serde_json::json!({"version": 1, "fetched_at": fetched_at, "details": details});
    std::fs::write(dir.join(file_name(id)), file.to_string()).unwrap();
}

/// Lets the store take in answers until `done` says so.
async fn drain_until(store: &mut DetailStore, mut done: impl FnMut(&DetailStore) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        store.drain(NOW);
        if done(store) {
            return;
        }
        assert!(Instant::now() < deadline, "the answers never came");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Lets the store take in answers until none is on its way or waits, so that
/// a test can look at where everything has ended. (After a drain, no request
/// on its way means none that waits: a free place starts the newest.)
async fn settle(store: &mut DetailStore) {
    drain_until(store, |s| s.in_flight() == 0).await;
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

/// The ids the portal was asked for (a film's or a series'), in the order the
/// requests came.
async fn asked_ids(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter_map(|r| {
            r.url
                .query_pairs()
                .find(|(k, _)| k == "vod_id" || k == "series_id")
                .map(|(_, v)| v.into_owned())
        })
        .collect()
}

/// Waits until the portal has been asked `n` times.
async fn until_asked(server: &MockServer, n: usize) {
    let deadline = Instant::now() + PATIENCE;
    while asked_ids(server).await.len() < n {
        assert!(
            Instant::now() < deadline,
            "the portal was never asked {n} times"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The folder holds no file at all, not even a temporary one.
fn nothing_in(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().next().is_none()
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
    // 2 stays on its way for the whole test, so a place frees up only when 1
    // answers, and then when each request after it does: the order of the
    // requests does not depend on how long anything takes.
    film_answers(&server, 1, "Film 1.", 0).await;
    film_answers(&server, 2, "Film 2.", STUCK_MS).await;
    film_answers(&server, 3, "Film 3.", 0).await;
    film_answers(&server, 4, "Film 4.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=4 {
        store.request(film(id), NOW);
    }
    assert_eq!(store.in_flight(), 2);
    until_asked(&server, 2).await;
    // 1's place goes to the newest request that waits, 4 ...
    drain_until(&mut store, |s| plot(s.get(film(1))).is_some()).await;
    assert_eq!(store.in_flight(), 2, "2 and 4");
    // ... and 4's to the one after it, 3.
    drain_until(&mut store, |s| plot(s.get(film(4))).is_some()).await;
    assert_eq!(store.in_flight(), 2, "2 and 3");
    drain_until(&mut store, |s| plot(s.get(film(3))).is_some()).await;
    assert_eq!(store.in_flight(), 1, "2, which has not answered");
    assert_eq!(store.get(film(2)), DetailState::Loading);
    let order = asked_ids(&server).await;
    assert_eq!(order.len(), 4, "{order:?}");
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
    // It was asked, and refused: the old details stayed because the refresh
    // failed, not because nothing asked.
    assert_eq!(asked(&server, "get_vod_info").await, 1);
    // They are still too old, but the refresh rests: a page that opens again
    // meanwhile asks nothing, and the next open after the rest asks again.
    store.request(film(8), NOW + 1);
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), false)));
    assert_eq!(store.in_flight(), 0);
    store.request(film(8), NOW + REFRESH_REST_SECS);
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), true)));
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), false)));
    assert_eq!(asked(&server, "get_vod_info").await, 2);
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
    // The file is written before the answer is handed over: nothing to wait for.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Fixed."), "{text}");
}

#[tokio::test]
async fn a_file_of_another_version_or_shape_is_asked_again() {
    let server = MockServer::start().await;
    film_answers(&server, 12, "Current.", 0).await;
    film_answers(&server, 13, "Current.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    // A later build's file, and a file whose time is not a time.
    let later = serde_json::json!({"version": 2, "fetched_at": NOW, "details": {"plot": "Later."}});
    let odd =
        serde_json::json!({"version": 1, "fetched_at": "yesterday", "details": {"plot": "Odd."}});
    std::fs::write(dir.path().join(file_name(film(12))), later.to_string()).unwrap();
    std::fs::write(dir.path().join(file_name(film(13))), odd.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(12), NOW);
    store.request(film(13), NOW);
    assert_eq!(store.get(film(12)), DetailState::Loading, "not read");
    assert_eq!(store.get(film(13)), DetailState::Loading, "not read");
    settle(&mut store).await;
    for id in [12, 13] {
        assert_eq!(plot(store.get(film(id))), Some(("Current.".into(), false)));
    }
}

#[tokio::test]
async fn only_the_eight_newest_requests_wait() {
    let server = MockServer::start().await;
    // Answers are taken in by `drain` alone, so 1 and 2 stay on their way
    // while the other ten are requested, however fast the portal is.
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"info":{"plot":"x"}}"#))
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

// What follows is not in the plan's list: the numbers it leaves unpinned, the
// freshness of details held in memory, a page opened twice, an answer that
// holds nothing a page shows, and the log.

#[tokio::test]
async fn the_freshness_is_a_day_for_a_series_and_30_days_for_a_film_to_the_second() {
    // The numbers themselves: the tests above count with the constants.
    assert_eq!(SERIES_FRESH_SECS, 86_400);
    assert_eq!(FILM_FRESH_SECS, 2_592_000);
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    // (title, age of its file in seconds, whether the portal is asked again)
    let cases = [
        (series(1), 86_400, false),
        (series(2), 86_401, true),
        (film(3), 2_592_000, false),
        (film(4), 2_592_001, true),
    ];
    for (id, age, _) in cases {
        put_file(
            dir.path(),
            id,
            NOW - age,
            serde_json::json!({"plot": "Kept."}),
        );
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for (id, age, asked_again) in cases {
        store.request(id, NOW);
        assert_eq!(
            plot(store.get(id)),
            Some(("Kept.".into(), asked_again)),
            "{id:?} of {age} s"
        );
    }
}

#[tokio::test]
async fn details_held_in_memory_are_asked_again_once_they_are_too_old() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"info":{"plot":"Day one."}}"#))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    answers(
        &server,
        "get_series_info",
        "series_id",
        1,
        r#"{"info":{"plot":"Day two."}}"#,
    )
    .await;
    // No folder: the details are in memory only.
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(series(1), NOW);
    drain_until(&mut store, |s| plot(s.get(series(1))).is_some()).await;
    // The page opens again the same day: fresh, no question.
    store.request(series(1), NOW + 23 * HOUR);
    assert_eq!(plot(store.get(series(1))), Some(("Day one.".into(), false)));
    assert_eq!(store.in_flight(), 0);
    assert_eq!(asked(&server, "get_series_info").await, 1);
    // A day and a second after they came: the old ones show, the portal is asked.
    store.request(series(1), NOW + DAY + 1);
    assert_eq!(plot(store.get(series(1))), Some(("Day one.".into(), true)));
    drain_until(&mut store, |s| {
        plot(s.get(series(1))) == Some(("Day two.".into(), false))
    })
    .await;
    assert_eq!(asked(&server, "get_series_info").await, 2);
    // And they are fresh again from then on.
    store.request(series(1), NOW + DAY + 1 + HOUR);
    assert_eq!(store.in_flight(), 0);
    assert_eq!(asked(&server, "get_series_info").await, 2);
}

#[tokio::test]
async fn a_page_opened_again_while_it_waits_becomes_the_newest_request() {
    let server = MockServer::start().await;
    // 2 stays on its way, so a place frees up when 1 answers and then when
    // each request after it does.
    film_answers(&server, 1, "Film 1.", 0).await;
    film_answers(&server, 2, "Film 2.", STUCK_MS).await;
    for id in 3..=5 {
        film_answers(&server, id, &format!("Film {id}."), 0).await;
    }
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=5 {
        store.request(film(id), NOW);
    }
    // 3, 4 and 5 wait; the page of 3 opens again, which makes its request the
    // newest: it starts first.
    store.request(film(3), NOW);
    until_asked(&server, 2).await;
    for id in [1, 3, 5, 4] {
        drain_until(&mut store, |s| plot(s.get(film(id))).is_some()).await;
    }
    // 3 waited once: with 2, which has not answered, nothing else is on its way.
    assert_eq!(store.in_flight(), 1);
    let order = asked_ids(&server).await;
    assert_eq!(
        &order[2..],
        ["3", "5", "4"],
        "the page opened last starts first: {order:?}"
    );
}

#[tokio::test]
async fn a_page_opened_again_while_its_request_is_on_its_way_asks_nothing_more() {
    let server = MockServer::start().await;
    film_answers(&server, 1, "Film 1.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(film(1), NOW);
    store.request(film(1), NOW);
    assert_eq!(store.in_flight(), 1);
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(1))), Some(("Film 1.".into(), false)));
    assert_eq!(asked_ids(&server).await, ["1"]);
}

#[tokio::test]
async fn enter_on_a_page_that_has_its_details_asks_nothing() {
    let server = MockServer::start().await;
    film_answers(&server, 1, "Film 1.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(film(1), NOW);
    settle(&mut store).await;
    store.retry(film(1), NOW);
    store.retry(film(2), NOW);
    assert_eq!(store.in_flight(), 0);
    assert_eq!(plot(store.get(film(1))), Some(("Film 1.".into(), false)));
    assert_eq!(store.get(film(2)), DetailState::Loading, "never asked");
    assert_eq!(asked_ids(&server).await, ["1"]);
}

#[tokio::test]
async fn only_the_64_titles_asked_last_stay_in_memory() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for id in 1..=65 {
        put_file(
            dir.path(),
            film(id),
            NOW,
            serde_json::json!({"plot": format!("Film {id}.")}),
        );
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in 1..=64 {
        store.request(film(id), NOW);
    }
    assert!(
        (1..=64).all(|id| plot(store.get(film(id))).is_some()),
        "64 titles fit"
    );
    store.request(film(65), NOW);
    assert_eq!(
        store.get(film(1)),
        DetailState::Loading,
        "the title asked longest ago went"
    );
    assert!((2..=65).all(|id| plot(store.get(film(id))).is_some()));
    // Its page opens again: its file is read in the same call, and the next
    // oldest goes.
    store.request(film(1), NOW);
    assert_eq!(plot(store.get(film(1))), Some(("Film 1.".into(), false)));
    assert_eq!(store.get(film(2)), DetailState::Loading);
    assert_eq!(store.in_flight(), 0);
    assert!(
        asked_ids(&server).await.is_empty(),
        "the portal was not asked"
    );
}

#[tokio::test]
async fn a_title_that_waits_or_is_on_its_way_stays_in_memory() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    // 10 titles whose file is too old: 2 go to the portal, 8 wait ...
    for id in 1..=10 {
        put_file(
            dir.path(),
            film(id),
            NOW - 31 * DAY,
            serde_json::json!({"plot": "Old."}),
        );
    }
    // ... and 60 fresh ones, which take the memory over its 64.
    for id in 101..=160 {
        put_file(
            dir.path(),
            film(id),
            NOW,
            serde_json::json!({"plot": "Fresh."}),
        );
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in (1..=10).chain(101..=160) {
        store.request(film(id), NOW);
    }
    for id in 1..=10 {
        assert_eq!(
            plot(store.get(film(id))),
            Some(("Old.".into(), true)),
            "{id}"
        );
    }
    // The oldest of the fresh ones went instead.
    assert!((101..=106).all(|id| store.get(film(id)) == DetailState::Loading));
    assert!((107..=160).all(|id| plot(store.get(film(id))) == Some(("Fresh.".into(), false))));
}

/// What a portal answers with for a series it knows nothing of, or whose
/// information is all empty: the reader takes each of them as a success.
const EMPTY_SERIES: [&str; 6] = [
    r#"{}"#,
    r#"{"info":[],"seasons":[],"episodes":[]}"#,
    r#"{"info":{},"seasons":[],"episodes":{}}"#,
    r#"{"info":null,"seasons":null,"episodes":null}"#,
    // Empty text, a season with nothing in it and an episode without an id
    // (which cannot play, so the reader leaves it out) are no page either.
    r#"{"info":{"plot":"  ","cast":""},"seasons":[{}],"episodes":{"1":[{"title":"No id"}]}}"#,
    // The TMDB id is on no page.
    r#"{"info":{"tmdb_id":"603"}}"#,
];

#[tokio::test]
async fn an_empty_series_answer_is_no_details_and_is_not_kept() {
    for body in EMPTY_SERIES {
        let server = MockServer::start().await;
        answers(&server, "get_series_info", "series_id", 4321, body).await;
        let dir = tempfile::tempdir().unwrap();
        let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        store.request(series(4321), NOW);
        settle(&mut store).await;
        assert_eq!(store.get(series(4321)), DetailState::Failed, "{body}");
        assert!(nothing_in(dir.path()), "{body} was kept");
        // Opening the page again asks nothing: Enter does.
        store.request(series(4321), NOW);
        assert_eq!(store.get(series(4321)), DetailState::Failed, "{body}");
        assert_eq!(store.in_flight(), 0, "{body}");
        assert_eq!(asked(&server, "get_series_info").await, 1, "{body}");
    }
}

#[tokio::test]
async fn a_film_answer_without_info_is_no_details_and_is_not_kept() {
    for body in [
        r#"{"info":[]}"#,
        r#"{"info":{}}"#,
        r#"{"info":null,"movie_data":{"stream_id":5001,"name":"Heat","container_extension":"mkv"}}"#,
        r#"{"info":{"plot":"","youtube_trailer":"","video":{"width":0,"height":0}}}"#,
        r#"{"info":{"tmdb_id":"603"}}"#,
    ] {
        let server = MockServer::start().await;
        answers(&server, "get_vod_info", "vod_id", 5001, body).await;
        let dir = tempfile::tempdir().unwrap();
        let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        store.request(film(5001), NOW);
        settle(&mut store).await;
        assert_eq!(store.get(film(5001)), DetailState::Failed, "{body}");
        assert!(nothing_in(dir.path()), "{body} was kept");
        store.request(film(5001), NOW);
        assert_eq!(store.get(film(5001)), DetailState::Failed, "{body}");
        assert_eq!(store.in_flight(), 0, "{body}");
        assert_eq!(asked(&server, "get_vod_info").await, 1, "{body}");
    }
}

#[tokio::test]
async fn a_series_with_info_but_no_seasons_or_episodes_is_a_page_and_is_kept() {
    // An announced series has its description and nothing else yet; any one
    // thing a page shows is enough, a film's as well.
    let pages = [
        (
            series(1),
            r#"{"info":{"plot":"Coming soon."}}"#,
            "Coming soon.",
        ),
        (
            series(2),
            r#"{"info":{"plot":"Coming soon."},"seasons":[],"episodes":[]}"#,
            "Coming soon.",
        ),
        (
            series(3),
            r#"{"info":{"plot":"","genre":"Drama"},"seasons":[],"episodes":{}}"#,
            "",
        ),
        (series(4), r#"{"seasons":[{"season_number":1}]}"#, ""),
        (series(5), r#"{"episodes":{"1":[{"id":"7"}]}}"#, ""),
        (film(6), r#"{"info":{"youtube_trailer":"dQw4w9WgXcQ"}}"#, ""),
        (
            film(7),
            r#"{"info":{"video":{"width":3840,"height":2160}}}"#,
            "",
        ),
        (
            film(8),
            r#"{"info":{"backdrop_path":"https://cdn.example/b.jpg"}}"#,
            "",
        ),
    ];
    for (id, body, plot_of_it) in pages {
        let (action, key) = match id.kind {
            Kind::Film => ("get_vod_info", "vod_id"),
            Kind::Series => ("get_series_info", "series_id"),
        };
        let server = MockServer::start().await;
        answers(&server, action, key, id.id, body).await;
        let dir = tempfile::tempdir().unwrap();
        let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        store.request(id, NOW);
        drain_until(&mut store, |s| plot(s.get(id)).is_some()).await;
        assert_eq!(
            plot(store.get(id)),
            Some((plot_of_it.into(), false)),
            "{body}"
        );
        assert!(
            dir.path().join(file_name(id)).exists(),
            "{body} was not kept"
        );
        // The next page open reads the file and asks nothing.
        let mut again = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        again.request(id, NOW + HOUR);
        assert_eq!(
            plot(again.get(id)),
            Some((plot_of_it.into(), false)),
            "{body}"
        );
        assert_eq!(again.in_flight(), 0, "{body}");
        assert_eq!(asked(&server, action).await, 1, "{body}");
    }
}

#[tokio::test]
async fn enter_asks_again_after_an_empty_answer() {
    let server = MockServer::start().await;
    // The first answer has nothing in it, the second has a page.
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    answers(
        &server,
        "get_series_info",
        "series_id",
        7,
        r#"{"info":{"plot":"Second time."}}"#,
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(7), NOW);
    settle(&mut store).await;
    assert_eq!(store.get(series(7)), DetailState::Failed);
    assert!(nothing_in(dir.path()));
    store.retry(series(7), NOW);
    assert_eq!(store.get(series(7)), DetailState::Loading, "asking again");
    settle(&mut store).await;
    assert_eq!(
        plot(store.get(series(7))),
        Some(("Second time.".into(), false))
    );
    assert_eq!(asked(&server, "get_series_info").await, 2);
    let text = std::fs::read_to_string(dir.path().join(file_name(series(7)))).unwrap();
    assert!(text.contains("Second time."), "{text}");
}

#[tokio::test]
async fn a_kept_file_that_holds_no_details_is_not_shown() {
    // What an older build kept of an answer with nothing in it, fresh by its time.
    let server = MockServer::start().await;
    answers(
        &server,
        "get_series_info",
        "series_id",
        4321,
        r#"{"info":{"plot":"Now there is one."}}"#,
    )
    .await;
    answers(&server, "get_vod_info", "vod_id", 5001, r#"{"info":[]}"#).await;
    let dir = tempfile::tempdir().unwrap();
    put_file(dir.path(), series(4321), NOW, serde_json::json!({}));
    put_file(
        dir.path(),
        film(5001),
        NOW,
        serde_json::json!({"plot": "", "tmdb": 603, "seasons": [{}]}),
    );
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(4321), NOW);
    store.request(film(5001), NOW);
    assert_eq!(store.get(series(4321)), DetailState::Loading, "not shown");
    assert_eq!(store.get(film(5001)), DetailState::Loading, "not shown");
    assert_eq!(store.in_flight(), 2, "both are asked");
    // The portal has details for the series and still none for the film.
    settle(&mut store).await;
    assert_eq!(
        plot(store.get(series(4321))),
        Some(("Now there is one.".into(), false))
    );
    assert_eq!(store.get(film(5001)), DetailState::Failed);
    let text = std::fs::read_to_string(dir.path().join(file_name(series(4321)))).unwrap();
    assert!(text.contains("Now there is one."), "written anew: {text}");
}

#[tokio::test]
async fn an_empty_answer_to_a_refresh_keeps_the_old_details_and_the_old_file() {
    let server = MockServer::start().await;
    answers(&server, "get_vod_info", "vod_id", 8, r#"{"info":[]}"#).await;
    let dir = tempfile::tempdir().unwrap();
    put_file(
        dir.path(),
        film(8),
        NOW - 31 * DAY,
        serde_json::json!({"plot": "Old."}),
    );
    let path = dir.path().join(file_name(film(8)));
    let before = std::fs::read(&path).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(8), NOW);
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), true)));
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), false)));
    assert_eq!(asked(&server, "get_vod_info").await, 1);
    assert_eq!(std::fs::read(&path).unwrap(), before, "the old file stays");
}

/// What the log says, as the app's own filter lets this crate's lines through
/// (`main.rs`: this crate at debug, reqwest and hyper at warn), from this
/// thread. The second dispatcher is there for the tests that run beside this
/// one: with a single dispatcher registered, tracing works a line's interest
/// out from whoever hits it first, and another test's thread, which has no
/// subscriber, would silence the line for this one for good.
struct LogCapture {
    buffer: LogBuffer,
    _second: tracing::Dispatch,
    _logging: tracing::subscriber::DefaultGuard,
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    fn start() -> Self {
        let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let buffer = LogBuffer::default();
        let sink = buffer.clone();
        let filter = tracing_subscriber::EnvFilter::new(format!(
            "{}=debug,reqwest=warn,hyper=warn",
            bongo_player::LOG_TARGET
        ));
        // No time: a port number must not be found in a clock's digits.
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_ansi(false)
            .without_time()
            .with_env_filter(filter)
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

/// Opens the page of `id` on `portal`, files in `folder`, and waits for the
/// answer to be taken in.
async fn fetched(portal: Arc<dyn Portal>, folder: Option<PathBuf>, id: TitleId) -> DetailState {
    let mut store = DetailStore::new(portal, folder);
    store.request(id, NOW);
    settle(&mut store).await;
    store.get(id)
}

const USER: &str = "madeup-account-x7k2";
const PASS: &str = "madeup-secret-q9w4";
const FOLDER: &str = "madeup-user-folder-4417";

#[tokio::test]
async fn the_log_names_a_title_by_its_id_and_never_an_address_an_account_or_a_folder() {
    let log = LogCapture::start();
    let server = MockServer::start().await;
    let port = port_of(&server);
    let dead_port = {
        // Nothing listens there once the listener is gone.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    film_answers(&server, 5001, "A hacker.", 0).await;
    for (id, status, body) in [
        (5002, 503, ""),
        (5003, 200, "this is not json"),
        (5004, 200, "[]"),
        (5005, 200, r#"{"info":[]}"#),
    ] {
        Mock::given(query_param("vod_id", id.to_string().as_str()))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
    }
    film_answers(&server, 5007, "Not kept.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    // A folder that cannot be made: a file stands where it should be.
    let blocker = dir.path().join("a file where a folder should be");
    std::fs::write(&blocker, b"x").unwrap();

    // The account is in every request's address, made up for this test.
    let portal = portal_on(port, USER, PASS);
    let ready = |state: DetailState| matches!(state, DetailState::Ready { .. });
    let kept = Some(dir.path().join("kept"));
    assert!(ready(fetched(portal.clone(), kept, film(5001)).await));
    for id in 5002..=5005 {
        let state = fetched(portal.clone(), None, film(id)).await;
        assert_eq!(state, DetailState::Failed, "{id}");
    }
    let refused = portal_on(dead_port, USER, PASS);
    assert_eq!(
        fetched(refused, None, film(5006)).await,
        DetailState::Failed
    );
    let unwritable = Some(blocker.join(FOLDER));
    assert!(
        ready(fetched(portal, unwritable, film(5007)).await),
        "the page has its details though they could not be kept"
    );

    let text = log.text();
    for line in [
        "vod: details of m5001 fetched in ",
        "vod: details of m5002 not fetched (http status 503)",
        "vod: details of m5003 not fetched (http: ",
        "vod: details of m5004 not fetched (portal returned unexpected shape: no film info)",
        "vod: details of m5005 not fetched (the answer holds no details)",
        "vod: details of m5006 not fetched (http: ",
        "vod: details of m5007 not kept (",
    ] {
        assert!(text.contains(line), "{line:?} is not in the log: {text}");
    }
    let (tmp, tmp_name) = (
        dir.path().to_string_lossy().into_owned(),
        dir.path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    let fragments = [
        USER,
        PASS,
        "127.0.0.1",
        "localhost",
        &port.to_string(),
        &dead_port.to_string(),
        "player_api",
        "username",
        "password",
        "http://",
        FOLDER,
        "a file where a folder should be",
        &tmp,
        &tmp_name,
    ];
    for fragment in fragments {
        assert!(
            !text.contains(fragment),
            "the log holds {fragment:?}: {text}"
        );
    }
    // The Debug form of an error is where the IP and port show up.
    for debug_form in [
        "Error {", "kind:", "source:", "Http(", "Status(", "Os {", "Custom {",
    ] {
        assert!(
            !text.contains(debug_form),
            "an error in its Debug form ({debug_form:?}): {text}"
        );
    }
}

// What follows is the review of the commit: the behaviour the tests above leave
// unpinned (a mutant of vod/details.rs that every test above lets live), and the
// rest after a failed refresh.

/// A drain at `now` until `done` says so: the time a request that a drain starts
/// is stamped with.
async fn drain_at_until(
    store: &mut DetailStore,
    now: i64,
    mut done: impl FnMut(&DetailStore) -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        store.drain(now);
        if done(store) {
            return;
        }
        assert!(Instant::now() < deadline, "the answers never came");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The `fetched_at` of a details file.
fn file_time(dir: &Path, id: TitleId) -> i64 {
    let text = std::fs::read_to_string(dir.join(file_name(id))).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap()["fetched_at"]
        .as_i64()
        .unwrap()
}

#[tokio::test]
async fn a_refresh_that_failed_rests_however_often_the_page_opens() {
    // Two kinds of failure: the portal refuses, and the portal has nothing.
    for (id, action, key, status, body) in [
        (film(8), "get_vod_info", "vod_id", 509, ""),
        (series(9), "get_series_info", "series_id", 200, "{}"),
    ] {
        let server = MockServer::start().await;
        Mock::given(query_param("action", action))
            .and(query_param(key, id.id.to_string().as_str()))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        put_file(
            dir.path(),
            id,
            NOW - 31 * DAY,
            serde_json::json!({"plot": "Old."}),
        );
        let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        store.request(id, NOW);
        settle(&mut store).await;
        assert_eq!(asked(&server, action).await, 1, "{id:?}");
        // A page that asks at every frame (one a second here) for the whole rest:
        // the old details show and nothing goes to the portal.
        for second in 1..REFRESH_REST_SECS {
            store.request(id, NOW + second);
            assert_eq!(store.in_flight(), 0, "{id:?}, {second} s after the failure");
        }
        assert_eq!(plot(store.get(id)), Some(("Old.".into(), false)), "{id:?}");
        assert_eq!(asked(&server, action).await, 1, "{id:?}");
        // The rest is over: the next open asks again, and the old details show
        // while it does.
        store.request(id, NOW + REFRESH_REST_SECS);
        assert_eq!(store.in_flight(), 1, "{id:?}");
        assert_eq!(plot(store.get(id)), Some(("Old.".into(), true)), "{id:?}");
        drain_at_until(&mut store, NOW + REFRESH_REST_SECS, |s| s.in_flight() == 0).await;
        assert_eq!(asked(&server, action).await, 2, "{id:?}");
        // It failed again, so it rests again, from when it failed.
        store.request(id, NOW + 2 * REFRESH_REST_SECS - 1);
        assert_eq!(store.in_flight(), 0, "{id:?}");
        store.request(id, NOW + 2 * REFRESH_REST_SECS);
        assert_eq!(store.in_flight(), 1, "{id:?}");
        // Let that request come in before the server is given back. One still on
        // its way would be sent after the next await, to whichever test the pool
        // has handed the server by then, and count among its questions.
        drain_at_until(&mut store, NOW + 2 * REFRESH_REST_SECS, |s| {
            s.in_flight() == 0
        })
        .await;
        assert_eq!(asked(&server, action).await, 3, "{id:?}");
    }
}

#[tokio::test]
async fn after_the_rest_a_refresh_that_works_makes_the_details_fresh_again() {
    let server = MockServer::start().await;
    // The first refresh fails, the second (after the rest) works.
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(509))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    film_answers(&server, 8, "New.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    put_file(
        dir.path(),
        film(8),
        NOW - 31 * DAY,
        serde_json::json!({"plot": "Old."}),
    );
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(8), NOW);
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(8))), Some(("Old.".into(), false)));
    store.request(film(8), NOW + REFRESH_REST_SECS);
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(8))), Some(("New.".into(), false)));
    // Fresh again, for 30 days from the second fetch: no question until then.
    store.request(film(8), NOW + REFRESH_REST_SECS + FILM_FRESH_SECS);
    assert_eq!(store.in_flight(), 0);
    store.request(film(8), NOW + REFRESH_REST_SECS + FILM_FRESH_SECS + 1);
    assert_eq!(store.in_flight(), 1);
}

#[test]
fn the_rest_is_ten_minutes() {
    assert_eq!(REFRESH_REST_SECS, 600);
}

#[tokio::test]
async fn a_request_is_stamped_with_the_time_it_started_and_its_file_says_so() {
    let server = MockServer::start().await;
    for id in 1..=3 {
        film_answers(&server, id, &format!("Film {id}."), 0).await;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in 1..=3 {
        store.request(film(id), NOW);
    }
    // 3 waits for a place. Only a drain at NOW + 1000 ever runs, so it is the
    // one that starts 3.
    drain_at_until(&mut store, NOW + 1000, |s| {
        (1..=3).all(|id| plot(s.get(film(id))).is_some())
    })
    .await;
    let times = [1, 2, 3].map(|id| file_time(dir.path(), film(id)));
    assert_eq!(times, [NOW, NOW, NOW + 1000]);
    // The store holds the same time: 3 is fresh for a month from then.
    store.request(film(3), NOW + 1000 + FILM_FRESH_SECS);
    assert_eq!(store.in_flight(), 0);
    store.request(film(3), NOW + 1000 + FILM_FRESH_SECS + 1);
    assert_eq!(store.in_flight(), 1);
}

#[tokio::test]
async fn a_page_with_its_details_in_memory_reads_no_file_when_it_opens_again() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .and(query_param("vod_id", "2"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    put_file(
        dir.path(),
        film(1),
        NOW,
        serde_json::json!({"plot": "First."}),
    );
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(1), NOW);
    assert_eq!(plot(store.get(film(1))), Some(("First.".into(), false)));
    // Another writer changes the file. This store has what it needs in memory:
    // it reads nothing, and it asks nothing.
    put_file(
        dir.path(),
        film(1),
        NOW,
        serde_json::json!({"plot": "Changed."}),
    );
    store.request(film(1), NOW);
    assert_eq!(plot(store.get(film(1))), Some(("First.".into(), false)));
    // A title that failed is not helped by a file that turns up either: Enter asks.
    store.request(film(2), NOW);
    settle(&mut store).await;
    assert_eq!(store.get(film(2)), DetailState::Failed);
    put_file(
        dir.path(),
        film(2),
        NOW,
        serde_json::json!({"plot": "Turned up."}),
    );
    store.request(film(2), NOW);
    assert_eq!(store.get(film(2)), DetailState::Failed);
    assert_eq!(store.in_flight(), 0);
}

#[tokio::test]
async fn a_title_whose_page_opened_again_is_the_last_to_leave_memory() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for id in 1..=65 {
        put_file(
            dir.path(),
            film(id),
            NOW,
            serde_json::json!({"plot": format!("Film {id}.")}),
        );
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in 1..=64 {
        store.request(film(id), NOW);
    }
    store.request(film(1), NOW);
    store.request(film(65), NOW);
    assert_eq!(plot(store.get(film(1))), Some(("Film 1.".into(), false)));
    assert_eq!(
        store.get(film(2)),
        DetailState::Loading,
        "asked longest ago"
    );
}

#[tokio::test]
async fn titles_that_failed_leave_memory_like_the_others() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=65 {
        store.request(film(id), NOW);
        settle(&mut store).await;
    }
    assert_eq!(store.get(film(1)), DetailState::Loading, "the oldest went");
    assert_eq!(store.get(film(2)), DetailState::Failed);
    assert_eq!(store.get(film(65)), DetailState::Failed);
}

#[tokio::test]
async fn enter_starts_its_request_in_the_same_call() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    film_answers(&server, 7, "Second time.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(film(7), NOW);
    settle(&mut store).await;
    assert_eq!(store.get(film(7)), DetailState::Failed);
    store.retry(film(7), NOW);
    assert_eq!(store.in_flight(), 1);
    // Enter again while that request is on its way asks nothing more.
    store.retry(film(7), NOW);
    assert_eq!(store.in_flight(), 1);
    settle(&mut store).await;
    assert_eq!(
        plot(store.get(film(7))),
        Some(("Second time.".into(), false))
    );
    assert_eq!(asked(&server, "get_vod_info").await, 2);
}

#[tokio::test]
async fn a_request_pushed_out_of_the_queue_is_not_refreshing_and_asks_again_when_it_opens() {
    let server = MockServer::start().await;
    for id in 1..=11 {
        film_answers(&server, id, &format!("New {id}."), 0).await;
    }
    let dir = tempfile::tempdir().unwrap();
    for id in 1..=11 {
        put_file(
            dir.path(),
            film(id),
            NOW - 31 * DAY,
            serde_json::json!({"plot": "Old."}),
        );
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    // 1 and 2 are on their way, 9 wait for a place, and only 8 may.
    for id in 1..=11 {
        store.request(film(id), NOW);
    }
    assert_eq!(store.in_flight(), 2);
    assert_eq!(
        plot(store.get(film(3))),
        Some(("Old.".into(), false)),
        "pushed out: it shows what it had, and nothing is on its way for it"
    );
    for id in [1, 2, 4, 5, 6, 7, 8, 9, 10, 11] {
        assert_eq!(
            plot(store.get(film(id))),
            Some(("Old.".into(), true)),
            "{id}"
        );
    }
    settle(&mut store).await;
    assert_eq!(asked_ids(&server).await.len(), 10);
    assert_eq!(plot(store.get(film(3))), Some(("Old.".into(), false)));
    // Its page opens again: it is asked.
    store.request(film(3), NOW);
    assert_eq!(plot(store.get(film(3))), Some(("Old.".into(), true)));
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(3))), Some(("New 3.".into(), false)));
}

#[tokio::test]
async fn a_file_of_version_0_is_asked_again() {
    let server = MockServer::start().await;
    film_answers(&server, 14, "Current.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let before =
        serde_json::json!({"version": 0, "fetched_at": NOW, "details": {"plot": "Before."}});
    std::fs::write(dir.path().join(file_name(film(14))), before.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(14), NOW);
    assert_eq!(store.get(film(14)), DetailState::Loading, "not read");
    settle(&mut store).await;
    assert_eq!(plot(store.get(film(14))), Some(("Current.".into(), false)));
}

#[tokio::test]
async fn the_answer_is_handed_over_after_its_file_is_whole() {
    // A long-running series: the file takes a while to write, so an answer that
    // is handed over before it is written is seen as such.
    let episodes: Vec<serde_json::Value> = (0..1500)
        .map(|i| {
            serde_json::json!({"id": (1000 + i).to_string(), "episode_num": i % 20 + 1,
                "title": format!("Episode {i}"), "container_extension": "mkv",
                "info": {"plot": "p".repeat(300)}})
        })
        .collect();
    let body = serde_json::json!({"info": {"plot": "Long running."},
        "seasons": [{"season_number": 1}], "episodes": {"1": episodes}})
    .to_string();
    let server = MockServer::start().await;
    answers(&server, "get_series_info", "series_id", 1, &body).await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(1), NOW);
    drain_until(&mut store, |s| plot(s.get(series(1))).is_some()).await;
    let bytes = std::fs::read(dir.path().join(file_name(series(1)))).expect("the file is there");
    let file: serde_json::Value = serde_json::from_slice(&bytes).expect("the file is whole");
    assert_eq!(
        file["details"]["episodes"].as_array().map(Vec::len),
        Some(1500)
    );
    assert!(
        nothing_in_but_the_file(dir.path(), series(1)),
        "a temporary file is left"
    );
}

/// The folder holds this title's file and nothing else.
fn nothing_in_but_the_file(dir: &Path, id: TitleId) -> bool {
    let names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names == [file_name(id)]
}

#[tokio::test]
async fn the_log_lines_have_the_levels_a_reader_expects() {
    let log = LogCapture::start();
    let server = MockServer::start().await;
    film_answers(&server, 1, "Fine.", 0).await;
    Mock::given(query_param("vod_id", "2"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    answers(&server, "get_vod_info", "vod_id", 3, r#"{"info":[]}"#).await;
    film_answers(&server, 4, "Not kept.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("a file");
    std::fs::write(&blocker, b"x").unwrap();
    let portal = portal_of(&server);
    fetched(portal.clone(), None, film(1)).await;
    fetched(portal.clone(), None, film(2)).await;
    fetched(portal.clone(), None, film(3)).await;
    fetched(portal, Some(blocker.join("folder")), film(4)).await;
    let text = log.text();
    let level_of = |line: &str| -> String {
        let found: Vec<&str> = text.lines().filter(|l| l.contains(line)).collect();
        assert_eq!(
            found.len(),
            1,
            "{line:?} is in the log {} times: {text}",
            found.len()
        );
        found[0].split_whitespace().next().unwrap().to_owned()
    };
    assert_eq!(level_of("m1 fetched in "), "INFO");
    assert_eq!(level_of("m2 not fetched (http status 503)"), "WARN");
    assert_eq!(
        level_of("m3 not fetched (the answer holds no details)"),
        "WARN"
    );
    assert_eq!(level_of("m4 not kept ("), "WARN");
}

// The review's m5: a trailer is written as the portal gave it.

#[tokio::test]
async fn a_trailer_that_holds_an_account_shows_but_is_not_kept() {
    // The trailer is the YouTube code as the portal gives it, a code or a YouTube
    // address, and the file keeps either. A portal that put a stream address there
    // has the account in it, in one of the shapes `redact` knows (the path of
    // /movie/ or /series/, user:pass@, the query, the short form): no file keeps
    // that, and the page shows what memory has.
    let pages = [
        (film(1), "dQw4w9WgXcQ", true),
        (film(2), "https://www.youtube.com/watch?v=dQw4w9WgXcQ", true),
        (
            film(3),
            "http://pandora.example:8080/movie/madeup-user/madeup-pass/55.mp4",
            false,
        ),
        (
            series(4),
            "http://pandora.example:8080/series/madeup-user/madeup-pass/55.mkv",
            false,
        ),
        (
            film(5),
            "http://madeup-user:madeup-pass@pandora.example/trailer.mp4",
            false,
        ),
        (
            film(6),
            "http://pandora.example/play.php?username=madeup-user&password=madeup-pass",
            false,
        ),
        (
            film(7),
            "http://pandora.example:8080/madeup-user/madeup-pass/55.mp4",
            false,
        ),
    ];
    for (id, trailer, kept) in pages {
        let (action, key) = match id.kind {
            Kind::Film => ("get_vod_info", "vod_id"),
            Kind::Series => ("get_series_info", "series_id"),
        };
        let body = serde_json::json!({"info": {"plot": "A plot.", "youtube_trailer": trailer}});
        let server = MockServer::start().await;
        answers(&server, action, key, id.id, &body.to_string()).await;
        let dir = tempfile::tempdir().unwrap();
        let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
        store.request(id, NOW);
        settle(&mut store).await;
        let DetailState::Ready { details, .. } = store.get(id) else {
            panic!("{trailer}: no details");
        };
        assert_eq!(details.trailer, trailer, "memory has it as given");
        let text = std::fs::read_to_string(dir.path().join(file_name(id))).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["details"]["plot"], "A plot.", "{trailer}");
        if kept {
            assert_eq!(file["details"]["trailer"], trailer, "{trailer}");
        } else {
            assert_eq!(
                file["details"]["trailer"], "",
                "{trailer} is in the file: {text}"
            );
            assert!(!text.contains("madeup-"), "{trailer}: {text}");
        }
    }
}
