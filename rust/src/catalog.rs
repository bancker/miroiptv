use crate::news::{resolve_channels, NewsRule};
use crate::portal::xtream::{stream_ref, StreamKind};
use crate::portal::{Catalog as PortalCatalog, LiveChannel, Movie, Portal, Series};
use crate::search::{rank, ItemKind, SearchItem};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Canonical channel-name key for cross-variant matching. Live and
/// archive twins of the same channel often differ in superficial
/// formatting only: "NL | NPO 1" vs "NL | NPO1 HD". Normalizing both
/// sides to "npo1" makes the lookup robust to those variations.
///
/// Rules: strip the "NL | " (or similar pipe-separated country) prefix,
/// strip common quality suffixes (HD/UHD/4K/SD/FHD), drop all
/// whitespace, lowercase. Returns the canonical key.
pub fn normalize_channel_name(name: &str) -> String {
    // Drop country prefix - splits "NL | NPO 1" / "NL|NPO1" / "BE | Een" alike.
    let after_pipe = name.rsplit('|').next().unwrap_or(name).trim();
    // Strip trailing quality marker.
    let mut s = after_pipe;
    for suffix in [" UHD", " FHD", " HD", " 4K", " SD"] {
        s = s.trim_end_matches(suffix);
    }
    s.trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Adult content is what the portal flags with `is_adult`, and nothing else. A
/// name rule ("starts with XXX") was dropped on the user's decision of
/// 2026-10-12: it hid the ordinary film xXx (2002) and its sequels while
/// locked, and on this portal the flag is reliable.
fn adult_live(c: &LiveChannel) -> bool {
    c.is_adult
}

fn adult_movie(m: &Movie) -> bool {
    m.is_adult
}

fn adult_series(s: &Series) -> bool {
    s.is_adult
}

/// One column of the guide: a channel that has a catch-up (archive) variant,
/// all its variants merged. On this portal only the catch-up variants carry
/// EPG, so these are the channels the guide can fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideChannel {
    /// The name the column shows: the live variant's, else the catch-up one's.
    pub name: String,
    /// What plays live: the live variant zapping would pick, else `archive_id`.
    pub live_id: i64,
    /// Where the EPG and catch-up come from.
    pub archive_id: i64,
    /// How many days back catch-up reaches; 0 when the portal does not say.
    pub archive_days: u32,
    /// The logo URL, if the portal has one.
    pub logo: Option<String>,
    /// A variant of the channel carries the portal's adult flag.
    pub adult: bool,
}

/// The guide's channels in `live`: one per channel ([`normalize_channel_name`])
/// that has a catch-up variant (`tv_archive == 1`), in the order the channels
/// first appear. Of several live or catch-up variants the one zapping would
/// pick wins ([`zap_preference`]); on a tie, the first.
pub fn guide_channels_of(live: &[LiveChannel]) -> Vec<GuideChannel> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&LiveChannel>> = HashMap::new();
    for ch in live {
        let key = normalize_channel_name(&ch.name);
        if key.is_empty() {
            continue;
        }
        groups
            .entry(key)
            .or_insert_with_key(|k| {
                order.push(k.clone());
                Vec::new()
            })
            .push(ch);
    }
    order
        .iter()
        .filter_map(|key| {
            let group = groups.get(key)?;
            let best = |archive: bool| {
                group
                    .iter()
                    .copied()
                    .filter(|c| (c.tv_archive == 1) == archive)
                    .fold(None::<&LiveChannel>, |best, c| match best {
                        Some(b)
                            if zap_preference(quality_rank(&b.name))
                                >= zap_preference(quality_rank(&c.name)) =>
                        {
                            Some(b)
                        }
                        _ => Some(c),
                    })
            };
            let archive = best(true)?;
            let live_variant = best(false);
            let shown = live_variant.unwrap_or(archive);
            let logo = live_variant
                .into_iter()
                .chain(Some(archive))
                .filter_map(|c| c.stream_icon.as_deref())
                .map(str::trim)
                .find(|s| !s.is_empty())
                .map(str::to_owned);
            Some(GuideChannel {
                name: shown.name.clone(),
                live_id: shown.stream_id,
                archive_id: archive.stream_id,
                archive_days: u32::try_from(archive.tv_archive_duration).unwrap_or(0),
                logo,
                adult: group.iter().any(|c| adult_live(c)),
            })
        })
        .collect()
}

