//! The catalog hides adult channels and films while locked, everywhere the
//! app takes its lists from.
use bongo_player::catalog::{fallback_variant, CatalogStore, QualityNav};
use bongo_player::epg::Epg;
use bongo_player::portal::{Catalog, Episode, LiveChannel, Movie, Portal, PortalError, Series};
use std::sync::Arc;
use std::time::Duration;

/// Has nothing to say, except for the catalog it is asked to fetch: `catalog()`.
struct NullPortal;

#[async_trait::async_trait]
impl Portal for NullPortal {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        Ok(catalog())
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
    fn live_stream_id(&self, url: &str) -> Option<i64> {
        bongo_player::portal::xtream::stream_ref(url).map(|(_, id)| id)
    }
    fn movie_stream_url(&self, id: i64, ext: &str) -> String {
        format!("http://h/movie/u/p/{id}.{ext}")
    }
    fn series_stream_url(&self, id: &str, ext: &str) -> String {
        format!("http://h/series/u/p/{id}.{ext}")
    }
    fn catchup_url(&self, id: i64, _: chrono::DateTime<chrono::Utc>, _: u32) -> String {
        format!("http://h/timeshift/u/p/30/x/{id}.m3u8")
    }
}

fn live(id: i64, name: &str, adult: bool) -> LiveChannel {
    LiveChannel {
        stream_id: id,
        name: name.into(),
        category_id: None,
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult: adult,
        stream_icon: None,
    }
}

fn movie(id: i64, name: &str, adult: bool) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        container_extension: Some("mp4".into()),
        is_adult: adult,
        ..Movie::default()
    }
}

fn catalog() -> Catalog {
    Catalog {
        live: vec![
            live(1, "NL | NPO 1", false),
            live(2, "24/7 | SHOPLYFTER 1", true),
            live(3, "XXX | MEIDEN VAN HOLLAND", true),
        ],
        movies: vec![
            movie(10, "Gewone film", false),
            movie(11, "Adult film", true),
        ],
        series: Vec::new(),
    }
}

fn store() -> CatalogStore {
    CatalogStore::from_catalog(Arc::new(NullPortal), catalog())
}

fn ids(v: Vec<LiveChannel>) -> Vec<i64> {
    v.into_iter().map(|c| c.stream_id).collect()
}

fn archive(id: i64, name: &str, adult: bool) -> LiveChannel {
    LiveChannel {
        tv_archive: 1,
        ..live(id, name, adult)
    }
}

fn series(id: i64, name: &str, adult: bool) -> Series {
    Series {
        series_id: id,
        name: name.into(),
        is_adult: adult,
        ..Series::default()
    }
}

fn rungs(nav: &QualityNav) -> Vec<i64> {
    nav.ladder.iter().map(|c| c.stream_id).collect()
}

#[test]
fn locked_hides_adult_channels_and_films_everywhere() {
    let s = store();
    assert_eq!(ids(s.zap_channels()), [1]);
    assert_eq!(ids(s.live_channels()), [1]);
    let found: Vec<i64> = s.search_items().into_iter().map(|i| i.id).collect();
    assert_eq!(found, [1, 10]);
    assert!(s.quality_nav(2).ladder.is_empty());
}

#[test]
fn unlocking_shows_everything_until_locked_again() {
    let s = store();
    s.set_adult_unlocked(true);
    assert!(s.adult_unlocked());
    assert_eq!(s.zap_channels().len(), 3);
    assert_eq!(s.search_items().len(), 5);
    s.set_adult_unlocked(false);
    assert_eq!(ids(s.zap_channels()), [1]);
}

#[test]
fn knows_which_urls_play_adult_content_whatever_the_lock() {
    let s = store();
    assert!(s.is_adult_url("http://h/live/u/p/2.m3u8"));
    assert!(s.is_adult_url("http://h/live/u/p/3.m3u8"));
    assert!(s.is_adult_url("http://h/movie/u/p/11.mp4"));
    assert!(!s.is_adult_url("http://h/live/u/p/1.m3u8"));
    assert!(!s.is_adult_url("http://127.0.0.1:5555/live/0.ts"));
    s.set_adult_unlocked(true);
    assert!(s.is_adult_url("http://h/live/u/p/2.m3u8"));
}

