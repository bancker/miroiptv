use tvplayer::args::XtreamCreds;
use tvplayer::portal::{xtream::XtreamPortal, Portal};
use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};

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
    mount_action(&server, "get_vod_streams",  fixtures("xtream_vod")).await;
    mount_action(&server, "get_series",       fixtures("xtream_series")).await;

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

#[test]
fn live_stream_url_shape() {
    let creds = XtreamCreds {
        username: "u".into(), password: "p".into(),
        host: "h.example.com".into(), port: 8080,
    };
    let portal = XtreamPortal::new(creds);
    assert_eq!(
        portal.live_stream_url(42),
        "http://h.example.com:8080/live/u/p/42.m3u8"
    );
}

#[test]
fn movie_stream_url_uses_extension() {
    let creds = XtreamCreds {
        username: "u".into(), password: "p".into(),
        host: "h.example.com".into(), port: 80,
    };
    let portal = XtreamPortal::new(creds);
    assert_eq!(
        portal.movie_stream_url(5001, "mkv"),
        "http://h.example.com:80/movie/u/p/5001.mkv"
    );
}
