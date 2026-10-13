use bongo_player::args::XtreamCreds;
use bongo_player::portal::xtream::ConnCheck;
use bongo_player::portal::{xtream::XtreamPortal, Portal};
use std::time::Duration;
use wiremock::matchers::{query_param, query_param_is_missing};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn fixtures(name: &str) -> String {
    std::fs::read_to_string(format!("fixtures/{}.json", name)).unwrap()
}

fn creds_for(server: &MockServer) -> XtreamCreds {
    let uri = url::Url::parse(&server.uri()).unwrap();
    XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap_or(80),
    }
}

async fn mount_action(server: &MockServer, action: &str, body: String) {
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(wiremock::matchers::query_param("action", action))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn fetch_catalog_combines_all_three_lists() {
    let server = MockServer::start().await;
    mount_action(&server, "get_live_streams", fixtures("xtream_live")).await;
    mount_action(&server, "get_vod_streams", fixtures("xtream_vod")).await;
    mount_action(&server, "get_series", fixtures("xtream_series")).await;

    let portal = XtreamPortal::new(creds_for(&server));
    let cat = portal.fetch_catalog().await.unwrap();
    assert_eq!(cat.live.len(), 2);
    assert_eq!(cat.movies.len(), 2);
    assert_eq!(cat.series.len(), 1);
    assert_eq!(cat.live[0].name, "NPO 1");
    assert_eq!(cat.live[0].stream_id, 101);
    assert_eq!(cat.movies[1].name, "The Matrix");
    assert_eq!(cat.series[0].series_id, 9001);
}

#[tokio::test]
async fn the_live_list_carries_the_logo_url() {
    let server = MockServer::start().await;
    mount_action(&server, "get_live_streams", fixtures("xtream_live")).await;
    mount_action(&server, "get_vod_streams", fixtures("xtream_vod")).await;
    mount_action(&server, "get_series", fixtures("xtream_series")).await;
    let cat = XtreamPortal::new(creds_for(&server))
        .fetch_catalog()
        .await
        .unwrap();
    assert_eq!(
        cat.live[0].stream_icon.as_deref(),
        Some("http://logos.example/npo1.png")
    );
    assert_eq!(cat.live[1].stream_icon.as_deref(), Some(""));
}

#[tokio::test]
async fn fetch_epg_returns_parsed_entries() {
    let server = MockServer::start().await;
    mount_action(&server, "get_short_epg", fixtures("xtream_epg")).await;

    let portal = XtreamPortal::new(creds_for(&server));
    let epg = portal.fetch_epg(101).await.unwrap();
    assert_eq!(epg.entries().len(), 2);
    assert_eq!(epg.entries()[0].title, "Nieuws");
    assert_eq!(epg.entries()[1].title, "Sport");
}

#[tokio::test]
async fn portal_requests_identify_as_bongo_player() {
    let server = MockServer::start().await;
    let ua = format!("BongoPlayer/{}", env!("CARGO_PKG_VERSION"));
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(wiremock::matchers::header("user-agent", ua.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixtures("xtream_epg")))
        .mount(&server)
        .await;

    let portal = XtreamPortal::new(creds_for(&server));
    let epg = portal.fetch_epg(101).await.unwrap();
    assert_eq!(
        epg.entries().len(),
        2,
        "portal saw a User-Agent other than {ua}"
    );
}

#[test]
fn live_stream_url_shape() {
    let creds = XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h.example.com".into(),
        port: 8080,
    };
    let portal = XtreamPortal::new(creds);
    assert_eq!(
        portal.live_stream_url(42),
        "http://h.example.com:8080/live/u/p/42.m3u8"
    );
}

#[test]
fn live_stream_id_reads_back_a_live_url() {
    // Last-watched keeps only the URL; resume needs to know the channel.
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h.example.com".into(),
        port: 8080,
    });
    let url = portal.live_stream_url(726347);
    assert_eq!(portal.live_stream_id(&url), Some(726347));
    assert_eq!(
        portal.live_stream_id("http://h.example.com:8080/movie/u/p/5001.mkv"),
        None
    );
    assert_eq!(
        portal.live_stream_id("http://h.example.com:8080/timeshift/u/p/35/2026-10-10:19-20/9.m3u8"),
        None
    );
}

