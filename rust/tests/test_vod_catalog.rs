//! The title index behind the catalog (catalog.rs, spec 2026-10-15 §4.2): the
//! portal's categories are asked beside the three lists, the catalog is put as
//! soon as those are in, and its titles are built after it, once the categories
//! are in (15 s at most), off the screen and from the stored lists. They leave
//! the store only as `Titles`, which apply the adult lock. The channels wait for
//! none of it, and `f` is untouched.

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{CatalogStatus, CatalogStore};
use bongo_player::epg::Epg;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{
    Catalog, Categories, Category, Episode, Movie, Portal, PortalError, Series,
};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::{Extras, Query, TitleIndex};
use bongo_player::vod::{Kind, TitleId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn idle_portal() -> Arc<dyn Portal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }))
}

fn film(id: i64, name: &str, adult: bool) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        is_adult: adult,
        ..Movie::default()
    }
}

fn library() -> Catalog {
    Catalog {
        movies: vec![
            film(1, "NL | The Matrix (1999) [4K]", false),
            film(2, "The Matrix (1999) 1080p", false),
            film(3, "Night Club (2020)", true),
        ],
        series: vec![Series {
            series_id: 9,
            name: "Dark".into(),
            genre: Some("Drama".into()),
            ..Series::default()
        }],
        ..Catalog::default()
    }
}

/// The names a search for everything gives, A to Z.
fn everything(store: &CatalogStore) -> Vec<String> {
    let titles = store.titles().unwrap();
    let query = Query {
        sort: bongo_player::vod::index::Sort::Title,
        ..Query::default()
    };
    titles
        .search(&query)
        .titles
        .iter()
        .map(|&p| titles.title(p as usize).unwrap().name.to_owned())
        .collect()
}

#[test]
fn a_catalog_hands_out_its_titles_behind_the_lock() {
    let store = CatalogStore::from_catalog(idle_portal(), library());
    assert_eq!(
        everything(&store),
        ["Dark", "The Matrix"],
        "locked: no adult title"
    );
    let adult = TitleId {
        kind: Kind::Film,
        id: 3,
    };
    let titles = store.titles().unwrap();
    let pos = titles.find(adult);
    assert_eq!(pos, None, "nor found by its id");
    assert_eq!(titles.title_of_version(3), None);
    store.set_adult_unlocked(true);
    assert_eq!(everything(&store), ["Dark", "Night Club", "The Matrix"]);
    let unlocked = store.titles().unwrap();
    let pos = unlocked.find(adult).unwrap();
    assert!(unlocked.title(pos).unwrap().adult);
    assert!(
        titles.title(pos).is_none(),
        "titles asked while locked stay locked"
    );
    store.set_adult_unlocked(false);
    assert_eq!(everything(&store).len(), 2);
}

#[test]
fn there_are_no_titles_before_a_catalog_and_none_on_an_m3u_list() {
    assert!(CatalogStore::new(idle_portal()).titles().is_none());
    let (portal, catalog) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttp://a.example/1\n").unwrap();
    let m3u = CatalogStore::from_catalog(Arc::new(portal), catalog);
    let titles = m3u.titles().unwrap();
    assert!(titles.search(&Query::default()).titles.is_empty());
}

#[test]
fn what_the_stored_lists_give_up_reaches_the_titles() {
    let catalog = Catalog {
        movies: vec![Movie {
            stream_icon: Some("https://c.example/een.jpg".into()),
            ..film(1, "Een (2001)", false)
        }],
        series: vec![Series {
            series_id: 9,
            name: "Dark".into(),
            cover: Some("https://c.example/dark.jpg".into()),
            genre: Some("Drama".into()),
            release_date: Some("2017-12-01".into()),
            ..Series::default()
        }],
        ..Catalog::default()
    };
    let store = CatalogStore::from_catalog(idle_portal(), catalog);
    let titles = store.titles().unwrap();
    let title = |kind, id| {
        titles
            .title(titles.find(TitleId { kind, id }).unwrap())
            .unwrap()
    };
    assert_eq!(
        title(Kind::Film, 1).cover,
        Some("https://c.example/een.jpg")
    );
    let dark = title(Kind::Series, 9);
    assert_eq!(
        (dark.cover, dark.genres, dark.year),
        (Some("https://c.example/dark.jpg"), Genre::Drama.bit(), 2017)
    );
}

