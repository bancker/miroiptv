//! What the thuisbioscoop takes from an Xtream portal (spec 2026-10-15 §4.1,
//! §4.3): the extra fields of the film and series lists, the two category
//! lists, and a film's and a series' details. Everything comes from a local
//! mock in Xtream's shape, read as tolerantly as portals need.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::{xtream::XtreamPortal, Category, Movie, Portal, PortalError, Series};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn creds_for(server: &MockServer) -> XtreamCreds {
    let uri = url::Url::parse(&server.uri()).unwrap();
    XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap_or(80),
    }
}

/// Answers `action` (and, when given, `key`=`value`) with `body`, status 200.
async fn answer(server: &MockServer, action: &str, key: Option<(&str, &str)>, body: &str) {
    let mut mock = Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param("action", action));
    if let Some((k, v)) = key {
        mock = mock.and(query_param(k, v));
    }
    mock.respond_with(ResponseTemplate::new(200).set_body_string(body.to_owned()))
        .mount(server)
        .await;
}

/// A portal on a mock that answers the three lists with these bodies.
async fn portal_with_lists(vod: &str, series: &str) -> (MockServer, XtreamPortal) {
    let server = MockServer::start().await;
    answer(&server, "get_live_streams", None, "[]").await;
    answer(&server, "get_vod_streams", None, vod).await;
    answer(&server, "get_series", None, series).await;
    let portal = XtreamPortal::new(creds_for(&server));
    (server, portal)
}

fn movie(json: &str) -> Movie {
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn the_film_list_carries_cover_rating_added_and_category() {
    let vod = r#"[
        {"stream_id":5001,"name":"The Matrix (1999)","container_extension":"mkv","is_adult":"0",
         "stream_icon":"https://image.tmdb.org/t/p/w600/m.jpg","rating":"8.2","added":"1792000000",
         "category_id":"12","plot":"not kept","cast":"not kept"},
        {"stream_id":"5002","name":"Plain","stream_icon":null,"rating":7.3,"added":1792000100,"category_id":13}
    ]"#;
    let (_server, portal) = portal_with_lists(vod, "[]").await;
    let cat = portal.fetch_catalog().await.unwrap();
    let m = &cat.movies[0];
    assert_eq!(
        m.stream_icon.as_deref(),
        Some("https://image.tmdb.org/t/p/w600/m.jpg")
    );
    assert_eq!((m.rating, m.added, m.category_id), (82, 1_792_000_000, 12));
    let plain = &cat.movies[1];
    assert_eq!(plain.stream_icon, None);
    assert_eq!(
        (
            plain.stream_id,
            plain.rating,
            plain.added,
            plain.category_id
        ),
        (5002, 73, 1_792_000_100, 13)
    );
}