/// A loaded catalog plus what the app asks of it while it draws, worked out once
/// where the catalog is stored: a lookup by stream id is one probe of a set, not
/// a scan of ~13,000 channels. Reads as the catalog itself (`Deref`).
struct StoredCatalog {
    catalog: PortalCatalog,
    /// Stream ids of the adult live channels (see `adult_live`).
    adult_live_ids: HashSet<i64>,
    /// The guide's columns (see `guide_channels_of`), adult ones included.
    guide: Vec<GuideChannel>,
}

impl StoredCatalog {
    /// The one way a catalog gets stored: the fetch and `from_catalog` both come
    /// through here, so what is worked out cannot differ between them.
    fn new(mut catalog: PortalCatalog) -> Self {
        let adult_live_ids = catalog
            .live
            .iter()
            .filter(|c| adult_live(c))
            .map(|c| c.stream_id)
            .collect();
        let guide = guide_channels_of(&catalog.live);
        // Only GuideChannel::logo keeps logos: ~13,000 stored URLs cost memory and a clone each.
        for c in &mut catalog.live {
            c.stream_icon = None;
        }
        Self {
            catalog,
            adult_live_ids,
            guide,
        }
    }
}

impl std::ops::Deref for StoredCatalog {
    type Target = PortalCatalog;

    fn deref(&self) -> &PortalCatalog {
        &self.catalog
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_channel_name as n;

    #[test]
    fn live_and_archive_npo_match() {
        assert_eq!(n("NL | NPO 1"), n("NL | NPO1 HD"));
        assert_eq!(n("NL | NPO 3"), n("NL | NPO3 HD"));
        assert_eq!(n("NL | NPO 3"), n("NL | NPO 3 HD"));
    }

    #[test]
    fn live_and_archive_rtl_match() {
        assert_eq!(n("NL | RTL 4"), n("NL | RTL4 HD"));
        assert_eq!(n("NL | RTL Z"), n("NL | RTLZ HD"));
    }

    #[test]
    fn different_channels_dont_match() {
        assert_ne!(n("NL | NPO 1"), n("NL | NPO 2"));
        assert_ne!(n("NL | RTL 4"), n("NL | RTL 5"));
    }

    #[test]
    fn no_country_prefix_works() {
        assert_eq!(n("NPO 1"), n("NPO1 HD"));
    }
}

/// Quality tier of a channel name's marker; higher = better. A plain
/// unmarked feed ranks ABOVE an explicit SD one (unmarked is usually the
/// main HD-ish stream). Only used to pick between variants of the SAME
/// channel, so loose substring matching is safe here.
pub fn quality_rank(name: &str) -> u8 {
    let up = name.to_uppercase();
    if up.contains("UHD") || up.contains("4K") || up.contains("2160") || up.contains("ULTRA HD") {
        4
    } else if up.contains("FHD") || up.contains("1080") || up.contains("FULL HD") {
        3
    } else if up.contains("HD") || up.contains("720") {
        2
    } else if up.contains("SD") {
        0
    } else {
        1
    }
}

/// Short label for a quality tier rank (see [`quality_rank`]). The unmarked
/// tier (1) carries no token in the channel name, so it shows as "(?)".
pub fn quality_label(rank: u8) -> &'static str {
    match rank {
        4 => "UHD",
        3 => "FHD",
        2 => "HD",
        0 => "SD",
        _ => "(?)",
    }
}

/// Live (`tv_archive == 0`) variants of the channel `key`
/// ([`normalize_channel_name`]), reduced to ONE representative per distinct
/// quality tier ([`quality_rank`]) and sorted by tier ascending (SD..UHD).
/// Within a tier the lowest `stream_id` wins, for a deterministic order. Used
/// by the +/- quality switcher to step between a channel's qualities.
pub fn quality_ladder(channels: &[LiveChannel], key: &str) -> Vec<LiveChannel> {
    // Lowest-stream_id representative per quality tier, among this channel's
    // live variants. BTreeMap keys (the tier ranks) iterate ascending, so the
    // result is sorted SD..UHD without an extra sort.
    let mut by_tier: std::collections::BTreeMap<u8, LiveChannel> = std::collections::BTreeMap::new();
    for ch in channels {
        if ch.tv_archive != 0 || normalize_channel_name(&ch.name) != key {
            continue; // live variants of this channel only
        }
        let tier = quality_rank(&ch.name);
        match by_tier.get(&tier) {
            Some(cur) if cur.stream_id <= ch.stream_id => {} // keep the lower sid
            _ => {
                by_tier.insert(tier, ch.clone());
            }
        }
    }
    by_tier.into_values().collect()
}