#[test]
fn what_the_lists_give_up_makes_the_titles_the_whole_lists_make() {
    // Every field of every title, built from the stored lists and what they
    // gave up beside them, is what the lists would have made whole (Ruling 22):
    // covers with spaces, empty ones and none, a genre field that names none,
    // both release date fields and odd ones, merged versions, adult ones.
    let covers = [
        " https://c.example/a.jpg ",
        "https://c.example/b.jpg",
        "",
        " ",
    ];
    let movies: Vec<Movie> = (0..300i64)
        .map(|i| {
            let name = format!(
                "{}Film {} ({}){}",
                ["NL | ", "EN | ", ""][(i % 3) as usize],
                i / 3,
                1990 + (i / 3) % 7,
                [" 4K", " 1080p", ""][(i % 3) as usize]
            );
            Movie {
                stream_icon: (i % 5 != 0).then(|| covers[(i % 4) as usize].to_owned()),
                rating: (i * 7 % 101) as u8,
                added: if i % 4 == 0 {
                    0
                } else {
                    1_600_000_000 + i * 86_400
                },
                category_id: i % 3,
                container_extension: Some(["mkv", "mp4"][(i % 2) as usize].into()),
                ..film(i + 1, &name, i % 17 == 0)
            }
        })
        .collect();
    let series: Vec<Series> = (0..100i64)
        .map(|i| Series {
            series_id: i + 1,
            name: format!(
                "{}Serie {i} ({})",
                ["NL | ", ""][(i % 2) as usize],
                2000 + i % 20
            ),
            cover: (i % 3 != 0).then(|| covers[(i % 4) as usize].to_owned()),
            rating: (i * 3 % 101) as u8,
            release_date_camel: [None, Some("2017-12-01"), Some(""), Some("unknown")]
                [(i % 4) as usize]
                .map(str::to_owned),
            release_date: [None, Some("2011-01-01"), Some(" ")][(i % 3) as usize]
                .map(str::to_owned),
            genre: [
                None,
                Some("Drama"),
                Some("Comedy, Drama"),
                Some(""),
                Some("Nothing"),
            ][(i % 5) as usize]
                .map(str::to_owned),
            category_id: i % 3,
            last_modified: 1_600_000_000 + i,
            ..Series::default()
        })
        .collect();
    let category = |id: i64, name: &str| Category {
        category_id: id,
        category_name: name.into(),
    };
    let categories = Categories {
        vod: vec![category(1, "NL | Komedie"), category(2, "Actie")],
        series: vec![category(1, "Nederlands"), category(2, "Misdaad")],
    };
    let whole = TitleIndex::build(&movies, &series, &categories, 2027);
    let (mut stored_films, mut stored_series) = (movies.clone(), series.clone());
    let extras = Extras::take(&mut stored_films, &mut stored_series);
    assert!(stored_films.iter().all(|m| m.stream_icon.is_none()));
    assert!(stored_series.iter().all(|s| s.cover.is_none()
        && s.genre.is_none()
        && s.release_date.is_none()
        && s.release_date_camel.is_none()));
    let built = TitleIndex::build_with(&stored_films, &stored_series, &extras, &categories, 2027);
    assert!(whole.stats().most_versions >= 3, "versions merged");
    assert_eq!(built.stats(), whole.stats());
    assert_eq!(built.cover_bytes(), whole.cover_bytes());
    assert_eq!(built.len(), whole.len());
    for pos in 0..whole.len() {
        let (a, b) = (whole.title(pos).unwrap(), built.title(pos).unwrap());
        assert_eq!(
            (a.id, a.name, a.cover, a.year, a.rating, a.genres, a.category),
            (b.id, b.name, b.cover, b.year, b.rating, b.genres, b.category),
            "title {pos}"
        );
        assert_eq!(
            (a.added_day, a.dutch, a.english, a.adult, a.trailer),
            (b.added_day, b.dutch, b.english, b.adult, b.trailer),
            "title {pos}"
        );
        let (va, vb): (Vec<_>, Vec<_>) = (a.versions.iter().collect(), b.versions.iter().collect());
        assert_eq!(va, vb, "title {pos}");
    }
}

#[test]
fn f_still_finds_a_film_by_its_full_name() {
    let store = CatalogStore::from_catalog(idle_portal(), library());
    let found: Vec<String> = store.search("matrix").into_iter().map(|i| i.name).collect();
    assert_eq!(
        found,
        ["The Matrix (1999) 1080p", "NL | The Matrix (1999) [4K]"]
    );
}

/// Two live channels, to zap through.
const LIVE: &str = r#"[{"stream_id":1,"name":"NL | Een"},{"stream_id":2,"name":"NL | Twee"}]"#;

