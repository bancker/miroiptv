//! What the thuisbioscoop takes from an Xtream portal (spec 2026-10-15 §4.1,
//! §4.3): the extra fields of the film and series lists, the two category
//! lists, and a film's and a series' details. Everything comes from a local
//! mock in Xtream's shape, read as tolerantly as portals need.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::vod_info::{parse_series_info, parse_vod_info};
use bongo_player::portal::VodDetails;
use bongo_player::portal::{xtream::XtreamPortal, Category, Movie, Portal, PortalError, Series};
use serde_json::json;
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

// ---- Fix round 1: what the review of the first commit found ----

/// A film's length in seconds from the clock in its `duration`, which it has
/// when it has no `duration_secs`.
fn film_clock(clock: &str) -> u32 {
    parse_vod_info(&json!({ "info": { "duration": clock } }))
        .unwrap()
        .duration
}

#[test]
fn a_clock_of_two_parts_is_minutes_and_seconds() {
    assert_eq!(film_clock("47:12"), 47 * 60 + 12);
    assert_eq!(film_clock("0:30"), 30);
    assert_eq!(film_clock("120:00"), 7200, "minutes may pass 59");
}

#[test]
fn a_clock_with_a_huge_part_saturates_and_never_overflows() {
    // Every part is whatever an i64 holds. Unchecked, a debug build panics on
    // the overflow and a release build wraps to a wrong length.
    for clock in [
        "9223372036854775807:00:00",
        "3000000000000000:00:00",
        "1:9223372036854775807:0",
        "0:9223372036854775807:1",
        "1:2:9223372036854775807",
        "9223372036854775807:9223372036854775807",
        "1:9223372036854775807",
        // 4,294,969,200 s: past u32::MAX without passing i64.
        "1193047:00:00",
    ] {
        assert_eq!(film_clock(clock), u32::MAX, "clock {clock}");
    }
}

#[test]
fn an_episodes_clock_is_read_like_a_films() {
    let answer = json!({"episodes": {"1": [
        {"id": "1", "episode_num": 1, "info": {"duration": "9223372036854775807:00:00"}},
        {"id": "2", "episode_num": 2, "info": {"duration": "47:12"}},
        {"id": "3", "episode_num": 3, "info": {"duration": "01:02:03"}}
    ]}});
    let d = parse_series_info(&answer).unwrap();
    let got: Vec<u32> = d.episodes.iter().map(|e| e.duration).collect();
    assert_eq!(got, [u32::MAX, 47 * 60 + 12, 3600 + 2 * 60 + 3]);
}

#[test]
fn a_film_whose_info_is_null_or_no_object_has_empty_details() {
    // Review Focus 2: nulls and wrong types leave the fields empty, and the
    // answer is no error. A film that exists but has no metadata gets
    // `"info": []` from some portals.
    for answer in [
        json!({"info": null}),
        json!({"info": []}),
        json!({"info": "none"}),
        json!({"info": 0}),
        json!({"info": null, "movie_data": {"stream_id": 5001, "container_extension": "mkv"}}),
    ] {
        let details = parse_vod_info(&answer).unwrap_or_else(|e| panic!("{answer}: {e}"));
        assert_eq!(details, VodDetails::default(), "{answer}");
    }
}

#[test]
fn an_answer_without_an_info_is_no_film_answer() {
    // `[]` is what portals answer for an id they do not know, and an object
    // without `info` is a refusal such as a login's `user_info`: both are an
    // error to retry, never an empty success to keep for a month.
    for answer in [
        json!([]),
        json!(null),
        json!("busy"),
        json!({}),
        json!({"user_info": {"auth": 0}}),
    ] {
        assert!(
            matches!(parse_vod_info(&answer), Err(PortalError::Shape(_))),
            "{answer}"
        );
    }
}

#[tokio::test]
async fn a_film_without_metadata_is_an_empty_page_not_an_error() {
    let server = MockServer::start().await;
    answer(
        &server,
        "get_vod_info",
        Some(("vod_id", "1")),
        r#"{"info":null}"#,
    )
    .await;
    answer(
        &server,
        "get_vod_info",
        Some(("vod_id", "2")),
        r#"{"info":[],"movie_data":{"stream_id":2,"container_extension":"mp4"}}"#,
    )
    .await;
    let portal = XtreamPortal::new(creds_for(&server));
    for id in [1, 2] {
        let details = portal.fetch_vod_info(id).await.unwrap();
        assert_eq!(details, VodDetails::default(), "film {id}");
    }
}

#[test]
fn a_series_answer_that_is_no_object_is_an_error() {
    // `[]` is what portals answer for a series they do not know.
    for answer in [json!([]), json!(null), json!("busy"), json!(5)] {
        assert!(
            matches!(parse_series_info(&answer), Err(PortalError::Shape(_))),
            "{answer}"
        );
    }
}