/// Index of the currently-playing variant within `ladder`: matched first by
/// `cur_sid`, then (if that sid isn't the tier's representative) by matching
/// tier `cur_rank`. `None` if neither matches.
pub fn quality_pos(ladder: &[LiveChannel], cur_sid: i64, cur_rank: u8) -> Option<usize> {
    ladder
        .iter()
        .position(|c| c.stream_id == cur_sid)
        .or_else(|| ladder.iter().position(|c| quality_rank(&c.name) == cur_rank))
}

/// Snapshot for the +/- quality switcher: a channel's quality ladder (one entry
/// per tier, ascending) and the index currently playing.
#[derive(Debug, Default)]
pub struct QualityNav {
    pub ladder: Vec<LiveChannel>,
    pub pos: Option<usize>,
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    fn ch(stream_id: i64, name: &str, tv_archive: i32) -> LiveChannel {
        LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    #[test]
    fn ladder_one_per_tier_sorted_ascending() {
        let list = vec![
            ch(1, "NL | NPO 1 SD", 0),
            ch(2, "NL | NPO 1 HD", 0),
            ch(3, "NL | NPO 1 FHD", 0),
            ch(4, "NL | NPO 1 UHD", 0),
            ch(9, "NL | NPO 2 HD", 0),
        ];
        let l = quality_ladder(&list, "npo1");
        let ranks: Vec<u8> = l.iter().map(|c| quality_rank(&c.name)).collect();
        assert_eq!(ranks, vec![0, 2, 3, 4]);
    }

    #[test]
    fn ladder_excludes_archive_twins() {
        let list = vec![
            ch(1, "NL | NPO 1 HD", 0),
            ch(2, "NL | NPO 1 HD", 1),
            ch(3, "NL | NPO 1 UHD", 0),
        ];
        let l = quality_ladder(&list, "npo1");
        assert_eq!(l.len(), 2);
        assert!(l.iter().all(|c| c.tv_archive == 0));
    }

    #[test]
    fn ladder_collapses_same_tier_lowest_sid_wins() {
        let list = vec![
            ch(5, "NL | NPO 1 HD", 0),
            ch(2, "NL | NPO 1 HD", 0),
            ch(8, "NL | NPO 1 UHD", 0),
        ];
        let l = quality_ladder(&list, "npo1");
        assert_eq!(l.len(), 2);
        let hd = l.iter().find(|c| quality_rank(&c.name) == 2).unwrap();
        assert_eq!(hd.stream_id, 2);
    }

    #[test]
    fn ladder_single_variant_len_one() {
        let list = vec![ch(1, "NL | NPO 1 HD", 0), ch(2, "NL | NPO 2 HD", 0)];
        assert_eq!(quality_ladder(&list, "npo1").len(), 1);
    }

    #[test]
    fn ladder_excludes_other_channels() {
        let list = vec![ch(1, "RTL 4 HD", 0), ch(2, "RTL 5 UHD", 0)];
        let l = quality_ladder(&list, "rtl4");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].stream_id, 1);
    }

    #[test]
    fn pos_by_sid() {
        let ladder = vec![ch(2, "NPO 1 HD", 0), ch(8, "NPO 1 UHD", 0)];
        assert_eq!(quality_pos(&ladder, 8, quality_rank("NPO 1 UHD")), Some(1));
        assert_eq!(quality_pos(&ladder, 2, 2), Some(0));
    }

    #[test]
    fn pos_falls_back_to_rank_when_sid_absent() {
        let ladder = vec![ch(2, "NPO 1 HD", 0), ch(8, "NPO 1 UHD", 0)];
        assert_eq!(quality_pos(&ladder, 99, 4), Some(1));
    }

    #[test]
    fn pos_none_when_neither_matches() {
        let ladder = vec![ch(2, "NPO 1 HD", 0)];
        assert_eq!(quality_pos(&ladder, 99, 4), None);
    }

    #[test]
    fn labels_for_ranks() {
        assert_eq!(quality_label(4), "UHD");
        assert_eq!(quality_label(3), "FHD");
        assert_eq!(quality_label(2), "HD");
        assert_eq!(quality_label(0), "SD");
        assert_eq!(quality_label(1), "(?)");
    }
}