#[test]
fn saved_favorites_and_presets_hide_while_locked() {
    let s = store();
    assert!(s.is_hidden_live(2));
    assert!(!s.is_hidden_live(1));
    assert!(
        !s.is_hidden_live(99),
        "an id the catalog does not know has nothing against it"
    );
    s.set_adult_unlocked(true);
    assert!(!s.is_hidden_live(2));
}

#[test]
fn locked_hides_adult_catch_up_channels_and_their_name_lookups() {
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                archive(1, "NL | NPO 1 HD", false),
                archive(2, "24/7 | SHOPLYFTER 1", true),
                archive(3, "XXX | MEIDEN VAN HOLLAND", true),
            ],
            ..Catalog::default()
        },
    );
    // The guide's columns are the catch-up channels the app lists.
    let columns = |s: &CatalogStore| -> Vec<i64> {
        s.guide_channels().iter().map(|c| c.archive_id).collect()
    };
    assert_eq!(columns(&s), [1]);
    assert_eq!(s.archive_id_by_name("NL | NPO 1"), Some(1));
    assert_eq!(s.archive_id_by_name("24/7 | SHOPLYFTER 1"), None);
    assert_eq!(s.archive_id_by_name("XXX | MEIDEN VAN HOLLAND"), None);
    s.set_adult_unlocked(true);
    assert_eq!(columns(&s), [1, 2, 3]);
    assert_eq!(s.archive_id_by_name("24/7 | SHOPLYFTER 1"), Some(2));
}

#[test]
fn search_leaves_out_flagged_films_and_series_while_locked() {
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            movies: vec![
                movie(10, "Zomer film", false),
                movie(11, "Zomer adult", true),
                movie(12, "XXX Zomer film", false), // a name says nothing
            ],
            series: vec![
                series(20, "Zomer serie", false),
                series(21, "Zomer flagged", true),
                series(22, "XXX Zomer", false), // a name says nothing
            ],
            ..Catalog::default()
        },
    );
    let hits = |s: &CatalogStore| {
        let mut found: Vec<i64> = s.search("zomer").into_iter().map(|i| i.id).collect();
        found.sort();
        found
    };
    assert_eq!(hits(&s), [10, 12, 20, 22]);
    s.set_adult_unlocked(true);
    assert_eq!(hits(&s), [10, 11, 12, 20, 21, 22]);
}

#[test]
fn only_the_portals_flag_marks_adult_content_never_a_name() {
    // The film xXx: State of the Union and a channel called "XXX Something" carry
    // no flag: they are ordinary content and show while locked. A flagged entry
    // hides, whatever it is called.
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                live(1, "NL | NPO 1", false),
                live(2, "XXX Something", false),
                live(3, "xxx | Flagged channel", true),
                live(4, "Quietly flagged", true),
            ],
            movies: vec![
                movie(10, "xXx: State of the Union", false),
                movie(11, "XXX", false),
                movie(12, "Flagged film", true),
            ],
            series: vec![
                series(20, "xXx Reloaded", false),
                series(21, "Flagged serie", true),
            ],
        },
    );
    let listed = |s: &CatalogStore| {
        let mut found: Vec<i64> = s.search_items().into_iter().map(|i| i.id).collect();
        found.sort();
        found
    };
    // Locked: the unflagged ones show everywhere, the flagged ones nowhere.
    assert_eq!(ids(s.zap_channels()), [1, 2]);
    assert_eq!(ids(s.live_channels()), [1, 2]);
    assert_eq!(listed(&s), [1, 2, 10, 11, 20]);
    let hits = s.search("state of the union");
    assert_eq!(hits.iter().map(|i| i.id).collect::<Vec<_>>(), [10]);
    assert!(!s.quality_nav(2).ladder.is_empty());
    assert!(s.quality_nav(3).ladder.is_empty());
    // What plays and what is saved follow the flag too.
    assert!(!s.is_adult_url("http://h/live/u/p/2.m3u8"));
    assert!(s.is_adult_url("http://h/live/u/p/3.m3u8"));
    assert!(!s.is_adult_url("http://h/movie/u/p/10.mp4"));
    assert!(s.is_adult_url("http://h/movie/u/p/12.mp4"));
    assert!(!s.is_hidden_live(2));
    assert!(s.is_hidden_live(3));
    // Unlocked: all of them.
    s.set_adult_unlocked(true);
    assert_eq!(ids(s.zap_channels()), [1, 2, 3, 4]);
    assert_eq!(listed(&s), [1, 2, 3, 4, 10, 11, 12, 20, 21]);
}