#[tokio::test]
async fn an_unknown_series_is_an_error_and_a_refusal_its_status() {
    let server = MockServer::start().await;
    answer(&server, "get_series_info", Some(("series_id", "1")), "[]").await;
    Mock::given(query_param("series_id", "2"))
        .respond_with(ResponseTemplate::new(509))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_series_info(1).await,
        Err(PortalError::Shape(_))
    ));
    assert!(matches!(
        portal.fetch_series_info(2).await,
        Err(PortalError::Status(509))
    ));
}

/// A series from the JSON of its list entry, and its release date.
fn released(entry: &str) -> Option<String> {
    let series: Series = serde_json::from_str(entry).unwrap();
    series.released().map(str::to_owned)
}

#[test]
fn of_two_release_dates_the_one_with_text_wins_and_releasedate_comes_first() {
    let both = |camel: &str, snake: &str| {
        released(&format!(
            r#"{{"series_id":1,"name":"x","releaseDate":{camel},"release_date":{snake}}}"#
        ))
    };
    let (a, b) = (r#""2017-12-01""#, r#""1999-01-01""#);
    assert_eq!(
        both(a, b).as_deref(),
        Some("2017-12-01"),
        "camel case first"
    );
    assert_eq!(
        both(b, a).as_deref(),
        Some("1999-01-01"),
        "camel case first"
    );
    // The one without text gives way, whichever it is and whatever it holds.
    for empty in [r#""""#, r#""  ""#, "null", "5", "[]"] {
        assert_eq!(both(empty, b).as_deref(), Some("1999-01-01"), "{empty}");
        assert_eq!(both(a, empty).as_deref(), Some("2017-12-01"), "{empty}");
    }
    assert_eq!(both(r#"" 2017-12-01 ""#, b).as_deref(), Some("2017-12-01"));
    assert_eq!(both(r#""""#, r#""  ""#), None);
}

#[test]
fn an_episode_that_names_its_season_keeps_it_else_it_is_in_the_season_of_its_key() {
    let answer = json!({"episodes": {
        "1": [
            {"id": "a", "episode_num": 1, "season": 3},
            {"id": "b", "episode_num": 2},
            {"id": "c", "episode_num": 3, "season": "2"}
        ],
        "4": [{"id": "d", "episode_num": 1}]
    }});
    let d = parse_series_info(&answer).unwrap();
    let got: Vec<(&str, u32, u32)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.season, e.number))
        .collect();
    // In order of season and number.
    assert_eq!(got, [("b", 1, 2), ("c", 2, 3), ("a", 3, 1), ("d", 4, 1)]);
}

#[test]
fn a_season_number_listed_twice_is_one_season_and_the_first_listed_stays() {
    let answer = json!({"seasons": [
        {"season_number": 2, "name": "Two"},
        {"season_number": 1, "name": "One"},
        {"season_number": 1, "name": "One again"},
        {"season_number": "2", "name": "Two again"},
        "not a season"
    ]});
    let d = parse_series_info(&answer).unwrap();
    let got: Vec<(u32, &str)> = d
        .seasons
        .iter()
        .map(|s| (s.number, s.name.as_str()))
        .collect();
    assert_eq!(got, [(1, "One"), (2, "Two")]);
}

#[test]
fn an_account_in_a_season_cover_or_an_episode_still_is_dropped_like_the_backdrop() {
    let answer = json!({
        "info": {"backdrop_path": ["http://h:80/movie/u/p/9.jpg"]},
        "seasons": [
            {"season_number": 1, "name": "One", "cover": "http://h:80/series/u/p/1.jpg"},
            {"season_number": 2, "name": "Two", "cover_big": "http://u:p@h/x.jpg"},
            {"season_number": 3, "name": "Three", "cover": "https://img.example/s3.jpg?username=u&password=p"},
            {"season_number": 4, "name": "Four", "cover": "https://img.example/s4.jpg"}
        ],
        "episodes": {"1": [
            {"id": "11", "episode_num": 1, "info": {"movie_image": "http://h:80/series/u/p/11.jpg"}},
            {"id": "12", "episode_num": 2, "info": {"movie_image": "http://u:p@h/y.jpg"}},
            {"id": "13", "episode_num": 3, "info": {"movie_image": "https://img.example/e3.jpg?password=p"}},
            {"id": "14", "episode_num": 4, "info": {"movie_image": "https://img.example/e4.jpg"}}
        ]}
    });
    let d = parse_series_info(&answer).unwrap();
    assert_eq!(d.backdrop, "");
    // The seasons and episodes stay; only the address with an account goes.
    let covers: Vec<(&str, &str)> = d
        .seasons
        .iter()
        .map(|s| (s.name.as_str(), s.cover.as_str()))
        .collect();
    assert_eq!(
        covers,
        [
            ("One", ""),
            ("Two", ""),
            ("Three", ""),
            ("Four", "https://img.example/s4.jpg")
        ]
    );
    let stills: Vec<(&str, &str)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.still.as_str()))
        .collect();
    assert_eq!(
        stills,
        [
            ("11", ""),
            ("12", ""),
            ("13", ""),
            ("14", "https://img.example/e4.jpg")
        ]
    );
}

#[test]
fn hlg_and_pq_are_hdr_and_the_usual_transfer_is_not() {
    let hdr = |transfer: &str| {
        parse_vod_info(&json!({"info": {"video": {"color_transfer": transfer}}}))
            .unwrap()
            .hdr
    };
    assert!(hdr("smpte2084"), "PQ (HDR10, Dolby Vision)");
    assert!(hdr("arib-std-b67"), "HLG");
    assert!(hdr("ARIB-STD-B67"), "in any letter case");
    assert!(!hdr("bt709"));
    assert!(!hdr(""));
}

#[test]
fn sound_channels_are_a_number_clamped_to_a_byte() {
    let channels = |value: serde_json::Value| {
        parse_vod_info(&json!({"info": {"audio": {"channels": value}}}))
            .unwrap()
            .channels
    };
    assert_eq!(channels(json!(6)), 6);
    assert_eq!(channels(json!("8")), 8, "a number as a string");
    assert_eq!(channels(json!(255)), 255);
    assert_eq!(channels(json!(256)), 255, "clamped, not wrapped to 0");
    assert_eq!(channels(json!(300)), 255, "clamped, not wrapped to 44");
    assert_eq!(channels(json!(-2)), 0);
    assert_eq!(channels(json!("many")), 0);
    assert_eq!(channels(json!(null)), 0);
}

#[tokio::test]
async fn a_refused_series_category_list_fails_the_call_though_the_film_list_came() {
    // The call is all or nothing (Ruling 1'): the catalog's fetch task takes an
    // error as "no categories" and logs one line. So a second list that fails
    // must not turn into an empty one on the way (`unwrap_or_default`), and the
    // film list that did come is not returned alone.
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_categories"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"[{"category_id":"12","category_name":"NL | Komedie"}]"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(query_param("action", "get_series_categories"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .expect(1)
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Status(503))
    ));
    // The same for a series list that is no list.
    let other = MockServer::start().await;
    answer(
        &other,
        "get_vod_categories",
        None,
        r#"[{"category_id":"12","category_name":"NL | Komedie"}]"#,
    )
    .await;
    answer(&other, "get_series_categories", None, r#"{"error":"x"}"#).await;
    let portal = XtreamPortal::new(creds_for(&other));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Shape(_))
    ));
}