#[test]
fn movie_stream_url_uses_extension() {
    let creds = XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h.example.com".into(),
        port: 80,
    };
    let portal = XtreamPortal::new(creds);
    assert_eq!(
        portal.movie_stream_url(5001, "mkv"),
        "http://h.example.com:80/movie/u/p/5001.mkv"
    );
}

#[tokio::test]
async fn portal_errors_never_show_the_credentials() {
    // reqwest puts the full request URL in its error text.
    let e = reqwest::get("http://127.0.0.1:1/player_api.php?username=bob&password=s3cr3t&action=x")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("s3cr3t"), "precondition: reqwest shows the URL");
    let msg = bongo_player::portal::PortalError::from(e).to_string();
    assert!(!msg.contains("bob") && !msg.contains("s3cr3t"), "{msg}");
}

#[test]
fn reads_the_adult_flag_in_every_form_the_portal_uses() {
    use bongo_player::portal::{LiveChannel, Movie, Series};
    let live: Vec<LiveChannel> = serde_json::from_str(
        r#"[
            {"stream_id": 1, "name": "a", "is_adult": "1"},
            {"stream_id": 2, "name": "b", "is_adult": 1},
            {"stream_id": 3, "name": "c", "is_adult": true},
            {"stream_id": 4, "name": "d", "is_adult": "0"},
            {"stream_id": 5, "name": "e"}
        ]"#,
    )
    .unwrap();
    let flags: Vec<bool> = live.iter().map(|c| c.is_adult).collect();
    assert_eq!(flags, [true, true, true, false, false]);
    let movies: Vec<Movie> =
        serde_json::from_str(r#"[{"stream_id": 9, "name": "m", "is_adult": "1"}, {"stream_id": 10, "name": "n"}]"#)
            .unwrap();
    assert_eq!(movies.iter().map(|m| m.is_adult).collect::<Vec<_>>(), [true, false]);
    let series: Vec<Series> = serde_json::from_str(r#"[{"series_id": 7, "name": "s", "is_adult": 1}]"#).unwrap();
    assert!(series[0].is_adult);
}

#[test]
fn a_logo_field_that_is_no_string_is_none_not_a_failed_catalog() {
    use bongo_player::portal::LiveChannel;
    let live: Vec<LiveChannel> = serde_json::from_str(
        r#"[
            {"stream_id": 1, "name": "A", "stream_icon": false},
            {"stream_id": 2, "name": "B", "stream_icon": 7},
            {"stream_id": 3, "name": "C", "stream_icon": null},
            {"stream_id": 4, "name": "D"},
            {"stream_id": 5, "name": "E", "stream_icon": "http://x/e.png"}
        ]"#,
    )
    .unwrap();
    let logos: Vec<Option<&str>> = live.iter().map(|c| c.stream_icon.as_deref()).collect();
    assert_eq!(logos, [None, None, None, None, Some("http://x/e.png")]);
}

#[test]
fn stream_ref_reads_kind_and_id_from_portal_urls() {
    use bongo_player::portal::xtream::{stream_ref, StreamKind};
    assert_eq!(stream_ref("http://h:8080/live/u/p/42.m3u8"), Some((StreamKind::Live, 42)));
    assert_eq!(stream_ref("http://h/movie/u/p/9001.mkv"), Some((StreamKind::Movie, 9001)));
    assert_eq!(stream_ref("http://h/series/u/p/77.mp4"), Some((StreamKind::Series, 77)));
    assert_eq!(
        stream_ref("http://h/timeshift/u/p/35/2026-10-10:19-20/601.m3u8"),
        Some((StreamKind::Timeshift, 601))
    );
    assert_eq!(stream_ref("http://h/player_api.php?username=u"), None);
}

const GOOD_LOGIN: &str = r#"{"user_info":{"auth":1},"server_info":{}}"#;

/// The portal's answer to a login (a player_api.php call without an action).
async fn login_answering(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param_is_missing("action"))
        .respond_with(answer)
        .mount(server)
        .await;
}

async fn login(server: &MockServer, body: &str) {
    login_answering(server, ResponseTemplate::new(200).set_body_string(body.to_string())).await;
}

/// The portal's answer to the live channel list.
async fn live_list_answering(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param("action", "get_live_streams"))
        .respond_with(answer)
        .mount(server)
        .await;
}

