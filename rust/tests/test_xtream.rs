use bongo_player::args::XtreamCreds;
use bongo_player::portal::{xtream::XtreamPortal, Portal};
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