/// Score for choosing a channel's zap representative: a live entry always
/// beats its archive (catch-up) twin, then the preferred quality wins.
fn variant_score(ch: &LiveChannel) -> u16 {
    let live = if ch.tv_archive == 0 { 1u16 } else { 0 };
    live * 10 + zap_preference(quality_rank(&ch.name)) as u16
}

/// Zap preference of a quality tier: FHD first. This portal's "UHD" feeds are
/// the same 1080p picture at up to ~70% more bitrate, so they only add
/// bandwidth and freeze risk; UHD stays one `+` away on the quality ladder.
fn zap_preference(rank: u8) -> u8 {
    match rank {
        3 => 5,
        r => r,
    }
}

/// Where the channel of `variant` sits in a zap list ([`dedupe_zap`]): its own
/// entry, or the variant that stands for the channel there.
pub fn zap_position(zap: &[LiveChannel], variant: &LiveChannel) -> Option<usize> {
    zap.iter()
        .position(|c| c.stream_id == variant.stream_id)
        .or_else(|| {
            let key = normalize_channel_name(&variant.name);
            zap.iter()
                .position(|c| normalize_channel_name(&c.name) == key)
        })
}

/// What to play when the portal refuses variant `sid` of a channel: another
/// variant from its `ladder` ([`quality_ladder`]) in zap order (FHD first, see
/// [`zap_preference`]), skipping those `refused` says were turned down too.
pub fn fallback_variant(
    ladder: &[LiveChannel],
    sid: i64,
    refused: impl Fn(&LiveChannel) -> bool,
) -> Option<LiveChannel> {
    ladder
        .iter()
        .filter(|c| c.stream_id != sid && !refused(c))
        .max_by_key(|c| zap_preference(quality_rank(&c.name)))
        .cloned()
}

/// Collapse a live-channel list to ONE entry per distinct channel (keyed by
/// [`normalize_channel_name`]), keeping the highest-scoring variant so the
/// user doesn't zap through NPO1 HD / UHD / FHD / catch-up duplicates. Order
/// follows each channel's first appearance in the portal list.
pub fn dedupe_zap<'a>(channels: impl IntoIterator<Item = &'a LiveChannel>) -> Vec<LiveChannel> {
    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, LiveChannel> = HashMap::new();
    for ch in channels {
        let key = normalize_channel_name(&ch.name);
        if key.is_empty() {
            continue;
        }
        match best.get(&key) {
            None => {
                order.push(key.clone());
                best.insert(key, ch.clone());
            }
            Some(cur) if variant_score(ch) > variant_score(cur) => {
                best.insert(key, ch.clone());
            }
            Some(_) => {}
        }
    }
    order.into_iter().filter_map(|k| best.remove(&k)).collect()
}

#[cfg(test)]
mod zap_tests {
    use super::*;

