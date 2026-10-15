use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{guide_channels_of, CatalogStore};
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, LiveChannel};
use std::sync::Arc;

fn ch(id: i64, name: &str, archive: i32) -> LiveChannel {
    LiveChannel {
        stream_id: id,
        name: name.into(),
        category_id: None,
        epg_channel_id: None,
        tv_archive: archive,
        tv_archive_duration: if archive == 1 { 7 } else { 0 },
        is_adult: false,
        stream_icon: None,
    }
}

fn with_icon(mut c: LiveChannel, icon: &str) -> LiveChannel {
    c.stream_icon = Some(icon.into());
    c
}

fn adult(mut c: LiveChannel) -> LiveChannel {
    c.is_adult = true;
    c
}

fn portal() -> Arc<XtreamPortal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }))
}

fn store(live: Vec<LiveChannel>) -> CatalogStore {
    CatalogStore::from_catalog(
        portal(),
        Catalog {
            live,
            ..Default::default()
        },
    )
}

#[test]
fn a_channel_with_a_catch_up_twin_is_one_column() {
    let live = vec![
        ch(501, "NL | NPO 1", 0),
        ch(502, "NL | NPO 1 UHD", 0),
        ch(503, "NL | NPO 1 FHD", 0),
        ch(601, "NL | NPO1 HD", 1),
    ];
    let g = guide_channels_of(&live);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].archive_id, 601);
    assert_eq!(g[0].live_id, 503, "the variant zapping picks: FHD first");
    assert_eq!(g[0].name, "NL | NPO 1 FHD");
    assert_eq!(g[0].archive_days, 7);
    assert!(!g[0].adult);
}

#[test]
fn a_channel_without_catch_up_is_no_column() {
    let live = [ch(1, "INFO CHANNEL PANDORA", 0), ch(2, "NL | OPENHAARD", 0)];
    assert!(guide_channels_of(&live).is_empty());
}

/// An M3U list has no catch-up (`tv_archive` 0 on every entry), so it has no
/// guide columns, however many entries share a name or a quality, and the PIN
/// changes nothing. This pins that result, which the early exit in
/// `guide_channels_of` must not change; it fails when a list without catch-up
/// gets a column, e.g. from an early exit that returns the entries.
#[test]
fn a_list_without_catch_up_has_no_guide_columns() {
    let (portal, catalog) = M3uPortal::from_text(
        "#EXTM3U\n\
         #EXTINF:-1 tvg-id=\"NPO1.nl\",NL | NPO 1\nhttps://a.example/npo1.m3u8\n\
         #EXTINF:-1,NL | NPO 1 HD\nhttps://a.example/npo1hd.m3u8\n\
         #EXTINF:-1,NL | RTL 4\nhttps://a.example/rtl4.m3u8\n",
    )
    .unwrap();
    assert_eq!(catalog.live.len(), 3);
    assert!(catalog.live.iter().all(|c| c.tv_archive == 0));
    assert!(guide_channels_of(&catalog.live).is_empty());
    let store = CatalogStore::from_catalog(Arc::new(portal), catalog);
    assert!(store.guide_channels().is_empty());
    store.set_adult_unlocked(true);
    assert!(store.guide_channels().is_empty(), "the PIN changes nothing");
}

#[test]
fn a_catch_up_channel_without_a_live_one_plays_live_through_itself() {
    let g = guide_channels_of(&[ch(701, "NL | REGIO HD", 1)]);
    assert_eq!((g[0].live_id, g[0].archive_id), (701, 701));
    assert_eq!(g[0].name, "NL | REGIO HD");
}

#[test]
fn columns_keep_the_order_in_which_channels_first_appear() {
    let live = vec![
        ch(602, "NL | NPO2 HD", 1),
        ch(501, "NL | NPO 1", 0),
        ch(502, "NL | NPO 2", 0),
        ch(601, "NL | NPO1 HD", 1),
    ];
    let ids: Vec<i64> = guide_channels_of(&live)
        .iter()
        .map(|g| g.archive_id)
        .collect();
    assert_eq!(ids, [602, 601]);
}