#[test]
fn catch_up_urls_follow_their_channel_and_episode_ids_are_not_stream_ids() {
    let s = store();
    assert!(s.is_adult_url("http://h/timeshift/u/p/30/x/2.m3u8"));
    assert!(!s.is_adult_url("http://h/timeshift/u/p/30/x/1.m3u8"));
    // 11 is an adult film's id, but a series URL carries an episode id.
    assert!(!s.is_adult_url("http://h/series/u/p/11.mkv"));
}

#[test]
fn a_store_without_a_catalog_does_not_panic_and_hides_nothing() {
    let s = CatalogStore::new(Arc::new(NullPortal));
    assert!(!s.is_hidden_live(2));
    assert!(s.zap_channels().is_empty());
    assert!(s.search_items().is_empty());
    assert!(s.quality_nav(1).ladder.is_empty());
}

#[test]
fn a_hidden_variant_is_no_rung_on_a_visible_channels_ladder() {
    // Channel 2 is flagged as adult but has channel 1's channel name.
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                live(1, "NL | Foo HD", false),
                live(2, "XXX | Foo FHD", true),
            ],
            ..Catalog::default()
        },
    );
    let nav = s.quality_nav(1);
    assert_eq!(rungs(&nav), [1]);
    assert_eq!(nav.pos, Some(0));
    // The 407 fallback has nothing hidden to switch to.
    assert!(fallback_variant(&nav.ladder, 1, |_| false).is_none());
    s.set_adult_unlocked(true);
    let nav = s.quality_nav(1);
    assert_eq!(rungs(&nav), [1, 2]);
    assert_eq!(nav.pos, Some(0));
    let alt = fallback_variant(&nav.ladder, 1, |_| false);
    assert_eq!(alt.map(|c| c.stream_id), Some(2));
}

#[test]
fn the_ladder_position_counts_only_the_rungs_that_show() {
    // Hidden 2 (HD) sits below the playing 1 (FHD), hidden 3 (UHD) above it.
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                live(1, "NL | Foo FHD", false),
                live(2, "XXX | Foo HD", true),
                live(3, "XXX | Foo UHD", true),
            ],
            ..Catalog::default()
        },
    );
    let nav = s.quality_nav(1);
    assert_eq!(rungs(&nav), [1]);
    assert_eq!(nav.pos, Some(0));
    s.set_adult_unlocked(true);
    let nav = s.quality_nav(1);
    assert_eq!(rungs(&nav), [2, 1, 3]);
    assert_eq!(nav.pos, Some(1));
}

#[test]
fn a_saved_favorite_hides_by_what_the_catalog_knows_of_its_id() {
    // 2 and 3 carry the portal's flag: whatever name a favorite or preset was
    // saved under, only the catalog decides.
    let s = store();
    assert!(s.is_hidden_live(2));
    assert!(s.is_hidden_live(3));
    assert!(!s.is_hidden_live(1));
    assert!(!s.is_hidden_live(4), "an unknown id stays");
    s.set_adult_unlocked(true);
    assert!(!s.is_hidden_live(2));
    assert!(!s.is_hidden_live(3));
}

#[test]
fn a_catch_up_url_of_a_flagged_channel_is_adult() {
    let s = store();
    assert!(s.is_adult_url("http://h/timeshift/u/p/30/x/3.m3u8"));
    assert!(s.is_adult_url("http://h/live/u/p/3.m3u8"));
}

#[tokio::test]
async fn a_fetched_catalog_knows_its_adult_channels_like_one_handed_over_whole() {
    // The fetch stores its catalog its own way; the lookups must not differ.
    let s = Arc::new(CatalogStore::new(Arc::new(NullPortal)));
    assert!(!s.is_hidden_live(2), "nothing is known yet");
    s.spawn_fetch();
    for _ in 0..100 {
        if s.is_loaded() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(s.is_loaded(), "the fetch did not finish");
    assert!(s.is_hidden_live(2), "the portal's flag");
    assert!(s.is_hidden_live(3), "the portal's flag");
    assert!(!s.is_hidden_live(1));
    assert!(s.is_adult_url("http://h/live/u/p/2.m3u8"));
    assert!(s.is_adult_url("http://h/timeshift/u/p/30/x/3.m3u8"));
    assert!(!s.is_adult_url("http://h/live/u/p/1.m3u8"));
    assert_eq!(ids(s.zap_channels()), [1]);
}