/// A portal on a mock with these films, two live channels and no series; its
/// film categories are `vod_categories` (`None`: refused with 503), answered
/// after `delay`.
async fn mock_portal(
    films: &str,
    vod_categories: Option<&str>,
    delay: Duration,
) -> (MockServer, Arc<dyn Portal>) {
    let server = MockServer::start().await;
    let answer = |action: &str, body: &str| {
        Mock::given(method("GET"))
            .and(path("/player_api.php"))
            .and(query_param("action", action))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.to_owned()))
    };
    answer("get_live_streams", LIVE).mount(&server).await;
    answer("get_vod_streams", films).mount(&server).await;
    answer("get_series", "[]").mount(&server).await;
    answer("get_series_categories", "[]").mount(&server).await;
    let categories = match vod_categories {
        Some(body) => ResponseTemplate::new(200).set_body_string(body.to_owned()),
        None => ResponseTemplate::new(503),
    };
    Mock::given(query_param("action", "get_vod_categories"))
        .respond_with(categories.set_delay(delay))
        .mount(&server)
        .await;
    let uri = url::Url::parse(&server.uri()).unwrap();
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    });
    (server, Arc::new(portal))
}

/// A store on [`mock_portal`], whose categories answer at once.
async fn portal_with(films: &str, vod_categories: Option<&str>) -> (MockServer, Arc<CatalogStore>) {
    let (server, portal) = mock_portal(films, vod_categories, Duration::ZERO).await;
    (server, Arc::new(CatalogStore::new(portal)))
}