#[test]
fn the_logo_comes_from_the_live_variant_else_the_catch_up_one() {
    let live = vec![
        with_icon(ch(501, "NL | NPO 1", 0), "  "),
        with_icon(ch(601, "NL | NPO1 HD", 1), "http://logos/npo1.png"),
    ];
    assert_eq!(
        guide_channels_of(&live)[0].logo.as_deref(),
        Some("http://logos/npo1.png")
    );
    let live = vec![
        with_icon(ch(501, "NL | NPO 1", 0), "http://logos/live.png"),
        with_icon(ch(601, "NL | NPO1 HD", 1), "http://logos/tk.png"),
    ];
    assert_eq!(
        guide_channels_of(&live)[0].logo.as_deref(),
        Some("http://logos/live.png")
    );
    assert_eq!(
        guide_channels_of(&[ch(601, "NL | NPO1 HD", 1)])[0].logo,
        None
    );
}

#[test]
fn the_stored_list_keeps_no_logos() {
    let catalog = store(vec![
        with_icon(ch(501, "NL | NPO 1", 0), "http://logos/live.png"),
        with_icon(ch(601, "NL | NPO1 HD", 1), "http://logos/tk.png"),
    ]);
    let live = catalog.live_channels();
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|c| c.stream_icon.is_none()), "{live:?}");
    let zap = catalog.zap_channels();
    assert_eq!(zap.len(), 1);
    assert!(zap.iter().all(|c| c.stream_icon.is_none()), "{zap:?}");
    assert_eq!(
        catalog.guide_channels()[0].logo.as_deref(),
        Some("http://logos/live.png"),
        "the guide's own list still has the logo"
    );
}

#[test]
fn the_catalog_leaves_adult_columns_out_while_locked() {
    let catalog = store(vec![
        ch(501, "NL | NPO 1", 0),
        ch(601, "NL | NPO1 HD", 1),
        adult(ch(901, "24/7 | ADULT A", 0)),
        adult(ch(911, "24/7 | ADULT A HD", 1)),
    ]);
    let ids = |c: &CatalogStore| {
        c.guide_channels()
            .iter()
            .map(|g| g.archive_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&catalog), [601]);
    catalog.set_adult_unlocked(true);
    assert_eq!(ids(&catalog), [601, 911]);
    assert!(catalog.guide_channels()[1].adult);
}

/// `live` holds the variants of one channel: its column is adult, left out of
/// the catalog's list while locked and there once unlocked.
fn assert_one_adult_column(live: Vec<LiveChannel>) {
    let g = guide_channels_of(&live);
    assert_eq!(g.len(), 1);
    assert!(g[0].adult);
    let catalog = store(live);
    assert!(catalog.guide_channels().is_empty(), "locked: left out");
    catalog.set_adult_unlocked(true);
    assert_eq!(catalog.guide_channels().len(), 1, "unlocked: there");
}

#[test]
fn a_column_is_adult_when_only_its_live_variant_is_flagged() {
    assert_one_adult_column(vec![
        adult(ch(901, "24/7 | ADULT B", 0)),
        ch(911, "24/7 | ADULT B HD", 1),
    ]);
}

#[test]
fn a_column_is_adult_when_only_its_catch_up_variant_is_flagged() {
    assert_one_adult_column(vec![
        ch(901, "24/7 | ADULT B", 0),
        adult(ch(911, "24/7 | ADULT B HD", 1)),
    ]);
}

#[test]
fn an_unloaded_catalog_has_no_guide_channels() {
    assert!(CatalogStore::new(portal()).guide_channels().is_empty());
}

#[test]
fn the_catalog_counts_every_load_and_not_the_lock() {
    assert_eq!(CatalogStore::new(portal()).revision(), 0, "nothing loaded");
    let s = store(vec![ch(601, "NL | NPO1 HD", 1)]);
    assert_eq!(s.revision(), 1, "the catalog in the store is the first");
    s.set_adult_unlocked(true);
    s.set_adult_unlocked(false);
    assert_eq!(s.revision(), 1, "the lock is no new catalog");
}