    fn ch(stream_id: i64, name: &str, tv_archive: i32) -> LiveChannel {
        LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    #[test]
    fn zaps_to_fhd_over_uhd() {
        // This portal's "UHD" is the same 1080p picture at more bitrate.
        let list = vec![
            ch(1, "NL | NPO 1 HD", 0),
            ch(2, "NL | NPO 1 UHD", 0),
            ch(3, "NL | NPO 1 FHD", 0),
            ch(4, "NL | NPO 2 HD", 0),
        ];
        let z = dedupe_zap(&list);
        assert_eq!(z.len(), 2, "NPO 1 collapses to one, NPO 2 separate");
        let npo1 = z
            .iter()
            .find(|c| normalize_channel_name(&c.name) == "npo1")
            .unwrap();
        assert_eq!(npo1.stream_id, 3, "FHD variant wins");
    }

    fn rtl4_ladder() -> Vec<LiveChannel> {
        vec![
            ch(1, "NL | RTL 4 HD", 0),
            ch(2, "NL | RTL 4 FHD", 0),
            ch(3, "NL | RTL 4 UHD", 0),
        ]
    }

    #[test]
    fn a_refused_uhd_variant_falls_back_to_fhd() {
        let alt = fallback_variant(&rtl4_ladder(), 3, |_| false);
        assert_eq!(alt.map(|c| c.stream_id), Some(2));
    }

    #[test]
    fn fallback_skips_variants_that_were_refused_too() {
        let alt = fallback_variant(&rtl4_ladder(), 3, |c| c.stream_id == 2);
        assert_eq!(alt.map(|c| c.stream_id), Some(1));
    }

    #[test]
    fn zap_position_finds_the_channel_of_any_variant() {
        // Resumed on NPO 1 HD, while the zap list has NPO 1 FHD: arrow up
        // must go to the neighbour, not to the end of a 10,000-channel list.
        let zap = dedupe_zap(&[
            ch(1, "NL | NPO 1 HD", 0),
            ch(3, "NL | NPO 1 FHD", 0),
            ch(4, "NL | NPO 2 HD", 0),
        ]);
        assert_eq!(zap_position(&zap, &ch(1, "NL | NPO 1 HD", 0)), Some(0));
        assert_eq!(zap_position(&zap, &ch(4, "NL | NPO 2 HD", 0)), Some(1));
        assert_eq!(zap_position(&zap, &ch(9, "NL | RTL 4", 0)), None);
    }

    #[test]
    fn no_fallback_for_a_channel_without_other_variants() {
        let ladder = vec![ch(3, "NL | RTL 4 UHD", 0)];
        assert!(fallback_variant(&ladder, 3, |_| false).is_none());
    }

    #[test]
    fn uhd_still_beats_hd_without_an_fhd_variant() {
        let list = vec![ch(1, "NL | RTL 7 HD", 0), ch(2, "NL | RTL 7 UHD", 0)];
        let z = dedupe_zap(&list);
        assert_eq!(z.len(), 1);
        assert_eq!(z[0].stream_id, 2);
    }

    #[test]
    fn prefers_live_over_archive_twin() {
        let list = vec![ch(10, "NL | NPO 1 HD", 1), ch(11, "NL | NPO 1 HD", 0)];
        let z = dedupe_zap(&list);
        assert_eq!(z.len(), 1);
        assert_eq!(z[0].stream_id, 11);
        assert_eq!(z[0].tv_archive, 0);
    }

    #[test]
    fn preserves_first_appearance_order() {
        let list = vec![ch(1, "RTL 4", 0), ch(2, "NPO 1", 0), ch(3, "RTL 4 HD", 0)];
        let z = dedupe_zap(&list);
        assert_eq!(z.len(), 2);
        assert_eq!(normalize_channel_name(&z[0].name), "rtl4");
        assert_eq!(normalize_channel_name(&z[1].name), "npo1");
    }

    #[test]
    fn unmarked_beats_explicit_sd() {
        let list = vec![ch(1, "Foo SD", 0), ch(2, "Foo", 0)];
        let z = dedupe_zap(&list);
        assert_eq!(z.len(), 1);
        assert_eq!(z[0].stream_id, 2);
    }

    #[test]
    fn a_stored_catalog_knows_its_adult_channels_by_the_portals_flag_alone() {
        let stored = StoredCatalog::new(PortalCatalog {
            live: vec![
                ch(1, "NL | NPO 1", 0),
                LiveChannel {
                    is_adult: true,
                    ..ch(2, "24/7 | SHOPLYFTER 1", 0) // flagged
                },
                ch(3, "XXX | MEIDEN VAN HOLLAND", 0), // a name says nothing
                LiveChannel {
                    is_adult: true,
                    ..ch(4, "XXX | FLAGGED TOO", 0)
                },
            ],
            ..PortalCatalog::default()
        });
        let mut adult: Vec<i64> = stored.adult_live_ids.iter().copied().collect();
        adult.sort();
        assert_eq!(adult, [2, 4]);
        // The rest of the catalog reads through it.
        assert_eq!(stored.live.len(), 4);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStatus {
    Idle,
    /// Opening the connection to the portal.
    Connecting,
    /// Downloading the catalog.
    Fetching,
    Loaded,
    Failed(String),
}

pub struct CatalogStore {
    inner: Arc<RwLock<Option<StoredCatalog>>>,
    portal: Arc<dyn Portal>,
    status: Arc<RwLock<CatalogStatus>>,
    /// Adult content shows only after the PIN unlocked it; never persisted.
    adult_unlocked: AtomicBool,
    /// How many times `inner` was replaced (see [`CatalogStore::revision`]).
    revision: Arc<AtomicU64>,
}

/// Puts a catalog in its slot and counts it. The count moves after the catalog
/// is in, so whoever sees the new count finds the new catalog.
fn put_catalog(slot: &RwLock<Option<StoredCatalog>>, revision: &AtomicU64, stored: StoredCatalog) {
    *slot.write() = Some(stored);
    revision.fetch_add(1, Ordering::Release);
}

impl CatalogStore {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            portal,
            status: Arc::new(RwLock::new(CatalogStatus::Idle)),
            adult_unlocked: AtomicBool::new(false),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A store with a catalog already loaded (tests, tools).
    pub fn from_catalog(portal: Arc<dyn Portal>, catalog: PortalCatalog) -> Self {
        let store = Self::new(portal);
        put_catalog(&store.inner, &store.revision, StoredCatalog::new(catalog));
        *store.status.write() = CatalogStatus::Loaded;
        store
    }

    /// How many catalogs this store has held: 0 until the first loads, then one
    /// more with every load, re-fetch and `from_catalog`. F5 replaces the catalog
    /// in place and `is_loaded` stays true, so whatever the app works out from
    /// the catalog (the guide's columns) compares this per frame, an atomic
    /// load. Read it before the catalog itself: one that lands in between is
    /// then noticed next time, not missed.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn set_adult_unlocked(&self, on: bool) {
        self.adult_unlocked.store(on, Ordering::Relaxed);
    }

    pub fn adult_unlocked(&self) -> bool {
        self.adult_unlocked.load(Ordering::Relaxed)
    }

    /// May this live channel show now?
    fn shows_live(&self, c: &LiveChannel) -> bool {
        self.adult_unlocked() || !adult_live(c)
    }

    /// Whether a stream URL plays adult content, whatever the lock state.
    pub fn is_adult_url(&self, url: &str) -> bool {
        let Some((kind, id)) = stream_ref(url) else {
            return false;
        };
        let g = self.inner.read();
        let Some(c) = g.as_ref() else {
            return false;
        };
        match kind {
            StreamKind::Live | StreamKind::Timeshift => c.adult_live_ids.contains(&id),
            StreamKind::Movie => c.movies.iter().any(|m| m.stream_id == id && adult_movie(m)),
            // Series URLs carry episode ids, not series ids.
            StreamKind::Series => false,
        }
    }

    /// Whether a saved favorite or preset must stay hidden now: a channel the
    /// portal flags as adult, while locked. Until the catalog has loaded
    /// nothing is known, so nothing is hidden: whoever shows saved names waits
    /// for `is_loaded`.
    pub fn is_hidden_live(&self, sid: i64) -> bool {
        if self.adult_unlocked() {
            return false;
        }
        self.inner
            .read()
            .as_ref()
            .is_some_and(|c| c.adult_live_ids.contains(&sid))
    }

    /// Spawn an async fetch on the current tokio runtime. Non-blocking.
    /// Safe to call repeatedly: subsequent calls trigger a re-fetch.
    pub fn spawn_fetch(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let revision = self.revision.clone();
        let portal = self.portal.clone();
        let status = self.status.clone();
        *status.write() = CatalogStatus::Connecting;

        tokio::spawn(async move {
            let t0 = Instant::now();
            *status.write() = CatalogStatus::Fetching;
            info!("catalog: starting fetch");

            match portal.fetch_catalog().await {
                Ok(c) => {
                    let elapsed_ms = t0.elapsed().as_millis();
                    let archive_count = c.live.iter().filter(|x| x.tv_archive == 1).count();
                    info!(
                        "catalog: loaded in {} ms ({} live total, {} archive, {} movies, {} series)",
                        elapsed_ms,
                        c.live.len(),
                        archive_count,
                        c.movies.len(),
                        c.series.len()
                    );
                    // Dump the full archive list so the user can verify
                    // which stream_id maps to which channel name and
                    // test EPG URLs directly.
                    for ch in c.live.iter().filter(|x| x.tv_archive == 1) {
                        info!(
                            "  archive: sid={} name=\"{}\" (normalized=\"{}\")",
                            ch.stream_id,
                            ch.name,
                            normalize_channel_name(&ch.name)
                        );
                    }
                    // Worked out before the lock is taken: readers never wait for it.
                    let stored = StoredCatalog::new(c);
                    put_catalog(&inner, &revision, stored);
                    *status.write() = CatalogStatus::Loaded;
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    warn!("catalog: fetch failed after {} ms: {}", t0.elapsed().as_millis(), msg);
                    *status.write() = CatalogStatus::Failed(msg);
                }
            }
        });
    }

    pub fn status(&self) -> CatalogStatus {
        self.status.read().clone()
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.read().is_some()
    }

    pub fn live_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| {
                c.live
                    .iter()
                    .filter(|x| self.shows_live(x))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Deduplicated channel list for up/down zapping: one entry per distinct
    /// channel, highest-quality live variant. See [`dedupe_zap`].
    pub fn zap_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| dedupe_zap(c.live.iter().filter(|x| self.shows_live(x))))
            .unwrap_or_default()
    }

    /// The guide's columns ([`guide_channels_of`]), without the adult ones while
    /// locked. Worked out when the catalog was stored; this clones the ~200
    /// entries, so the app asks again only when the catalog or the lock changes.
    pub fn guide_channels(&self) -> Vec<GuideChannel> {
        let unlocked = self.adult_unlocked();
        self.inner
            .read()
            .as_ref()
            .map(|c| {
                c.guide
                    .iter()
                    .filter(|g| unlocked || !g.adult)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Quality ladder for the channel currently playing `sid`, plus the index
    /// within it — for the +/- quality switcher. One read-lock, no full-catalog
    /// clone (only this channel's handful of variants are cloned). Empty ladder
    /// when the catalog isn't loaded, `sid` is unknown or hidden (adult while
    /// locked); hidden variants of a shown channel are no rungs either.
    pub fn quality_nav(&self, sid: i64) -> QualityNav {
        let g = self.inner.read();
        let Some(c) = g.as_ref() else {
            return QualityNav::default();
        };
        let Some(cur) = c.live.iter().find(|x| x.stream_id == sid) else {
            return QualityNav::default();
        };
        if !self.shows_live(cur) {
            return QualityNav::default();
        }
        let key = normalize_channel_name(&cur.name);
        let mut ladder = quality_ladder(&c.live, &key);
        // Before `pos`: it indexes the ladder the user steps through.
        ladder.retain(|x| self.shows_live(x));
        let pos = quality_pos(&ladder, sid, quality_rank(&cur.name));
        QualityNav { ladder, pos }
    }

    /// Where each news rule points ([`crate::news::resolve_channels`]), among the
    /// channels that show now: while locked, a rule pointing at an adult channel
    /// finds nothing. One read lock and one pass, no copy of the catalog (see
    /// `quality_nav`); an unloaded catalog resolves every rule to nothing.
    pub fn resolve_news(&self, rules: &[NewsRule]) -> Vec<Option<(i64, bool, String)>> {
        let g = self.inner.read();
        match g.as_ref() {
            Some(c) => resolve_channels(c.live.iter().filter(|x| self.shows_live(x)), rules),
            None => vec![None; rules.len()],
        }
    }

    /// Archive (catch-up) channels: `tv_archive == 1`. These serve
    /// `/timeshift/...` and `get_simple_data_table` returns the full
    /// catch-up history; the matching live channel (`tv_archive == 0`)
    /// is needed for live URLs.
    pub fn archive_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| {
                c.live
                    .iter()
                    .filter(|x| x.tv_archive == 1 && self.shows_live(x))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find the live (non-archive) variant of a channel by exact name match.
    /// Used by the guide when the user picks an airing-now programme on an
    /// archive channel - we have to swap to the live id to actually play.
    pub fn live_id_by_name(&self, name: &str) -> Option<i64> {
        let target = normalize_channel_name(name);
        self.inner.read().as_ref().and_then(|c| {
            c.live
                .iter()
                .find(|x| {
                    x.tv_archive != 1
                        && normalize_channel_name(&x.name) == target
                        && self.shows_live(x)
                })
                .map(|x| x.stream_id)
        })
    }

    /// Reverse of live_id_by_name. Used by the zap toast's EPG fetch
    /// path: some Xtream portals (hnlol et al.) only populate EPG against
    /// the tv_archive=1 stream_ids - the live variants return empty even
    /// though they share a programme schedule with their archive twin.
    /// Matching is by normalized name so 'NL | NPO 1' (live) and
    /// 'NL | NPO1 HD' (archive) resolve to the same canonical key.
    pub fn archive_id_by_name(&self, name: &str) -> Option<i64> {
        let target = normalize_channel_name(name);
        self.inner.read().as_ref().and_then(|c| {
            c.live
                .iter()
                .find(|x| {
                    x.tv_archive == 1
                        && normalize_channel_name(&x.name) == target
                        && self.shows_live(x)
                })
                .map(|x| x.stream_id)
        })
    }

    pub fn search_items(&self) -> Vec<SearchItem> {
        let g = self.inner.read();
        let Some(c) = g.as_ref() else {
            return Vec::new();
        };
        let mut v = Vec::with_capacity(c.live.len() + c.movies.len() + c.series.len());
        for x in &c.live {
            if self.shows_live(x) {
                v.push(SearchItem {
                    id: x.stream_id,
                    name: x.name.clone(),
                    kind: ItemKind::Live,
                });
            }
        }
        for x in &c.movies {
            if self.adult_unlocked() || !adult_movie(x) {
                v.push(SearchItem {
                    id: x.stream_id,
                    name: x.name.clone(),
                    kind: ItemKind::Movie,
                });
            }
        }
        for x in &c.series {
            if self.adult_unlocked() || !adult_series(x) {
                v.push(SearchItem {
                    id: x.series_id,
                    name: x.name.clone(),
                    kind: ItemKind::Series,
                });
            }
        }
        v
    }

    pub fn search(&self, query: &str) -> Vec<SearchItem> {
        let items = self.search_items();
        rank(query, &items).into_iter().cloned().collect()
    }

    pub fn portal(&self) -> &Arc<dyn Portal> {
        &self.portal
    }

    /// After catalog loads, dump every tv_archive=1 channel with its
    /// stream_id at INFO so the user can verify which sid maps to which
    /// channel name and test EPG URLs directly. Cheap: runs once per
    /// catalog load (~once per session).
    pub fn log_archive_inventory(&self) {
        if let Some(c) = self.inner.read().as_ref() {
            let arch: Vec<_> = c.live.iter().filter(|x| x.tv_archive == 1).collect();
            tracing::info!("catalog: {} archive channels total", arch.len());
            for x in &arch {
                tracing::info!(
                    "  archive: sid={} name=\"{}\" (normalized=\"{}\")",
                    x.stream_id,
                    x.name,
                    normalize_channel_name(&x.name)
                );
            }
        }
    }

    /// Look up a movie's container extension by stream_id; default "mkv".
    pub fn movie_extension(&self, stream_id: i64) -> String {
        self.inner
            .read()
            .as_ref()
            .and_then(|c| c.movies.iter().find(|m| m.stream_id == stream_id))
            .and_then(|m| m.container_extension.clone())
            .unwrap_or_else(|| "mkv".to_owned())
    }
}

#[cfg(test)]
mod news_tests {
    use super::*;
    use crate::args::XtreamCreds;
    use crate::portal::xtream::XtreamPortal;

    /// A portal that is never asked anything: the catalog only needs one to exist.
    fn idle_portal() -> Arc<dyn Portal> {
        Arc::new(XtreamPortal::new(XtreamCreds {
            username: "u".into(),
            password: "p".into(),
            host: "h".into(),
            port: 80,
        }))
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

    fn rule(channel: &str) -> NewsRule {
        NewsRule {
            channel: channel.into(),
            patterns: "*".into(),
            label: String::new(),
            skip_secs: 0,
        }
    }

    #[test]
    fn a_news_rule_finds_only_the_channels_that_show() {
        let store = CatalogStore::from_catalog(
            idle_portal(),
            PortalCatalog {
                live: vec![
                    ch(1, "NL | NPO 1", false),
                    ch(2, "24/7 | NIGHT NEWS", true), // adult by the portal's flag
                    ch(3, "XXX | AFTER DARK", true),  // adult by the flag, whatever the name
                ],
                ..PortalCatalog::default()
            },
        );
        let rules = [rule("NPO 1"), rule("NIGHT NEWS"), rule("AFTER DARK")];
        let npo = Some((1, false, "NL | NPO 1".to_string()));
        // Locked: a rule pointing at an adult channel finds nothing.
        assert_eq!(store.resolve_news(&rules), [npo.clone(), None, None]);
        store.set_adult_unlocked(true);
        assert_eq!(
            store.resolve_news(&rules),
            [
                npo.clone(),
                Some((2, false, "24/7 | NIGHT NEWS".to_string())),
                Some((3, false, "XXX | AFTER DARK".to_string())),
            ]
        );
        store.set_adult_unlocked(false);
        assert_eq!(store.resolve_news(&rules), [npo, None, None]);
    }

    #[test]
    fn an_unloaded_catalog_resolves_every_rule_to_nothing() {
        let store = CatalogStore::new(idle_portal());
        assert_eq!(
            store.resolve_news(&[rule("NPO 1"), rule("CNN")]),
            [None, None]
        );
        assert!(store.resolve_news(&[]).is_empty());
    }
}