/// Waits (2 s at most) until the store's catalog is in.
async fn loaded(store: &CatalogStore) {
    for _ in 0..400 {
        if store.status() == CatalogStatus::Loaded {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the catalog never loaded: {:?}", store.status());
}

/// Waits (2 s at most) until the store's titles are in: they come after the
/// catalog.
async fn titled(store: &CatalogStore) {
    for _ in 0..400 {
        if store.titles().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the titles never came: {:?}", store.status());
}

const FILMS: &str = r#"[{"stream_id":5,"name":"Lachen (2020)","category_id":"12"}]"#;

/// The film list's one category.
const KOMEDIE: &str = r#"[{"category_id":"12","category_name":"NL | Komedie"}]"#;

#[tokio::test]
async fn the_fetch_gives_the_titles_the_genres_of_their_categories() {
    let (_server, store) = portal_with(FILMS, Some(KOMEDIE)).await;
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    let lachen = titles.title(0).unwrap();
    assert_eq!(lachen.name, "Lachen");
    assert_eq!(lachen.genres, Genre::Comedy.bit());
    assert!(lachen.dutch, "NL | in the category's name");
    assert_eq!(lachen.category.map(|c| c.name), Some("NL | Komedie"));
    assert_eq!(titles.categories().count(), 1);
}

#[tokio::test]
async fn a_refused_category_list_leaves_the_titles_without_genres() {
    let (_server, store) = portal_with(FILMS, None).await;
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    assert_eq!(titles.title(0).unwrap().genres, 0);
    assert_eq!(titles.categories().count(), 0);
}

#[tokio::test]
async fn a_refused_series_category_list_leaves_the_films_without_theirs_too() {
    // The categories are all or nothing (Ruling 1'): the film list came, the
    // series' was refused, and the titles go without either. The catalog is no
    // failure for it.
    let (server, store) = portal_with(FILMS, Some(KOMEDIE)).await;
    Mock::given(query_param("action", "get_series_categories"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&server)
        .await;
    store.spawn_fetch();
    titled(&store).await;
    assert_eq!(store.status(), CatalogStatus::Loaded);
    let titles = store.titles().unwrap();
    let lachen = titles.title(0).unwrap();
    assert_eq!(lachen.name, "Lachen");
    assert_eq!(lachen.genres, 0, "not the film category's genre");
    assert_eq!(lachen.category, None);
    assert_eq!(titles.categories().count(), 0);
}

#[tokio::test]
async fn a_stalled_category_list_holds_up_neither_the_catalog_nor_zapping() {
    let (_server, portal) = mock_portal(FILMS, Some(KOMEDIE), Duration::from_secs(30)).await;
    let store = Arc::new(CatalogStore::new(portal));
    store.spawn_fetch();
    loaded(&store).await;
    assert!(store.is_loaded());
    assert_eq!(store.zap_channels().len(), 2, "zapping works");
    assert_eq!(store.search("lachen").len(), 1, "f works");
    assert!(
        store.titles().is_none(),
        "the titles wait for the categories"
    );
}

#[tokio::test]
async fn the_titles_come_after_the_categories() {
    // The category lists come only when the test lets them: the titles wait
    // for them however long the three lists took, with no margin of time.
    let (release, gate) = oneshot::channel();
    let komedie = Categories {
        vod: vec![Category {
            category_id: 12,
            category_name: "NL | Komedie".into(),
        }],
        series: Vec::new(),
    };
    let portal = Arc::new(Gate::new(
        Lists::Films(
            || {
                vec![Movie {
                    category_id: 12,
                    ..film(5, "Lachen (2020)", false)
                }]
            },
            Vec::new,
        ),
        Cats::After(Mutex::new(Some(gate)), komedie),
    ));
    let store = Arc::new(CatalogStore::new(portal));
    store.spawn_fetch();
    loaded(&store).await;
    assert!(store.titles().is_none(), "the categories are not in yet");
    release.send(()).unwrap();
    titled(&store).await;
    let lachen = store.titles().unwrap().title(0).unwrap().genres;
    assert_eq!(lachen, Genre::Comedy.bit(), "built with them");
}

#[tokio::test]
async fn at_the_bound_the_titles_are_built_without_the_categories() {
    let (_server, portal) = mock_portal(FILMS, Some(KOMEDIE), Duration::from_secs(30)).await;
    let wait = Duration::from_millis(200);
    let store = Arc::new(CatalogStore::new(portal).with_categories_wait(wait));
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    assert_eq!(titles.title(0).unwrap().genres, 0, "no genres");
    assert_eq!(titles.categories().count(), 0, "no categories");
}

#[test]
fn the_channels_wait_for_nothing_while_the_titles_are_built() {
    // One blocking thread, which the test holds: the build of the titles has
    // to wait for it, the catalog must not.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_server, store) = portal_with(FILMS, Some("[]")).await;
        let (release, held) = std::sync::mpsc::channel::<()>();
        let holder = tokio::task::spawn_blocking(move || held.recv());
        store.spawn_fetch();
        loaded(&store).await;
        // The lists are in: the catalog is there, zapping and `f` work, and
        // the titles are still being built.
        assert!(store.is_loaded());
        assert_eq!(store.zap_channels().len(), 2, "zapping works");
        assert_eq!(store.search("lachen").len(), 1, "f works");
        assert!(store.titles().is_none(), "the titles are not built yet");
        assert_eq!(store.titles_revision(), 0);
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();
        // ... and then they come.
        titled(&store).await;
        assert_eq!(store.titles().unwrap().title(0).unwrap().name, "Lachen");
        assert_eq!(store.titles_revision(), 1);
    });
}

/// What the log said on this thread while `work` ran.
fn logged(work: impl FnOnce()) -> String {
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // A second dispatcher, as app.rs's LogCapture has: with this test's one
    // alone, another test's thread (no subscriber) that hits the `vod:` line
    // first would silence it here for good.
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    let bytes = sink.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn the_log_says_what_the_lists_gave() {
    let catalog = Catalog {
        movies: vec![
            Movie {
                stream_icon: Some("https://x/1.jpg".into()),
                rating: 70,
                ..film(1, "A (2001)", false)
            },
            film(2, "B", false),
        ],
        ..Catalog::default()
    };
    let text = logged(|| {
        CatalogStore::from_catalog(idle_portal(), catalog);
    });
    let line = text
        .lines()
        .find(|l| l.contains("vod: "))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        line.contains("vod: 2 titles (0 series, 2 films in 2 versions, at most 1 in one film, 0 left out), 0 categories - cover 50 %, rating 50 %, year 50 %, genre 0 %, Dutch 0 %, English 0 % - index "),
        "{line}"
    );
    assert!(
        line.contains(" KB, covers ") && line.contains(" ms"),
        "{line}"
    );
    assert!(!line.contains("https://"), "no address: {line}");
}

#[test]
fn the_log_names_the_most_versions_in_one_title() {
    // A naming slip that folds many films into one title hardly moves the
    // shares; this figure shows it.
    let catalog = Catalog {
        movies: vec![
            film(1, "Dune (2021) 4K", false),
            film(2, "Dune (2021) 1080p", false),
            film(3, "Dune (2021)", false),
            film(4, "Heat (1995)", false),
        ],
        ..Catalog::default()
    };
    let text = logged(|| {
        CatalogStore::from_catalog(idle_portal(), catalog);
    });
    let line = text
        .lines()
        .find(|l| l.contains("vod: "))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        line.contains(
            "vod: 2 titles (0 series, 2 films in 4 versions, at most 3 in one film, 0 left out), "
        ),
        "{line}"
    );
}

#[test]
fn a_category_list_that_fails_is_logged_without_its_address() {
    // The fetch runs on this thread (the runtime has one), so its lines are
    // this thread's.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let text = logged(|| {
        runtime.block_on(async {
            // No JSON: the answer's reader fails, with the request's address.
            let (_server, store) = portal_with(FILMS, Some("not a list")).await;
            store.spawn_fetch();
            titled(&store).await;
        })
    });
    let line = text
        .lines()
        .find(|l| l.contains("catalog: no categories ("))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        !line.contains("http://") && !line.contains("player_api"),
        "no address: {line}"
    );
}

