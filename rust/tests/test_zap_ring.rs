//! The zap ring (catalog.rs): what up/down zapping steps through, worked out
//! once per catalog, lock and marks, with the defect channels out.

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{CatalogStore, ZapFrom, ZapRing};
use bongo_player::m3u::stream_id;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, LiveChannel};
use std::collections::HashMap;
use std::sync::Arc;

/// Four channels; with no tvg-id their keys, and so their stream ids, come
/// from their names.
const LIST: &str = "#EXTM3U\n\
#EXTINF:-1,Een\nhttps://a.example/1.m3u8\n\
#EXTINF:-1,Twee\nhttps://a.example/2.m3u8\n\
#EXTINF:-1,Drie\nhttps://a.example/3.m3u8\n\
#EXTINF:-1,Vier\nhttps://a.example/4.m3u8\n";

/// 15 October 2026, 00:00 UTC.
const FAILED: i64 = 1_792_022_400;

fn m3u_store() -> CatalogStore {
    let (portal, catalog) = M3uPortal::from_text(LIST).unwrap();
    CatalogStore::from_catalog(Arc::new(portal), catalog)
}

fn mark(store: &CatalogStore, names: &[&str]) {
    store.set_defect(names.iter().map(|n| (stream_id(n), FAILED)).collect());
}

fn ring(store: &CatalogStore) -> Arc<ZapRing> {
    store.zap_ring().expect("a loaded catalog")
}

fn names(ring: &ZapRing) -> Vec<&str> {
    ring.channels().map(|c| c.name.as_str()).collect()
}

/// The name of the channel `delta` steps on from `from`.
fn step(ring: &ZapRing, from: ZapFrom, delta: i32) -> &str {
    let pos = ring.step(from, delta).expect("a ring with channels");
    ring.get(pos).unwrap().name.as_str()
}

#[test]
fn a_defect_channel_is_out_of_the_ring_until_its_mark_goes() {
    let store = m3u_store();
    assert_eq!(names(&ring(&store)), ["Een", "Twee", "Drie", "Vier"]);
    mark(&store, &["Twee"]);
    assert_eq!(names(&ring(&store)), ["Een", "Drie", "Vier"]);
    assert!(store.is_defect(stream_id("Twee")));
    assert_eq!(store.defect_marks().get(&stream_id("Twee")), Some(&FAILED));
    store.set_defect(HashMap::new());
    assert_eq!(names(&ring(&store)), ["Een", "Twee", "Drie", "Vier"]);
    assert!(!store.is_defect(stream_id("Twee")));
}

#[test]
fn the_ring_is_worked_out_once_per_catalog_lock_and_marks() {
    let store = m3u_store();
    let first = ring(&store);
    assert!(Arc::ptr_eq(&first, &ring(&store)), "a press copies nothing");
    mark(&store, &["Drie"]);
    let marked = ring(&store);
    assert!(!Arc::ptr_eq(&first, &marked), "new marks, a new ring");
    assert!(Arc::ptr_eq(&marked, &ring(&store)));
    store.set_adult_unlocked(true);
    assert!(
        !Arc::ptr_eq(&marked, &ring(&store)),
        "the lock is part of it"
    );
    assert_eq!(store.defect_revision(), 1);
}

#[test]
fn a_defect_channel_that_plays_steps_from_its_place_in_the_file() {
    let store = m3u_store();
    mark(&store, &["Twee", "Vier"]);
    let ring = ring(&store);
    let twee = ZapFrom::Channel(stream_id("Twee"));
    assert_eq!(step(&ring, twee, 1), "Drie");
    assert_eq!(step(&ring, twee, -1), "Een");
    let vier = ZapFrom::Channel(stream_id("Vier"));
    assert_eq!(step(&ring, vier, 1), "Een", "round the ring");
    assert_eq!(step(&ring, vier, -1), "Drie");
    assert_eq!(
        ring.position_of(stream_id("Twee")),
        None,
        "it has no place in the ring"
    );
}