#[tokio::test]
async fn check_counts_the_channels_for_good_credentials() {
    let server = MockServer::start().await;
    login(&server, GOOD_LOGIN).await;
    mount_action(&server, "get_live_streams", fixtures("xtream_live")).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Ok { channels: Some(2) });
}

#[tokio::test]
async fn check_tells_a_wrong_login() {
    let server = MockServer::start().await;
    login(&server, r#"{"user_info":{"auth":0}}"#).await;
    assert_eq!(XtreamPortal::new(creds_for(&server)).check().await, ConnCheck::BadLogin);
}

#[tokio::test]
async fn check_tells_something_that_is_no_portal() {
    let server = MockServer::start().await;
    login(&server, "<html>hello</html>").await;
    assert_eq!(XtreamPortal::new(creds_for(&server)).check().await, ConnCheck::Invalid);
}

#[tokio::test]
async fn check_tells_an_unreachable_server() {
    let creds = XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "127.0.0.1".into(),
        port: 1,
    };
    assert_eq!(XtreamPortal::new(creds).check().await, ConnCheck::Unreachable);
}

#[tokio::test]
async fn check_tells_a_login_the_portal_refuses_outright() {
    // Some portals answer 401 or 403 instead of auth 0.
    for status in [401u16, 403u16] {
        let server = MockServer::start().await;
        login_answering(&server, ResponseTemplate::new(status)).await;
        let r = XtreamPortal::new(creds_for(&server)).check().await;
        assert_eq!(r, ConnCheck::BadLogin, "status {status}");
    }
}

#[tokio::test]
async fn check_accepts_auth_as_a_string() {
    let server = MockServer::start().await;
    login(&server, r#"{"user_info":{"auth":"1"}}"#).await;
    mount_action(&server, "get_live_streams", fixtures("xtream_live")).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Ok { channels: Some(2) });
}

#[tokio::test]
async fn a_good_login_is_ok_even_when_the_list_comes_too_late() {
    // The login proves the credentials work; the count is only a bonus.
    let server = MockServer::start().await;
    login(&server, GOOD_LOGIN).await;
    let slow = ResponseTemplate::new(200)
        .set_body_string(fixtures("xtream_live"))
        .set_delay(Duration::from_secs(3));
    live_list_answering(&server, slow).await;
    let r = XtreamPortal::new(creds_for(&server))
        .check_within(Duration::from_millis(300))
        .await;
    assert_eq!(r, ConnCheck::Ok { channels: None });
}

#[tokio::test]
async fn a_good_login_is_ok_even_when_the_portal_throttles_the_list() {
    let server = MockServer::start().await;
    login(&server, GOOD_LOGIN).await;
    let busy = ResponseTemplate::new(503).set_body_string("<html>Service Unavailable</html>");
    live_list_answering(&server, busy).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Ok { channels: None });
}

#[tokio::test]
async fn a_list_behind_an_error_status_is_not_counted() {
    // Whatever the body holds, only a 2xx answer counts.
    let server = MockServer::start().await;
    login(&server, GOOD_LOGIN).await;
    let busy = ResponseTemplate::new(503).set_body_string(fixtures("xtream_live"));
    live_list_answering(&server, busy).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Ok { channels: None });
}

#[tokio::test]
async fn a_good_login_is_ok_even_when_the_list_is_no_json() {
    let server = MockServer::start().await;
    login(&server, GOOD_LOGIN).await;
    mount_action(&server, "get_live_streams", "<html>not a list</html>".to_string()).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Ok { channels: None });
}

#[tokio::test]
async fn a_login_that_takes_longer_than_the_budget_is_unreachable() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_string(GOOD_LOGIN)
        .set_delay(Duration::from_secs(3));
    login_answering(&server, slow).await;
    let r = XtreamPortal::new(creds_for(&server))
        .check_within(Duration::from_millis(300))
        .await;
    assert_eq!(r, ConnCheck::Unreachable);
}

#[tokio::test]
async fn the_budget_is_one_for_the_whole_test() {
    // Login and list take 500 ms each: either fits in 800 ms, both together do not.
    let server = MockServer::start().await;
    let delay = Duration::from_millis(500);
    let login_slowly = ResponseTemplate::new(200)
        .set_body_string(GOOD_LOGIN)
        .set_delay(delay);
    login_answering(&server, login_slowly).await;
    let list_slowly = ResponseTemplate::new(200)
        .set_body_string(fixtures("xtream_live"))
        .set_delay(delay);
    live_list_answering(&server, list_slowly).await;
    let r = XtreamPortal::new(creds_for(&server))
        .check_within(Duration::from_millis(800))
        .await;
    assert_eq!(r, ConnCheck::Ok { channels: None });
}