/// A portal of the test's own, its lists in memory: one film in its first
/// catalog, `Lachen`, and another in every one after it, `Later`.
struct Here {
    fetches: AtomicUsize,
    categories: Asked,
}

/// Where [`Here`] has its categories from.
enum Asked {
    /// A real Xtream portal.
    Of(XtreamPortal),
    /// None, and the first fetch's only once the test lets them come.
    After(Mutex<Option<oneshot::Receiver<()>>>),
}

impl Here {
    fn new(categories: Asked) -> Self {
        Self {
            fetches: AtomicUsize::new(0),
            categories,
        }
    }
}

#[async_trait::async_trait]
impl Portal for Here {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        let film = match self.fetches.fetch_add(1, Ordering::SeqCst) {
            0 => film(5, "Lachen (2020)", false),
            _ => film(6, "Later (2021)", false),
        };
        Ok(Catalog {
            movies: vec![film],
            ..Catalog::default()
        })
    }
    async fn fetch_categories(&self) -> Result<Categories, PortalError> {
        match &self.categories {
            Asked::Of(portal) => portal.fetch_categories().await,
            Asked::After(gate) => {
                let gate = gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                Ok(Categories::default())
            }
        }
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
    fn live_stream_url(&self, _: i64) -> String {
        String::new()
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

#[test]
fn a_refused_connection_for_the_categories_is_logged_without_its_address() {
    // A port nobody listens on: the connection is refused, and the error's
    // Debug form names the address and the port. The line gives its Display.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let portal = Here::new(Asked::Of(XtreamPortal::new(XtreamCreds {
        username: "bob-q7x".into(),
        password: "s3cr3t-k9z".into(),
        host: "127.0.0.1".into(),
        port,
    })));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let text = logged(|| {
        runtime.block_on(async {
            let store = Arc::new(CatalogStore::new(Arc::new(portal)));
            store.spawn_fetch();
            // Windows takes about 2 s to give up on a refused connection.
            for _ in 0..2_000 {
                if store.titles().is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("the titles never came");
        })
    });
    let marker = "catalog: no categories (";
    let line = text
        .lines()
        .find(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("{text}"));
    // What the line says after its marker: the time in front holds digits.
    let said = &line[line.find(marker).unwrap()..];
    for leak in [
        "127.0.0.1",
        &port.to_string(),
        "bob-q7x",
        "s3cr3t-k9z",
        "player_api",
        "http://",
    ] {
        assert!(!said.contains(leak), "`{leak}` in the line: {line}");
    }
    assert!(said.starts_with("catalog: no categories (http: "), "{line}");
}