/// A port nothing listens on: a listener binds one, and is dropped.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// The account of the tests that look for it in an error.
const USER: &str = "bob-q7x";
const PASS: &str = "s3cr3t-k9z";

/// Fails when `err` names the portal's address or account, in its words
/// (`{}`) or in its debug form (`{:?}`): `reqwest` prints the URL of a call in
/// both when it has one, and the URL holds `username=` and `password=`.
fn assert_names_no_address(err: &PortalError, host_port: &str) {
    for text in [format!("{err}"), format!("{err:?}")] {
        for leak in [
            USER,
            PASS,
            "username",
            "password",
            "player_api",
            "http://",
            host_port,
        ] {
            assert!(!text.contains(leak), "`{leak}` in the error: {text}");
        }
    }
}

#[tokio::test]
async fn the_errors_of_the_new_calls_name_no_address_when_the_connection_fails() {
    // The debug form of a refused connection holds the socket address that
    // hyper tried (`127.0.0.1:port`, from the connect error, not from the
    // URL). With an IP as the host that is the typed host:port itself, so the
    // portal is called by a name: then the typed `localhost:port` can only
    // come from the URL.
    let port = closed_port();
    let portal = XtreamPortal::new(XtreamCreds {
        username: USER.into(),
        password: PASS.into(),
        host: "localhost".into(),
        port,
    });
    // A refused connection takes about 2 s on Windows: the three at once.
    let (categories, film, series) = tokio::join!(
        portal.fetch_categories(),
        portal.fetch_vod_info(1),
        portal.fetch_series_info(1)
    );
    let errors = [
        categories.unwrap_err(),
        film.unwrap_err(),
        series.unwrap_err(),
    ];
    for err in &errors {
        assert!(matches!(err, PortalError::Http(_)), "{err}");
        assert_names_no_address(err, &format!("localhost:{port}"));
    }
}

#[tokio::test]
async fn the_errors_of_the_new_calls_name_no_address_when_the_answer_is_no_json() {
    let server = MockServer::start().await;
    for action in ["get_vod_categories", "get_vod_info", "get_series_info"] {
        answer(&server, action, None, "<html>busy</html>").await;
    }
    let uri = url::Url::parse(&server.uri()).unwrap();
    let host_port = format!("{}:{}", uri.host_str().unwrap(), uri.port().unwrap());
    let portal = XtreamPortal::new(XtreamCreds {
        username: USER.into(),
        password: PASS.into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    });
    let errors = [
        portal.fetch_categories().await.unwrap_err(),
        portal.fetch_vod_info(1).await.unwrap_err(),
        portal.fetch_series_info(1).await.unwrap_err(),
    ];
    for err in &errors {
        assert!(matches!(err, PortalError::Http(_)), "{err}");
        assert_names_no_address(err, &host_port);
    }
}