#[tokio::test]
async fn check_tells_a_busy_portal_from_a_wrong_login() {
    // This portal answers 503 or 509 when it is busy: the credentials are not at
    // fault, the server gave no usable answer.
    for status in [404u16, 500, 502, 503, 509] {
        let server = MockServer::start().await;
        login_answering(&server, ResponseTemplate::new(status)).await;
        let r = XtreamPortal::new(creds_for(&server)).check().await;
        assert_eq!(r, ConnCheck::Unreachable, "status {status}");
    }
}

#[tokio::test]
async fn a_busy_answer_with_a_login_body_is_still_not_a_login() {
    // Only a 2xx answer is read; the body of a refusal says nothing.
    let server = MockServer::start().await;
    let busy = ResponseTemplate::new(503).set_body_string(GOOD_LOGIN);
    login_answering(&server, busy).await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Unreachable);
}

#[tokio::test]
async fn an_answer_that_is_no_xtream_login_is_still_invalid_not_unreachable() {
    let server = MockServer::start().await;
    login(&server, "{\"hello\": 1}").await;
    let r = XtreamPortal::new(creds_for(&server)).check().await;
    assert_eq!(r, ConnCheck::Invalid);
}

#[tokio::test]
async fn a_busy_day_table_is_an_error_not_an_empty_guide() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param("action", "get_simple_data_table"))
        .respond_with(ResponseTemplate::new(503).set_body_string("<html>busy</html>"))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let err = portal.fetch_day_epg(601).await.unwrap_err();
    assert!(
        matches!(err, bongo_player::portal::PortalError::Status(503)),
        "{err}"
    );
}

#[tokio::test]
async fn a_busy_short_epg_is_an_error_not_an_empty_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param("action", "get_short_epg"))
        .respond_with(ResponseTemplate::new(503).set_body_string("<html>busy</html>"))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let err = portal.fetch_epg(601).await.unwrap_err();
    assert!(
        matches!(err, bongo_player::portal::PortalError::Status(503)),
        "{err}"
    );
}

#[tokio::test]
async fn a_day_table_that_is_no_json_is_an_error_not_an_empty_guide() {
    // A hiccup of the portal: an error page, or nothing at all, served with 200.
    // An empty guide would stand in for good EPG for an hour.
    for body in ["<html><body>Service Unavailable</body></html>", ""] {
        let server = MockServer::start().await;
        mount_action(&server, "get_simple_data_table", body.to_string()).await;
        let portal = XtreamPortal::new(creds_for(&server));
        let result = portal.fetch_day_epg(601).await;
        let programmes = result.as_ref().map(|epg| epg.entries().len());
        assert!(programmes.is_err(), "{body:?} gave {programmes:?}");
    }
}

#[tokio::test]
async fn a_short_epg_that_is_no_json_is_an_error_too() {
    let server = MockServer::start().await;
    mount_action(&server, "get_short_epg", "<html>busy</html>".to_string()).await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(portal.fetch_epg(601).await.is_err());
}

#[tokio::test]
async fn an_empty_list_is_an_empty_guide_not_an_error() {
    let server = MockServer::start().await;
    let body = r#"{"epg_listings":[]}"#;
    mount_action(&server, "get_simple_data_table", body.to_string()).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let epg = portal.fetch_day_epg(601).await.unwrap();
    assert!(epg.entries().is_empty());
}

#[tokio::test]
async fn a_day_table_is_read_with_its_archive_flags() {
    let server = MockServer::start().await;
    let body = r#"{"epg_listings":[{"title":"Tk9T","start_timestamp":"1781021400","stop_timestamp":"1781024700","has_archive":"1"}]}"#;
    mount_action(&server, "get_simple_data_table", body.to_string()).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let epg = portal.fetch_day_epg(601).await.unwrap();
    assert_eq!(epg.entries().len(), 1);
    assert_eq!(epg.entries()[0].title, "NOS");
    assert_eq!(epg.entries()[0].has_archive, Some(true));
}