#[tokio::test]
async fn the_titles_of_a_catalog_replaced_while_they_wait_are_not_put() {
    // F5 while the first catalog's titles wait for their categories: the second
    // catalog's titles are put, and the first one's never are, also when their
    // categories come after the second catalog's titles.
    let (release, gate) = oneshot::channel();
    let portal: Arc<dyn Portal> = Arc::new(Here::new(Asked::After(Mutex::new(Some(gate)))));
    let store = Arc::new(CatalogStore::new(portal.clone()));
    store.spawn_fetch();
    loaded(&store).await;
    assert_eq!(store.search("lachen").len(), 1, "the first catalog");
    store.spawn_fetch();
    loaded(&store).await;
    assert_eq!(store.search("later").len(), 1, "the second catalog");
    titled(&store).await;
    let name = |store: &CatalogStore| store.titles().unwrap().title(0).unwrap().name.to_owned();
    assert_eq!(name(&store), "Later");
    // The first fetch's categories come now. Each fetch holds the portal until
    // it ends, and so does the task that asks its categories.
    release.send(()).unwrap();
    for _ in 0..400 {
        if Arc::strong_count(&portal) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(Arc::strong_count(&portal), 2, "both fetches ended");
    assert_eq!(name(&store), "Later");
    assert_eq!(
        store.titles_revision(),
        1,
        "the first catalog's were not put"
    );
}

// ---- The Task 5 review's tests: each kills a mutant the twelve and five above
// let live (task-5-review.md). They reuse this file's helpers. ----

use bongo_player::catalog::Titles;
use bongo_player::vod::index::Learned;
use std::collections::HashSet;
use tokio::sync::Notify;

/// A portal of the review's own: its lists in memory, its category lists as
/// the test says, and a note of when the category lists were asked.
struct Gate {
    lists: Lists,
    categories: Cats,
    asked: Notify,
}

/// How [`Gate`]'s three lists answer.
enum Lists {
    /// One film, `Lachen`, at once.
    Film,
    /// `Lachen`, but only once the category lists were asked; 2 s later a
    /// refusal.
    AfterTheCategories,
    /// These films, at once.
    Films(fn() -> Vec<Movie>, fn() -> Vec<Series>),
    /// Refused.
    Refused,
    /// The same list as the one that shows.
    Unchanged,
}

/// How [`Gate`]'s category lists answer.
enum Cats {
    /// These, at once.
    Now(Categories),
    /// Never: the request hangs until it is stopped.
    Never,
    /// These, once the test lets them come.
    After(Mutex<Option<oneshot::Receiver<()>>>, Categories),
}

impl Gate {
    fn new(lists: Lists, categories: Cats) -> Self {
        Self {
            lists,
            categories,
            asked: Notify::new(),
        }
    }
}

fn lachen() -> Catalog {
    Catalog {
        movies: vec![film(5, "Lachen (2020)", false)],
        ..Catalog::default()
    }
}

#[async_trait::async_trait]
impl Portal for Gate {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        match &self.lists {
            Lists::Film => Ok(lachen()),
            Lists::AfterTheCategories => {
                match tokio::time::timeout(Duration::from_secs(2), self.asked.notified()).await {
                    Ok(()) => Ok(lachen()),
                    Err(_) => Err(PortalError::Status(504)),
                }
            }
            Lists::Films(movies, series) => Ok(Catalog {
                movies: movies(),
                series: series(),
                ..Catalog::default()
            }),
            Lists::Refused => Err(PortalError::Status(503)),
            Lists::Unchanged => Err(PortalError::Unchanged),
        }
    }
    async fn fetch_categories(&self) -> Result<Categories, PortalError> {
        self.asked.notify_one();
        match &self.categories {
            Cats::Now(categories) => Ok(categories.clone()),
            Cats::Never => std::future::pending().await,
            Cats::After(gate, categories) => {
                let gate = gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                Ok(categories.clone())
            }
        }
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
    fn live_stream_url(&self, _: i64) -> String {
        String::new()
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

/// Waits (2 s at most) until only the test and the store hold `portal`: every
/// task of a fetch holds it until it ends, the category lists' own included.
async fn only_the_store_holds(portal: &Arc<Gate>) {
    for _ in 0..400 {
        if Arc::strong_count(portal) == 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "a task of the fetch still runs: {} holders",
        Arc::strong_count(portal)
    );
}

#[tokio::test]
async fn the_category_lists_are_asked_beside_the_three_lists() {
    // Ruling 1': beside them, not after them. This portal's lists come only
    // once its category lists were asked.
    let portal = Arc::new(Gate::new(
        Lists::AfterTheCategories,
        Cats::Now(Categories::default()),
    ));
    let store = Arc::new(CatalogStore::new(portal));
    store.spawn_fetch();
    titled(&store).await;
    assert_eq!(store.status(), CatalogStatus::Loaded);
    assert_eq!(store.titles().unwrap().title(0).unwrap().name, "Lachen");
}

#[tokio::test]
async fn a_fetch_that_brings_no_catalog_stops_its_category_lists() {
    // A refused list and an unchanged one: no titles wait for the categories,
    // and their request stops.
    for (lists, status) in [(Lists::Refused, "Failed"), (Lists::Unchanged, "Loaded")] {
        let portal = Arc::new(Gate::new(lists, Cats::Never));
        let store = Arc::new(CatalogStore::new(portal.clone()));
        store.spawn_fetch();
        for _ in 0..400 {
            let now = format!("{:?}", store.status());
            if now.starts_with("Failed") || now == "Loaded" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(format!("{:?}", store.status()).starts_with(status));
        only_the_store_holds(&portal).await;
        assert!(store.titles().is_none());
    }
}

#[tokio::test]
async fn at_the_bound_the_request_for_the_categories_stops() {
    let portal = Arc::new(Gate::new(Lists::Film, Cats::Never));
    let store = Arc::new(
        CatalogStore::new(portal.clone()).with_categories_wait(Duration::from_millis(100)),
    );
    store.spawn_fetch();
    titled(&store).await;
    only_the_store_holds(&portal).await;
    assert_eq!(store.titles().unwrap().title(0).unwrap().genres, 0);
}

#[test]
fn the_bound_is_logged_as_the_wait_that_ran_out() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let text = logged(|| {
        runtime.block_on(async {
            let portal = Arc::new(Gate::new(Lists::Film, Cats::Never));
            let store = Arc::new(
                CatalogStore::new(portal).with_categories_wait(Duration::from_millis(200)),
            );
            store.spawn_fetch();
            titled(&store).await;
        })
    });
    assert!(
        text.contains("catalog: no categories (none within 200ms) - the titles go without"),
        "{text}"
    );
}

/// Three dramas: Dark, the adult Hot, and Ozark.
fn dramas() -> Catalog {
    let drama = |id: i64, name: &str, adult: bool| Series {
        series_id: id,
        name: name.into(),
        genre: Some("Drama".into()),
        is_adult: adult,
        ..Series::default()
    };
    Catalog {
        series: vec![
            drama(1, "Dark", false),
            drama(2, "Hot", true),
            drama(3, "Ozark", false),
        ],
        ..Catalog::default()
    }
}

/// The series ids `titles` holds like series `id`, in their order.
fn like(titles: &Titles, unlocked: &Titles, id: i64) -> Vec<i64> {
    let none = HashSet::new();
    let pos = unlocked
        .find(TitleId {
            kind: Kind::Series,
            id,
        })
        .unwrap();
    titles
        .more_like(pos, 10, &none, &none)
        .into_iter()
        .map(|p| unlocked.title(p as usize).unwrap().id.id)
        .collect()
}

#[test]
fn a_locked_handle_knows_nothing_like_an_adult_title_and_no_adult_title_like_another() {
    let store = CatalogStore::from_catalog(idle_portal(), dramas());
    let locked = store.titles().unwrap();
    store.set_adult_unlocked(true);
    let unlocked = store.titles().unwrap();
    assert_eq!(like(&locked, &unlocked, 1), [3], "not the adult Hot");
    assert_eq!(
        like(&locked, &unlocked, 2),
        Vec::<i64>::new(),
        "none like Hot"
    );
    assert_eq!(like(&unlocked, &unlocked, 1), [2, 3]);
    assert_eq!(like(&unlocked, &unlocked, 2), [1, 3]);
}

#[test]
fn what_a_handle_learns_reaches_the_index_every_handle_shares() {
    let store = CatalogStore::from_catalog(idle_portal(), library());
    let titles = store.titles().unwrap();
    let id = TitleId {
        kind: Kind::Film,
        id: 1,
    };
    let pos = titles.find(id).unwrap();
    assert_eq!(titles.title(pos).unwrap().genres, 0);
    titles.learn(
        pos,
        Learned {
            year: 2001,
            genres: Genre::Action.bit(),
            trailer: true,
        },
    );
    let again = store.titles().unwrap();
    let matrix = again.title(again.find(id).unwrap()).unwrap();
    assert_eq!(
        (matrix.year, matrix.genres, matrix.trailer),
        (1999, Genre::Action.bit(), true),
        "the name's year stays; genres and the trailer are learned"
    );
}

#[test]
fn a_name_may_give_the_year_after_this_one_and_no_later() {
    use chrono::Datelike;
    let year = chrono::Utc::now().year() as u16;
    let catalog = Catalog {
        movies: vec![
            film(1, &format!("Nieuw ({})", year + 1), false),
            film(2, &format!("Later ({})", year + 2), false),
        ],
        ..Catalog::default()
    };
    let store = CatalogStore::from_catalog(idle_portal(), catalog);
    let titles = store.titles().unwrap();
    let title = |id| {
        let t = titles
            .title(
                titles
                    .find(TitleId {
                        kind: Kind::Film,
                        id,
                    })
                    .unwrap(),
            )
            .unwrap();
        (t.name.to_owned(), t.year)
    };
    assert_eq!(title(1), ("Nieuw".to_owned(), year + 1));
    assert_eq!(title(2), (format!("Later ({})", year + 2), 0));
}

#[test]
fn a_catalog_without_titles_logs_no_vod_line() {
    // Ruling 30: once per build that has titles. An M3U list has none.
    let (portal, catalog) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttp://a.example/1\n").unwrap();
    let text = logged(|| {
        CatalogStore::from_catalog(Arc::new(portal), catalog);
    });
    assert!(!text.contains("vod: "), "{text}");
}

/// 400 films with long covers and 30 series: an index and a cover table of
/// sizes apart.
fn sized_films() -> Vec<Movie> {
    (0..400)
        .map(|i| Movie {
            stream_icon: Some(format!("https://c.example/{}/{i:04}.jpg", "x".repeat(280))),
            category_id: 1,
            ..film(i + 1, &format!("Film {i} (2001)"), false)
        })
        .collect()
}

fn sized_series() -> Vec<Series> {
    (0..30)
        .map(|i| Series {
            series_id: i + 1,
            name: format!("Serie {i}"),
            category_id: 1 + i % 3,
            ..Series::default()
        })
        .collect()
}

#[test]
fn the_fetch_logs_both_category_lists_and_the_sizes_it_built() {
    // The fetch builds on the blocking pool: its threads log into the sink too.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let sink = Sink::default();
    let writer = sink.clone();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish(),
    );
    let category = |id: i64, name: &str| Category {
        category_id: id,
        category_name: name.into(),
    };
    let categories = Categories {
        vod: vec![category(1, "NL | Komedie")],
        series: vec![
            category(1, "Drama"),
            category(2, "Actie"),
            category(3, "Kids"),
        ],
    };
    let on_start = dispatch.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .on_thread_start(move || std::mem::forget(tracing::dispatcher::set_default(&on_start)))
        .build()
        .unwrap();
    let cats = categories.clone();
    tracing::dispatcher::with_default(&dispatch, || {
        runtime.block_on(async move {
            let portal = Arc::new(Gate::new(
                Lists::Films(sized_films, sized_series),
                Cats::Now(cats),
            ));
            let store = Arc::new(CatalogStore::new(portal));
            store.spawn_fetch();
            titled(&store).await;
        })
    });
    let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    let line = text
        .lines()
        .find(|l| l.contains("vod: "))
        .unwrap_or_else(|| panic!("{text}"));
    use chrono::Datelike;
    let next = chrono::Utc::now().year() as u16 + 1;
    let index = TitleIndex::build(&sized_films(), &sized_series(), &categories, next);
    let (kb, covers) = (index.heap_bytes() / 1024, index.cover_bytes() / 1024);
    assert!(kb > 0 && covers > kb, "sizes apart: {kb} KB, {covers} KB");
    assert!(
        line.contains(
            "vod: 430 titles (30 series, 400 films in 400 versions, at most 1 in one film, 0 left out), 4 categories - cover 93 %, "
        ),
        "{line}"
    );
    assert!(
        line.contains(&format!(" - index {kb} KB, covers {covers} KB, built in ")),
        "{line}"
    );
}

#[test]
fn a_catalog_replaced_while_its_categories_wait_builds_no_titles() {
    // F5 while the first fetch's category lists are held (the Task 5 review,
    // m8): the second catalog's titles are built and put, and once the first
    // fetch's categories come its titles are built not at all, so the log has
    // one `vod:` line. `Cats::After` takes its gate once: the second fetch's
    // category lists come at once.
    fn films() -> Vec<Movie> {
        static FETCHES: AtomicUsize = AtomicUsize::new(0);
        match FETCHES.fetch_add(1, Ordering::SeqCst) {
            0 => vec![film(5, "Lachen (2020)", false)],
            _ => vec![film(6, "Later (2021)", false)],
        }
    }
    // The builds run on the blocking pool: its threads log into the sink too.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let sink = Sink::default();
    let writer = sink.clone();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish(),
    );
    let on_start = dispatch.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .on_thread_start(move || std::mem::forget(tracing::dispatcher::set_default(&on_start)))
        .build()
        .unwrap();
    let (release, gate) = oneshot::channel();
    tracing::dispatcher::with_default(&dispatch, || {
        runtime.block_on(async move {
            let portal = Arc::new(Gate::new(
                Lists::Films(films, Vec::new),
                Cats::After(Mutex::new(Some(gate)), Categories::default()),
            ));
            let store = Arc::new(CatalogStore::new(portal.clone()));
            store.spawn_fetch();
            loaded(&store).await;
            store.spawn_fetch();
            loaded(&store).await;
            titled(&store).await;
            let name =
                |store: &CatalogStore| store.titles().unwrap().title(0).unwrap().name.to_owned();
            assert_eq!(name(&store), "Later", "the second catalog's titles");
            release.send(()).unwrap();
            only_the_store_holds(&portal).await;
            assert_eq!(name(&store), "Later");
            assert_eq!(store.titles_revision(), 1);
        })
    });
    let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.matches("vod: ").count(), 1, "{text}");
}
