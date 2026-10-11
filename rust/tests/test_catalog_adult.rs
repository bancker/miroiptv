//! The catalog hides adult channels and films while locked, everywhere the
//! app takes its lists from.
use bongo_player::catalog::CatalogStore;
use bongo_player::epg::Epg;
use bongo_player::portal::{Catalog, Episode, LiveChannel, Movie, Portal, PortalError, Series};
use std::sync::Arc;

struct NullPortal;

#[async_trait::async_trait]
impl Portal for NullPortal {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        Ok(Catalog::default())
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
    }
}

fn movie(id: i64, name: &str, adult: bool) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        container_extension: Some("mp4".into()),
        is_adult: adult,
    }
}

fn store() -> CatalogStore {
    CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                live(1, "NL | NPO 1", false),
                live(2, "24/7 | SHOPLYFTER 1", true),
                live(3, "XXX | MEIDEN VAN HOLLAND", false), // the name rule
            ],
            movies: vec![
                movie(10, "Gewone film", false),
                movie(11, "Adult film", true),
            ],
            series: Vec::new(),
        },
    )
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
    }
}

#[test]
fn locked_hides_adult_channels_and_films_everywhere() {
    let s = store();
    assert_eq!(ids(s.zap_channels()), [1]);
    assert_eq!(ids(s.live_channels()), [1]);
    let found: Vec<i64> = s.search_items().into_iter().map(|i| i.id).collect();
    assert_eq!(found, [1, 10]);
    assert_eq!(s.live_id_by_name("XXX | MEIDEN VAN HOLLAND"), None);
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
    assert!(s.is_adult_url("http://h/live/u/p/3.m3u8"), "name rule");
    assert!(s.is_adult_url("http://h/movie/u/p/11.mp4"));
    assert!(!s.is_adult_url("http://h/live/u/p/1.m3u8"));
    assert!(!s.is_adult_url("http://127.0.0.1:5555/live/0.ts"));
    s.set_adult_unlocked(true);
    assert!(s.is_adult_url("http://h/live/u/p/2.m3u8"));
}

#[test]
fn saved_favorites_and_presets_hide_while_locked() {
    let s = store();
    assert!(s.is_hidden_live(2, "24/7 | SHOPLYFTER 1"));
    assert!(
        s.is_hidden_live(99, "XXX | Onbekend"),
        "name rule, id not in the catalog"
    );
    assert!(!s.is_hidden_live(1, "NL | NPO 1"));
    s.set_adult_unlocked(true);
    assert!(!s.is_hidden_live(2, "24/7 | SHOPLYFTER 1"));
}

#[test]
fn locked_hides_adult_catch_up_channels_and_their_name_lookups() {
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            live: vec![
                archive(1, "NL | NPO 1 HD", false),
                archive(2, "24/7 | SHOPLYFTER 1", true),
                archive(3, "XXX | MEIDEN VAN HOLLAND", false), // the name rule
            ],
            ..Catalog::default()
        },
    );
    assert_eq!(ids(s.archive_channels()), [1]);
    assert_eq!(s.archive_id_by_name("NL | NPO 1"), Some(1));
    assert_eq!(s.archive_id_by_name("24/7 | SHOPLYFTER 1"), None);
    assert_eq!(s.archive_id_by_name("XXX | MEIDEN VAN HOLLAND"), None);
    s.set_adult_unlocked(true);
    assert_eq!(ids(s.archive_channels()), [1, 2, 3]);
    assert_eq!(s.archive_id_by_name("24/7 | SHOPLYFTER 1"), Some(2));
}

#[test]
fn search_leaves_out_adult_films_and_series_while_locked() {
    let s = CatalogStore::from_catalog(
        Arc::new(NullPortal),
        Catalog {
            movies: vec![
                movie(10, "Zomer film", false),
                movie(11, "Zomer adult", true),
                movie(12, "XXX Zomer film", false), // the name rule
            ],
            series: vec![
                series(20, "Zomer serie", false),
                series(21, "Zomer flagged", true),
                series(22, "XXX Zomer", false), // the name rule
            ],
            ..Catalog::default()
        },
    );
    let hits = |s: &CatalogStore| {
        let mut found: Vec<i64> = s.search("zomer").into_iter().map(|i| i.id).collect();
        found.sort();
        found
    };
    assert_eq!(hits(&s), [10, 20]);
    s.set_adult_unlocked(true);
    assert_eq!(hits(&s), [10, 11, 12, 20, 21, 22]);
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
fn a_store_without_a_catalog_does_not_panic_and_still_applies_the_name_rule() {
    let s = CatalogStore::new(Arc::new(NullPortal));
    assert!(s.is_hidden_live(2, "XXX | Onbekend"));
    assert!(s.zap_channels().is_empty());
    assert!(s.search_items().is_empty());
    assert!(s.quality_nav(1).ladder.is_empty());
}