#[test]
fn a_position_that_holds_another_channel_now_is_found_again_by_its_channel() {
    let store = m3u_store();
    let drie = stream_id("Drie");
    // Drie was at 2; with Een marked it is at 1.
    mark(&store, &["Een"]);
    let ring = ring(&store);
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 2, sid: drie }, 1), "Vier");
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 2, sid: drie }, -1), "Twee");
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 1, sid: drie }, 1), "Vier");
}

#[test]
fn with_nothing_known_a_step_counts_from_the_start_as_before() {
    let store = m3u_store();
    let ring = ring(&store);
    assert_eq!(
        step(&ring, ZapFrom::Start, 1),
        "Twee",
        "as next_live_idx(None, ..)"
    );
    assert_eq!(step(&ring, ZapFrom::Start, -1), "Vier");
    assert_eq!(
        step(&ring, ZapFrom::Channel(42), 1),
        "Twee",
        "a channel the list does not have"
    );
}

#[test]
fn a_ring_with_every_channel_marked_has_no_step() {
    let store = m3u_store();
    mark(&store, &["Een", "Twee", "Drie", "Vier"]);
    let ring = ring(&store);
    assert!(ring.is_empty());
    assert_eq!(ring.step(ZapFrom::Start, 1), None);
    assert!(store.zap_channels().is_empty());
}

#[test]
fn the_search_and_the_names_still_know_a_defect_channel() {
    let store = m3u_store();
    mark(&store, &["Twee"]);
    let found: Vec<String> = store.search("twee").into_iter().map(|i| i.name).collect();
    assert_eq!(
        found,
        ["Twee"],
        "the search names it; the app adds the label"
    );
    assert_eq!(
        store.live_name(stream_id("Twee")).as_deref(),
        Some("Twee"),
        "a preset still plays it"
    );
    assert_eq!(store.live_name(42), None);
}

#[test]
fn an_unloaded_catalog_has_no_ring() {
    let (portal, _) = M3uPortal::from_text(LIST).unwrap();
    let store = CatalogStore::new(Arc::new(portal));
    assert!(store.zap_ring().is_none());
    assert!(store.zap_channels().is_empty());
    assert_eq!(store.live_name(stream_id("Een")), None);
}

// ---- an Xtream portal ----

fn xtream(live: Vec<LiveChannel>) -> CatalogStore {
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    });
    let catalog = Catalog {
        live,
        ..Catalog::default()
    };
    CatalogStore::from_catalog(Arc::new(portal), catalog)
}

fn ch(stream_id: i64, name: &str, is_adult: bool) -> LiveChannel {
    LiveChannel {
        stream_id,
        name: name.into(),
        category_id: None,
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult,
        stream_icon: None,
    }
}

#[test]
fn an_xtream_variant_out_of_the_ring_steps_from_the_entry_that_stands_for_it() {
    let store = xtream(vec![
        ch(1, "NL | NPO 1 HD", false),
        ch(2, "NL | NPO 1 FHD", false),
        ch(3, "NL | RTL 4", false),
        ch(4, "NL | NOS", false),
    ]);
    let ring = ring(&store);
    assert_eq!(names(&ring), ["NL | NPO 1 FHD", "NL | RTL 4", "NL | NOS"]);
    assert_eq!(
        ring.position_of(1),
        Some(0),
        "the FHD entry stands for NPO 1 HD"
    );
    assert_eq!(step(&ring, ZapFrom::Channel(1), 1), "NL | RTL 4");
    assert_eq!(step(&ring, ZapFrom::Channel(1), -1), "NL | NOS");
}

#[test]
fn the_xtream_ring_follows_the_lock() {
    let store = xtream(vec![ch(1, "NL | NPO 1", false), ch(9, "XXX | NACHT", true)]);
    assert_eq!(names(&ring(&store)), ["NL | NPO 1"]);
    assert_eq!(ring(&store).position_of(9), None, "hidden while locked");
    store.set_adult_unlocked(true);
    assert_eq!(names(&ring(&store)), ["NL | NPO 1", "XXX | NACHT"]);
}