#[test]
fn a_rating_is_read_whatever_its_form_and_a_wrong_one_is_none() {
    for (rating, tenths) in [
        (r#""7.3""#, 73),
        ("7.3", 73),
        (r#""7,3""#, 73),
        ("10", 100),
        (r#""7.25""#, 73),
        (r#""""#, 0),
        ("null", 0),
        (r#""0""#, 0),
        (r#""11""#, 0),
        ("-1", 0),
        (r#""n/a""#, 0),
        ("[7]", 0),
    ] {
        let m = movie(&format!(
            r#"{{"stream_id":1,"name":"x","rating":{rating}}}"#
        ));
        assert_eq!(m.rating, tenths, "rating {rating}");
    }
    let none = movie(r#"{"stream_id":1,"name":"x"}"#);
    assert_eq!((none.rating, none.added, none.category_id), (0, 0, 0));
}

#[tokio::test]
async fn the_series_list_carries_cover_rating_release_date_genre_category_and_change_time() {
    let series = r#"[
        {"series_id":4321,"name":"Dark","cover":"https://img.example/dark.jpg","rating":"8.7",
         "releaseDate":"2017-12-01","genre":"Drama, Mystery","category_id":"7","last_modified":"1792001000",
         "plot":"not kept","cast":"not kept"},
        {"series_id":4322,"name":"Old Style","release_date":"2005-03-24","rating":""},
        {"series_id":4323,"name":"Both","releaseDate":"","release_date":"2010-01-01"}
    ]"#;
    let (_server, portal) = portal_with_lists("[]", series).await;
    let cat = portal.fetch_catalog().await.unwrap();
    let dark = &cat.series[0];
    assert_eq!(dark.cover.as_deref(), Some("https://img.example/dark.jpg"));
    assert_eq!(
        (dark.rating, dark.category_id, dark.last_modified),
        (87, 7, 1_792_001_000)
    );
    assert_eq!(dark.released(), Some("2017-12-01"));
    assert_eq!(dark.genre.as_deref(), Some("Drama, Mystery"));
    assert_eq!(cat.series[1].released(), Some("2005-03-24"));
    assert_eq!(cat.series[1].rating, 0);
    // Both names in one series must not fail the list (serde's duplicate
    // field, had they been one field with an alias).
    assert_eq!(cat.series[2].released(), Some("2010-01-01"));
}

#[test]
fn a_series_without_release_date_has_none() {
    let s: Series =
        serde_json::from_str(r#"{"series_id":1,"name":"x","releaseDate":" "}"#).unwrap();
    assert_eq!(s.released(), None);
}

#[tokio::test]
async fn the_category_lists_are_fetched_and_an_odd_entry_is_left_out() {
    let server = MockServer::start().await;
    answer(
        &server,
        "get_vod_categories",
        None,
        r#"[{"category_id":"12","category_name":"NL | Komedie","parent_id":0},"odd",
            {"category_id":13,"category_name":null}]"#,
    )
    .await;
    answer(
        &server,
        "get_series_categories",
        None,
        r#"[{"category_id":"7","category_name":"Series | Drama"}]"#,
    )
    .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let categories = portal.fetch_categories().await.unwrap();
    let named = |id: i64, name: &str| Category {
        category_id: id,
        category_name: name.into(),
    };
    assert_eq!(categories.vod, [named(12, "NL | Komedie"), named(13, "")]);
    assert_eq!(categories.series, [named(7, "Series | Drama")]);
}

#[tokio::test]
async fn a_category_list_that_is_refused_or_no_list_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_categories"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Status(503))
    ));
    let other = MockServer::start().await;
    answer(&other, "get_vod_categories", None, r#"{"error":"x"}"#).await;
    let portal = XtreamPortal::new(creds_for(&other));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Shape(_))
    ));
}

#[tokio::test]
async fn a_films_details_are_read_from_get_vod_info() {
    let server = MockServer::start().await;
    let body = r#"{"info":{
        "plot":"A hacker learns the truth.","actors":"Keanu Reeves, Carrie-Anne Moss",
        "director":"Lana Wachowski","genre":"Action, Science Fiction","releasedate":"1999-03-30",
        "age":"16","duration_secs":8160,"rating":"8.2","youtube_trailer":"vKQi3bBA1y8",
        "backdrop_path":["https://image.tmdb.org/t/p/w1280/b.jpg","https://x/2.jpg"],"tmdb_id":"603",
        "video":{"width":3840,"height":"2160","color_transfer":"smpte2084"},"audio":{"channels":6}},
        "movie_data":{"stream_id":5001,"container_extension":"mkv"}}"#;
    answer(&server, "get_vod_info", Some(("vod_id", "5001")), body).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_vod_info(5001).await.unwrap();
    assert_eq!(d.plot, "A hacker learns the truth.");
    assert_eq!(d.cast, "Keanu Reeves, Carrie-Anne Moss", "from `actors`");
    assert_eq!(d.director, "Lana Wachowski");
    assert_eq!(d.genre, "Action, Science Fiction");
    assert_eq!((d.released.as_str(), d.year), ("1999-03-30", 1999));
    assert_eq!((d.age.as_str(), d.duration, d.rating), ("16", 8160, 82));
    assert_eq!(d.trailer, "vKQi3bBA1y8");
    assert_eq!(
        d.backdrop, "https://image.tmdb.org/t/p/w1280/b.jpg",
        "the first"
    );
    assert_eq!(d.tmdb, 603);
    assert_eq!(
        (d.width, d.height, d.hdr, d.channels),
        (3840, 2160, true, 6)
    );
    assert!(d.seasons.is_empty() && d.episodes.is_empty());
}

#[tokio::test]
async fn a_films_length_comes_from_its_clock_when_it_has_no_seconds() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"duration":"01:47:12","video":[],"audio":[],"backdrop_path":"https://x/one.jpg"}}"#;
    answer(&server, "get_vod_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_vod_info(1)
        .await
        .unwrap();
    assert_eq!(d.duration, 3600 + 47 * 60 + 12);
    assert_eq!(d.backdrop, "https://x/one.jpg", "one backdrop as a string");
    assert_eq!((d.width, d.height, d.hdr, d.channels), (0, 0, false, 0));
}

#[tokio::test]
async fn an_unknown_film_is_an_error_and_a_refusal_its_status() {
    let server = MockServer::start().await;
    answer(&server, "get_vod_info", Some(("vod_id", "1")), "[]").await;
    Mock::given(query_param("vod_id", "2"))
        .respond_with(ResponseTemplate::new(509))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_vod_info(1).await,
        Err(PortalError::Shape(_))
    ));
    assert!(matches!(
        portal.fetch_vod_info(2).await,
        Err(PortalError::Status(509))
    ));
}

#[tokio::test]
async fn a_series_details_hold_its_seasons_and_episodes_in_order() {
    let server = MockServer::start().await;
    let body = r#"{
        "seasons":[{"season_number":2,"name":"Season 2","cover":"https://x/s2.jpg"},
                   {"season_number":1,"name":"Season 1","cover_big":"https://x/s1.jpg"},
                   {"season_number":0,"name":"Specials"}],
        "info":{"name":"Dark","plot":"Kids vanish.","cast":"Louis Hofmann","genre":"Drama",
                "releaseDate":"2017-12-01","rating":"8.7","episode_run_time":"55",
                "backdrop_path":["https://x/back.jpg"],"youtube_trailer":"rrwycJ08PSA"},
        "episodes":{
          "2":[{"id":"987655","episode_num":"1","title":"Beginnings","container_extension":"mkv",
                "info":{"duration_secs":2820,"plot":"Two.","movie_image":"https://x/e21.jpg"},"added":"1792089400"}],
          "1":[{"id":"987651","episode_num":2,"title":"Lies","container_extension":"mp4","info":{"duration":"00:47:00"}},
               {"id":"987650","episode_num":1,"title":"Secrets","container_extension":"mp4","info":[]},
               {"episode_num":3,"title":"no id: not playable"}]
        }}"#;
    answer(
        &server,
        "get_series_info",
        Some(("series_id", "4321")),
        body,
    )
    .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_series_info(4321).await.unwrap();
    assert_eq!(
        (d.plot.as_str(), d.cast.as_str()),
        ("Kids vanish.", "Louis Hofmann")
    );
    assert_eq!((d.year, d.rating, d.duration), (2017, 87, 55 * 60));
    assert_eq!(d.backdrop, "https://x/back.jpg");
    assert_eq!(d.trailer, "rrwycJ08PSA");
    let seasons: Vec<(u32, &str, &str)> = d
        .seasons
        .iter()
        .map(|s| (s.number, s.name.as_str(), s.cover.as_str()))
        .collect();
    assert_eq!(
        seasons,
        [
            (0, "Specials", ""),
            (1, "Season 1", "https://x/s1.jpg"),
            (2, "Season 2", "https://x/s2.jpg")
        ]
    );
    let episodes: Vec<(&str, u32, u32, &str, &str, u32)> = d
        .episodes
        .iter()
        .map(|e| {
            let (id, title, ext) = (e.id.as_str(), e.title.as_str(), e.ext.as_str());
            (id, e.season, e.number, title, ext, e.duration)
        })
        .collect();
    assert_eq!(
        episodes,
        [
            ("987650", 1, 1, "Secrets", "mp4", 0),
            ("987651", 1, 2, "Lies", "mp4", 47 * 60),
            ("987655", 2, 1, "Beginnings", "mkv", 2820),
        ]
    );
    let last = &d.episodes[2];
    assert_eq!(
        (last.plot.as_str(), last.still.as_str()),
        ("Two.", "https://x/e21.jpg")
    );
    assert_eq!(last.added, 1_792_089_400);
}

#[tokio::test]
async fn episodes_in_lists_and_a_series_without_info_are_read_too() {
    let server = MockServer::start().await;
    let body = r#"{"episodes":[[{"id":11,"episode_num":1,"season":1,"title":"A"}],
                                [{"id":"21","episode_num":1,"season":"2","title":"B"}]]}"#;
    answer(&server, "get_series_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_series_info(9)
        .await
        .unwrap();
    let got: Vec<(&str, u32, u32)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.season, e.number))
        .collect();
    assert_eq!(got, [("11", 1, 1), ("21", 2, 1)]);
    assert!(d.plot.is_empty() && d.seasons.is_empty());
}

#[tokio::test]
async fn an_image_address_with_the_account_in_it_is_dropped() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"backdrop_path":"http://h:80/movie/u/p/5001.jpg","plot":"kept"}}"#;
    answer(&server, "get_vod_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_vod_info(5001)
        .await
        .unwrap();
    assert_eq!(d.backdrop, "", "never kept, never shown");
    assert_eq!(d.plot, "kept");
}

#[tokio::test]
async fn an_m3u_list_has_no_categories_and_no_details() {
    let (portal, _) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttp://a.example/1\n").unwrap();
    let categories = portal.fetch_categories().await.unwrap();
    assert!(categories.vod.is_empty() && categories.series.is_empty());
    assert!(matches!(
        portal.fetch_vod_info(1).await,
        Err(PortalError::NoDetails)
    ));
    assert!(matches!(
        portal.fetch_series_info(1).await,
        Err(PortalError::NoDetails)
    ));
}

#[tokio::test]
async fn nulls_and_odd_types_in_a_details_answer_leave_fields_empty() {
    let server = MockServer::start().await;
    let series = r#"{"info":null,"seasons":null,
        "episodes":{"1":null,"2":[null,{"id":null},{"id":5,"episode_num":"x","title":7,"info":"none"}]}}"#;
    answer(&server, "get_series_info", None, series).await;
    let film = r#"{"info":{"plot":null,"rating":{},"duration_secs":"12x","video":"none","audio":null,
        "backdrop_path":[null,""],"tmdb_id":"n/a"}}"#;
    answer(&server, "get_vod_info", None, film).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_series_info(1).await.unwrap();
    assert!(d.plot.is_empty() && d.seasons.is_empty());
    let got: Vec<(&str, u32, u32, &str)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.season, e.number, e.title.as_str()))
        .collect();
    assert_eq!(
        got,
        [("5", 2, 0, "7")],
        "an episode without an id plays nothing"
    );
    let f = portal.fetch_vod_info(1).await.unwrap();
    assert_eq!(f, bongo_player::portal::VodDetails::default());
}