// ---- beyond the brief's ten: what its tests leave unpinned ----

#[test]
fn a_defect_channel_before_the_first_entry_steps_back_to_the_last() {
    let store = m3u_store();
    mark(&store, &["Een"]);
    let ring = ring(&store);
    let een = ZapFrom::Channel(stream_id("Een"));
    assert_eq!(
        step(&ring, een, 1),
        "Twee",
        "on to the first entry after it"
    );
    assert_eq!(step(&ring, een, -1), "Vier", "back round the ring");
}

#[test]
fn a_name_shows_only_when_its_channel_does() {
    let store = xtream(vec![ch(1, "NL | NPO 1", false), ch(9, "XXX | NACHT", true)]);
    assert_eq!(store.live_name(1).as_deref(), Some("NL | NPO 1"));
    assert_eq!(store.live_name(9), None, "adult, while locked");
    store.set_adult_unlocked(true);
    assert_eq!(store.live_name(9).as_deref(), Some("XXX | NACHT"));
}

#[test]
fn an_entry_is_the_name_and_the_catch_up_flag_of_a_channel_that_shows() {
    let archive = |stream_id, name: &str, is_adult| LiveChannel {
        tv_archive: 1,
        ..ch(stream_id, name, is_adult)
    };
    let store = xtream(vec![
        ch(1, "NL | NPO 1", false),
        archive(2, "NL | NPO 1 HD", false),
        archive(9, "XXX | NACHT", true),
    ]);
    let entry = |sid| store.live_entry(sid);
    assert_eq!(entry(1), Some(("NL | NPO 1".to_string(), false)));
    assert_eq!(entry(2), Some(("NL | NPO 1 HD".to_string(), true)));
    assert_eq!(entry(9), None, "adult, while locked");
    assert_eq!(entry(42), None, "not in the catalog");
    store.set_adult_unlocked(true);
    assert_eq!(entry(9), Some(("XXX | NACHT".to_string(), true)));
    // It shows what live_name shows, whatever the lock says.
    for locked in [true, false] {
        store.set_adult_unlocked(!locked);
        for sid in [1, 2, 9, 42] {
            let name = entry(sid).map(|(name, _)| name);
            assert_eq!(name, store.live_name(sid), "{sid}, locked: {locked}");
        }
    }
}

#[test]
fn a_defect_channel_still_has_an_entry_and_an_unloaded_catalog_has_none() {
    let store = m3u_store();
    mark(&store, &["Twee"]);
    assert_eq!(
        store.live_entry(stream_id("Twee")),
        Some(("Twee".to_string(), false)),
        "a preset still plays it"
    );
    let (portal, _) = M3uPortal::from_text(LIST).unwrap();
    let unloaded = CatalogStore::new(Arc::new(portal));
    assert_eq!(unloaded.live_entry(stream_id("Een")), None);
}

#[test]
fn a_hidden_variant_has_no_place_even_when_its_channel_shows_in_the_ring() {
    // Locked, the ring has the plain "NL | FOO"; the flagged HD variant is
    // hidden, so it has no place, not the place of that entry.
    let store = xtream(vec![ch(1, "NL | FOO HD", true), ch(2, "NL | FOO", false)]);
    let locked = ring(&store);
    assert_eq!(names(&locked), ["NL | FOO"]);
    assert_eq!(locked.position_of(2), Some(0));
    assert_eq!(locked.position_of(1), None, "hidden while locked");
    store.set_adult_unlocked(true);
    let unlocked = ring(&store);
    assert_eq!(names(&unlocked), ["NL | FOO HD"], "the HD variant wins");
    assert_eq!(unlocked.position_of(1), Some(0));
    assert_eq!(
        unlocked.position_of(2),
        Some(0),
        "it stands for the plain one"
    );
}
