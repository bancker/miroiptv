use crate::channel_list::ChannelGroups;
use crate::news::{resolve_channels, NewsRule};
use crate::portal::xtream::{stream_ref, StreamKind};
use crate::portal::{
    Catalog as PortalCatalog, Categories, LiveChannel, Movie, Portal, PortalError, Series,
};
use crate::search::{rank, ItemKind, SearchItem};
use crate::sources::SourceKind;
use crate::vod::index::{CategoryInfo, Extras, Learned, Query, Results, Title, TitleIndex};
use crate::vod::TitleId;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
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

/// Does this live channel show? Unlocked, all do; locked, the adult ones do
/// not. The one place that says so: `shows_live`, the zap ring and the channel
/// list's groups all ask it, so the lists cannot differ about the lock.
fn live_shows(unlocked: bool, c: &LiveChannel) -> bool {
    unlocked || !adult_live(c)
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
    // A column needs a catch-up variant, so without one there is none. An M3U
    // list never has one: grouping its ~11,000 names to find that out took ~6 ms.
    if !live.iter().any(|c| c.tv_archive == 1) {
        return Vec::new();
    }
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
/// a scan of ~13,000 channels. Reads as the catalog (`Deref`), except `live`,
/// which has a field of its own.
struct StoredCatalog {
    /// The films and series; its `live` is empty (see `live`). Shared with the
    /// build of their titles, which reads them once the catalog is in
    /// (`TitleInput`).
    catalog: Arc<PortalCatalog>,
    /// The live channels, taken out of `catalog`: what is worked out from them
    /// (the zap ring, the channel list's groups) holds this `Arc` and positions
    /// in it, never copies of the channels. `c.live` reads this field, not the
    /// catalog's empty one.
    live: Arc<Vec<LiveChannel>>,
    /// Stream ids of the adult live channels (see `adult_live`).
    adult_live_ids: HashSet<i64>,
    /// The guide's columns (see `guide_channels_of`), adult ones included.
    guide: Vec<GuideChannel>,
    /// The zap ring last asked for, with the lock and the marks it was made for.
    zap: Mutex<Option<Arc<ZapRing>>>,
}

impl StoredCatalog {
    /// The one way a catalog gets stored: the fetch and `from_catalog` both come
    /// through here, so what is worked out cannot differ between them. What only
    /// the title index reads leaves the lists here, and comes back as the
    /// [`TitleInput`] of their titles, which are built once the catalog is in.
    fn new(mut catalog: PortalCatalog) -> (Self, TitleInput) {
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
        let live = Arc::new(std::mem::take(&mut catalog.live));
        // The covers, a series' genre and release date are the index's alone
        // (Ruling 22): they move out of the stored lists, to its build.
        let extras = Extras::take(&mut catalog.movies, &mut catalog.series);
        let catalog = Arc::new(catalog);
        let titles = TitleInput {
            lists: catalog.clone(),
            extras,
        };
        let stored = Self {
            catalog,
            live,
            adult_live_ids,
            guide,
            zap: Mutex::new(None),
        };
        (stored, titles)
    }

    /// The zap ring for this lock and these marks: the one made last when it was
    /// made for them, else a new one, kept for the next press.
    fn zap_ring(
        &self,
        kind: SourceKind,
        unlocked: bool,
        defect: &HashMap<i64, i64>,
        defect_revision: u64,
    ) -> Arc<ZapRing> {
        let mut slot = self.zap.lock();
        if let Some(ring) = slot
            .as_ref()
            .filter(|r| r.unlocked == unlocked && r.defect_revision == defect_revision)
        {
            return ring.clone();
        }
        let ring = Arc::new(ZapRing::build(
            &self.live,
            kind,
            unlocked,
            defect,
            defect_revision,
        ));
        *slot = Some(ring.clone());
        ring
    }
}

impl std::ops::Deref for StoredCatalog {
    type Target = PortalCatalog;

    fn deref(&self) -> &PortalCatalog {
        &self.catalog
    }
}

/// What the titles of a stored catalog are built from ([`TitleInput::build`]):
/// its lists, shared with it through their `Arc`, and what only the index
/// reads, which the lists gave up before they were stored, so that no address
/// is held twice once the index has it (Ruling 22).
struct TitleInput {
    lists: Arc<PortalCatalog>,
    extras: Extras,
}

impl TitleInput {
    /// The title index of these lists, with the portal's `categories`, and its
    /// `vod:` line: on the blocking pool in the fetch, once the catalog is put
    /// and its categories are in (up to ~150 ms for a big portal); in place in
    /// `from_catalog`. It reads the stored lists and copies none of them.
    fn build(self, categories: &Categories) -> TitleIndex {
        let t0 = Instant::now();
        let titles = TitleIndex::build_with(
            &self.lists.movies,
            &self.lists.series,
            &self.extras,
            categories,
            latest_year(),
        );
        if !titles.is_empty() {
            log_titles(&titles, categories, t0.elapsed());
        }
        titles
    }
}

/// The latest year a title's name may give: the year after this one ("Blade
/// Runner 2049" is no film of 2049).
fn latest_year() -> u16 {
    use chrono::Datelike;
    (chrono::Utc::now().year() + 1).clamp(1900, u16::MAX as i32) as u16
}

/// What the lists gave the title index, in one line (spec 2026-10-15 §11: the
/// quality of the portal's data shows only with real titles).
fn log_titles(titles: &TitleIndex, categories: &Categories, took: std::time::Duration) {
    let s = titles.stats();
    let n = titles.len().max(1);
    let pct = |part: usize| part * 100 / n;
    info!(
        "vod: {} titles ({} series, {} films in {} versions, at most {} in one film, {} left out), {} categories - cover {} %, rating {} %, year {} %, genre {} %, Dutch {} %, English {} % - index {} KB, covers {} KB, built in {} ms",
        titles.len(),
        s.series,
        s.films,
        s.versions,
        s.most_versions,
        s.skipped,
        categories.vod.len() + categories.series.len(),
        pct(s.with_cover),
        pct(s.with_rating),
        pct(s.with_year),
        pct(s.with_genre),
        pct(s.dutch),
        pct(s.english),
        titles.heap_bytes() / 1024,
        titles.cover_bytes() / 1024,
        took.as_millis()
    );
}

/// The thuisbioscoop's titles as the adult lock lets them show (spec
/// 2026-10-15 §4.2). Every answer leaves the adult titles out while locked:
/// this is the only way out of the catalog for the index, so nothing else
/// filters. Asked again after the lock changes.
///
/// Until the titles of the store's catalog are built, they are those of the
/// cache file (`saved_at`).
#[derive(Clone)]
pub struct Titles {
    index: Arc<TitleIndex>,
    unlocked: bool,
    /// When the cache file the titles come from was written, unix seconds;
    /// none for a catalog's own.
    saved_at: Option<i64>,
}

impl Titles {
    /// The titles that answer `query`, with the counts of its filters.
    pub fn search(&self, query: &Query) -> Results {
        self.index.search(query, self.unlocked)
    }

    /// The title at `pos`; none for an adult one while locked.
    pub fn title(&self, pos: usize) -> Option<Title<'_>> {
        self.index.title(pos).filter(|t| self.unlocked || !t.adult)
    }

    /// Where title `id` is, when it shows.
    pub fn find(&self, id: TitleId) -> Option<usize> {
        self.index.find(id).filter(|&pos| self.title(pos).is_some())
    }

    /// The film title a version plays, when it shows.
    pub fn title_of_version(&self, stream_id: i64) -> Option<usize> {
        self.index
            .title_of_version(stream_id)
            .filter(|&pos| self.title(pos).is_some())
    }

    /// Titles like the one at `pos` ([`TitleIndex::more_like`]).
    pub fn more_like(
        &self,
        pos: usize,
        limit: usize,
        watched: &HashSet<TitleId>,
        disliked: &HashSet<TitleId>,
    ) -> Vec<u32> {
        if self.title(pos).is_none() {
            return Vec::new();
        }
        self.index
            .more_like(pos, limit, self.unlocked, watched, disliked)
    }

    /// What title `pos`'s details tell the index ([`TitleIndex::learn`]).
    pub fn learn(&self, pos: usize, learned: Learned) {
        self.index.learn(pos, learned);
    }

    /// The portal's categories.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.index.categories()
    }

    /// When the cache file these titles come from was written (unix seconds):
    /// Thuis says "Gegevens van <dag> <tijd>". None for a catalog's own.
    pub fn saved_at(&self) -> Option<i64> {
        self.saved_at
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

/// The positions (as `channels` numbers them) of the entries [`dedupe_zap`]
/// keeps: ONE per distinct channel ([`normalize_channel_name`]), the
/// highest-scoring variant, in the order of each channel's first appearance.
fn dedupe<'a>(channels: impl IntoIterator<Item = (usize, &'a LiveChannel)>) -> Vec<usize> {
    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, (usize, &LiveChannel)> = HashMap::new();
    for (i, ch) in channels {
        let key = normalize_channel_name(&ch.name);
        if key.is_empty() {
            continue;
        }
        match best.get(&key) {
            None => {
                order.push(key.clone());
                best.insert(key, (i, ch));
            }
            Some((_, cur)) if variant_score(ch) > variant_score(cur) => {
                best.insert(key, (i, ch));
            }
            Some(_) => {}
        }
    }
    order
        .into_iter()
        .filter_map(|k| best.remove(&k))
        .map(|(i, _)| i)
        .collect()
}

/// Collapse a live-channel list to ONE entry per distinct channel (keyed by
/// [`normalize_channel_name`]), keeping the highest-scoring variant so the
/// user doesn't zap through NPO1 HD / UHD / FHD / catch-up duplicates. Order
/// follows each channel's first appearance in the portal list.
pub fn dedupe_zap<'a>(channels: impl IntoIterator<Item = &'a LiveChannel>) -> Vec<LiveChannel> {
    let channels: Vec<&LiveChannel> = channels.into_iter().collect();
    dedupe(channels.iter().copied().enumerate())
        .into_iter()
        .map(|i| channels[i].clone())
        .collect()
}

/// What up/down zapping steps through: one entry per channel the user zaps to,
/// in zap order. An Xtream portal's variants of a channel are one entry, its
/// best ([`dedupe_zap`]); an M3U list's entries are all there, in the file's
/// order. Adult channels while locked and channels marked defect are out.
/// Worked out once per catalog, lock and marks ([`CatalogStore::zap_ring`]), it
/// holds the catalog's channels and positions in them, never copies: a press
/// copies nothing.
pub struct ZapRing {
    live: Arc<Vec<LiveChannel>>,
    /// Indexes into `live`, in zap order.
    order: Vec<u32>,
    /// An M3U list's ring: `order` keeps the file's order and ascends.
    file_order: bool,
    /// The lock state and the marks revision it was made for.
    unlocked: bool,
    defect_revision: u64,
}

/// Where a zap step starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZapFrom {
    /// Nothing plays: the step counts from the ring's first entry.
    Start,
    /// Ring position `pos`, which held channel `sid` when it was taken. When it
    /// holds another one now (the ring changed), as `Channel(sid)`.
    Pos { pos: usize, sid: i64 },
    /// Channel `sid`, wherever it is.
    Channel(i64),
}

/// Where a channel sits in the ring.
enum Place {
    /// Its entry, or the one that stands for it.
    At(usize),
    /// Out of the ring (an M3U channel marked defect): just before this ring
    /// position, which may be the ring's length.
    Before(usize),
    /// Not in the catalog, or hidden.
    Unknown,
}

impl ZapRing {
    fn build(
        live: &Arc<Vec<LiveChannel>>,
        kind: SourceKind,
        unlocked: bool,
        defect: &HashMap<i64, i64>,
        defect_revision: u64,
    ) -> Self {
        let shows = |c: &LiveChannel| live_shows(unlocked, c) && !defect.contains_key(&c.stream_id);
        let entries = live.iter().enumerate().filter(|(_, c)| shows(c));
        let order: Vec<usize> = match kind {
            SourceKind::Xtream => dedupe(entries),
            // Every entry, in the file's order: merging variants is made for the
            // Xtream portal's names.
            SourceKind::M3u => entries.map(|(i, _)| i).collect(),
        };
        Self {
            live: live.clone(),
            order: order.into_iter().map(|i| i as u32).collect(),
            file_order: kind == SourceKind::M3u,
            unlocked,
            defect_revision,
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The channel at ring position `pos`.
    pub fn get(&self, pos: usize) -> Option<&LiveChannel> {
        self.order.get(pos).and_then(|&i| self.live.get(i as usize))
    }

    /// The ring's channels, in zap order.
    pub fn channels(&self) -> impl Iterator<Item = &LiveChannel> + '_ {
        self.order.iter().filter_map(|&i| self.live.get(i as usize))
    }

    /// Where channel `sid` sits: its own entry, else (on an Xtream portal) the
    /// entry that stands for its channel. None for a channel out of the ring.
    pub fn position_of(&self, sid: i64) -> Option<usize> {
        match self.place_of(sid) {
            Place::At(pos) => Some(pos),
            Place::Before(_) | Place::Unknown => None,
        }
    }

    /// The ring position `delta` entries on from `from`, round the ring; None
    /// for an empty ring. A channel out of the ring (an M3U channel marked
    /// defect) steps from its place in the file: on to the first entry after
    /// it, back to the last one before it. With nothing known the step counts
    /// from the start, as zapping always did ([`crate::shortcuts::next_live_idx`]).
    pub fn step(&self, from: ZapFrom, delta: i32) -> Option<usize> {
        let len = self.order.len();
        if len == 0 {
            return None;
        }
        let place = match from {
            ZapFrom::Start => Place::Unknown,
            ZapFrom::Pos { pos, sid } if self.get(pos).is_some_and(|c| c.stream_id == sid) => {
                Place::At(pos)
            }
            ZapFrom::Pos { sid, .. } | ZapFrom::Channel(sid) => self.place_of(sid),
        };
        match place {
            Place::At(pos) => crate::shortcuts::next_live_idx(Some(pos), len, delta),
            Place::Before(pos) => {
                // One step on lands on `pos`, one back on the entry before it.
                let to = pos as i64 + i64::from(delta) - i64::from(delta > 0);
                Some(to.rem_euclid(len as i64) as usize)
            }
            Place::Unknown => crate::shortcuts::next_live_idx(None, len, delta),
        }
    }

    fn place_of(&self, sid: i64) -> Place {
        let Some(file) = self.live.iter().position(|c| c.stream_id == sid) else {
            return Place::Unknown;
        };
        let own = if self.file_order {
            self.order.binary_search(&(file as u32)).ok()
        } else {
            self.order.iter().position(|&i| i as usize == file)
        };
        if let Some(pos) = own {
            return Place::At(pos);
        }
        let channel = &self.live[file];
        if !live_shows(self.unlocked, channel) {
            return Place::Unknown;
        }
        if self.file_order {
            return Place::Before(self.order.partition_point(|&i| (i as usize) < file));
        }
        let key = normalize_channel_name(&channel.name);
        self.order
            .iter()
            .position(|&i| normalize_channel_name(&self.live[i as usize].name) == key)
            .map_or(Place::Unknown, Place::At)
    }
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
        let (stored, _) = StoredCatalog::new(PortalCatalog {
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
        // Its live channels are kept apart, in `live`.
        assert_eq!(stored.live.len(), 4);
    }

    #[tokio::test]
    async fn a_store_whose_titles_came_first_takes_none_from_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod-h_80.json");
        let cached = TitleIndex::build(
            &[],
            &[Series {
                series_id: 1,
                name: "Cached".into(),
                ..Series::default()
            }],
            &Categories::default(),
            2027,
        );
        crate::vod::cache::save(&path, &cached, 1).unwrap();
        let portal = Arc::new(crate::portal::xtream::XtreamPortal::new(
            crate::args::XtreamCreds {
                username: "u".into(),
                password: "p".into(),
                host: "h".into(),
                port: 80,
            },
        ));
        let store =
            CatalogStore::from_catalog(portal, PortalCatalog::default()).with_vod_cache(Some(path));
        store.read_vod_cache();
        // The read holds the slot until it ends: then the store alone does.
        for _ in 0..400 {
            if Arc::strong_count(&store.titles) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(Arc::strong_count(&store.titles), 1, "the read has ended");
        let held = store.titles.read();
        let (index, saved_at) = held.as_ref().expect("the catalog's own titles");
        assert!(
            index.is_empty() && saved_at.is_none(),
            "no second index held"
        );
        assert_eq!(store.titles_revision(), 1, "the file put nothing");
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

/// What the app should tell the user after a fetch of an M3U list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogNotice {
    /// The fresh list did not come; the saved one stays.
    NotUpdated,
    /// What came is no channel list; what showed stays.
    NotAList,
}

pub struct CatalogStore {
    inner: Arc<RwLock<Option<StoredCatalog>>>,
    portal: Arc<dyn Portal>,
    status: Arc<RwLock<CatalogStatus>>,
    /// Adult content shows only after the PIN unlocked it; never persisted.
    adult_unlocked: AtomicBool,
    /// How many times `inner` was replaced (see [`CatalogStore::revision`]).
    revision: Arc<AtomicU64>,
    /// What the last fetch has to tell the user, until the app takes it.
    notice: Arc<Mutex<Option<CatalogNotice>>>,
    /// The M3U channels marked defect: stream id → when it failed, unix seconds
    /// (spec 2026-10-15 §4). The zap ring and the channel list's groups leave
    /// them out; the search and the favorites still name them. The app sets them
    /// (`set_defect`); an Xtream catalog has none.
    defect: RwLock<Arc<HashMap<i64, i64>>>,
    /// Counts every `set_defect`: what is worked out from the marks follows it.
    defect_revision: AtomicU64,
    /// The thuisbioscoop's titles (`vod::index`), adult ones included: they
    /// leave the store only as `Titles`, which apply the lock. Put after the
    /// catalog they come from (`put_titles`); until then those of the cache
    /// file, once read (`read_vod_cache`).
    titles: Arc<RwLock<Option<StoredTitles>>>,
    /// How many times `titles` was put (see [`CatalogStore::titles_revision`]).
    titles_revision: Arc<AtomicU64>,
    /// How long the titles wait for the category lists once the catalog is in
    /// ([`CatalogStore::with_categories_wait`]).
    categories_wait: Duration,
    /// Where this portal's titles are kept between starts (`vod::cache`);
    /// none for an M3U list, and in tests that do not ask for one.
    vod_cache: Option<PathBuf>,
    /// The file was asked for: it is read once per store.
    vod_cache_asked: AtomicBool,
    /// The store's source is no longer in use ([`CatalogStore::retire`]): its
    /// titles are not written to the file any more. Every write holds it from
    /// that check until the file is in place ([`keep_titles`]), which keeps the
    /// store's writes one at a time as well.
    vod_cache_retired: Arc<Mutex<bool>>,
}

/// Titles in the store, and when the cache file they come from was written
/// (unix seconds; none for a catalog's own).
type StoredTitles = (Arc<TitleIndex>, Option<i64>);

/// Writes `titles` to the cache file at `path`, on the blocking pool
/// ([`keep_titles`]).
fn write_vod_cache(path: PathBuf, titles: Arc<TitleIndex>, retired: Arc<Mutex<bool>>) {
    tokio::task::spawn_blocking(move || keep_titles(&path, &titles, &retired));
}

/// Writes `titles` to the cache file at `path`, unless the source of their
/// store is no longer in use (`retired`, [`CatalogStore::retire`]). The flag is
/// held until the file is in place, as an M3U list's own is while it saves its
/// list: a retire waits for a write that has begun, and no write begins after
/// it. So the store's writes go one at a time, and those of the process too
/// (`cache::save`). A write that fails leaves the file that was there whole and
/// is logged by the file's name, never its folder (which holds the user's
/// name); the next build writes again.
fn keep_titles(path: &Path, titles: &TitleIndex, retired: &Mutex<bool>) {
    let retired = retired.lock();
    if *retired {
        info!("vod: cache not written - the source is no longer in use");
        return;
    }
    let now = chrono::Utc::now().timestamp();
    match crate::vod::cache::save(path, titles, now) {
        Ok((n, bytes)) => info!("vod: cache written ({} titles, {} KB)", n, bytes / 1024),
        Err(e) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            warn!("vod: cache not written ({name}: {e})");
        }
    }
}

/// The titles of the cache file at `path`, put in `slot` while it holds none
/// and counted in `titles_revision`; on the blocking pool
/// ([`CatalogStore::read_vod_cache`]). A file that cannot be read is ignored,
/// and its error logged masked and cut ([`logged_error`]).
fn read_titles(path: &Path, slot: &RwLock<Option<StoredTitles>>, titles_revision: &AtomicU64) {
    let t0 = Instant::now();
    match crate::vod::cache::load(path) {
        Ok(Some((index, saved_at))) => {
            info!(
                "vod: cache read ({} titles, written at {}) in {} ms",
                index.len(),
                saved_at,
                t0.elapsed().as_millis()
            );
            // Titles that came first (the catalog's own) are fresher.
            let mut held = slot.write();
            if held.is_none() {
                *held = Some((Arc::new(index), Some(saved_at)));
                titles_revision.fetch_add(1, Ordering::Release);
            }
        }
        Ok(None) => {}
        Err(e) => warn!(
            "vod: cache unreadable ({}) - ignored, the next load writes a new one",
            logged_error(&e)
        ),
    }
}

/// How many characters of a file's error its log line keeps.
const LOGGED_ERROR_CHARS: usize = 300;

/// A file's error as its log line says it: masked (`redact`), then cut to
/// `LOGGED_ERROR_CHARS` characters at a character's boundary. serde quotes the
/// value it could not read, and a file may hold anything: a name of 70,000
/// characters would make a line of 70,000, and an account cut in two before it
/// is masked would be one that `redact` no longer knows.
fn logged_error(e: &str) -> String {
    let e = crate::redact::redact(e);
    match e.char_indices().nth(LOGGED_ERROR_CHARS) {
        Some((cut, _)) => format!("{}...", &e[..cut]),
        None => e.into_owned(),
    }
}

/// Puts a catalog in its slot and counts it. The count moves after the catalog
/// is in, so whoever sees the new count finds the new catalog, and before the
/// slot's write lock is let go: two fetches that put at once cannot leave one's
/// catalog in the slot under the other's count. The count it put the catalog
/// at: its titles are put for that one ([`put_titles`]).
fn put_catalog(
    slot: &RwLock<Option<StoredCatalog>>,
    revision: &AtomicU64,
    stored: StoredCatalog,
) -> u64 {
    let mut held = slot.write();
    *held = Some(stored);
    revision.fetch_add(1, Ordering::Release) + 1
}

/// Puts the titles built from the catalog put at count `of` in their slot and
/// counts them, as `put_catalog` does, when that catalog is still the store's
/// (`revision`): a newer one's own titles come after it. They take the place
/// of the cache file's. Whether they were put.
fn put_titles(
    slot: &RwLock<Option<StoredTitles>>,
    titles_revision: &AtomicU64,
    revision: &AtomicU64,
    of: u64,
    titles: Arc<TitleIndex>,
) -> bool {
    let mut held = slot.write();
    if revision.load(Ordering::Acquire) != of {
        return false;
    }
    *held = Some((titles, None));
    titles_revision.fetch_add(1, Ordering::Release);
    true
}

/// How long the titles wait for the portal's category lists once the catalog
/// is in: then they are built without them (spec 2026-10-15 §5: the channels
/// wait for none of it, and the titles not for long).
const CATEGORIES_WAIT: Duration = Duration::from_secs(15);

/// The category lists the fetch asked beside the three lists, once its catalog
/// is in: what they brought; none when they fail or have not come within
/// `wait`, and then their request stops. The warn line names no address
/// (`api_json`).
async fn categories_after(
    mut asked: tokio::task::JoinHandle<Result<Categories, PortalError>>,
    wait: Duration,
) -> Categories {
    let why = match tokio::time::timeout(wait, &mut asked).await {
        Ok(Ok(Ok(categories))) => return categories,
        Ok(Ok(Err(e))) => e.to_string(),
        Ok(Err(e)) => e.to_string(),
        Err(_) => {
            asked.abort();
            format!("none within {wait:?}")
        }
    };
    warn!("catalog: no categories ({why}) - the titles go without");
    Categories::default()
}

impl CatalogStore {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            portal,
            status: Arc::new(RwLock::new(CatalogStatus::Idle)),
            adult_unlocked: AtomicBool::new(false),
            revision: Arc::new(AtomicU64::new(0)),
            notice: Arc::new(Mutex::new(None)),
            defect: RwLock::new(Arc::new(HashMap::new())),
            defect_revision: AtomicU64::new(0),
            titles: Arc::new(RwLock::new(None)),
            titles_revision: Arc::new(AtomicU64::new(0)),
            categories_wait: CATEGORIES_WAIT,
            vod_cache: None,
            vod_cache_asked: AtomicBool::new(false),
            vod_cache_retired: Arc::new(Mutex::new(false)),
        }
    }

    /// How long this store's titles wait for the portal's category lists once
    /// its catalog is in: 15 s, a test less. Then they are built without them.
    pub fn with_categories_wait(mut self, wait: Duration) -> Self {
        self.categories_wait = wait;
        self
    }

    /// This store keeps its titles in the file at `path` (`vod::cache`): it
    /// writes it after every build of its titles and reads it once, when asked
    /// ([`CatalogStore::read_vod_cache`]). None keeps no file.
    pub fn with_vod_cache(mut self, path: Option<PathBuf>) -> Self {
        self.vod_cache = path;
        self
    }

    /// Reads the titles of the last start from the file, once, on the blocking
    /// pool: the app asks after its first frame (spec 2026-10-15 §4.6). They are
    /// put only while the store has no titles, and the titles of its catalog
    /// take their place. A file that cannot be read is ignored; the next load
    /// writes a new one.
    pub fn read_vod_cache(&self) {
        // Asked every frame: once asked, an atomic load and nothing else.
        if self.vod_cache_asked.load(Ordering::Acquire) {
            return;
        }
        let Some(path) = self.vod_cache.clone() else {
            self.vod_cache_asked.store(true, Ordering::Release);
            return;
        };
        if self.vod_cache_asked.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (titles, titles_revision) = (self.titles.clone(), self.titles_revision.clone());
        runtime.spawn_blocking(move || read_titles(&path, &titles, &titles_revision));
    }

    /// The app no longer runs on this store: its source was switched away
    /// from, saved with another address or deleted. Its portal writes nothing
    /// for it any more ([`Portal::retire`]), and neither does the store: a write
    /// of the titles' file that has begun ends before this returns, and none
    /// begins after it. A fetch still under way may still put its titles here,
    /// in a store that no longer shows.
    pub fn retire(&self) {
        self.portal.retire();
        *self.vod_cache_retired.lock() = true;
    }

    /// A store with a catalog already loaded (tests, tools), and its titles,
    /// built in place. It has no categories: its films take no genre from them.
    pub fn from_catalog(portal: Arc<dyn Portal>, catalog: PortalCatalog) -> Self {
        let store = Self::new(portal);
        let (stored, input) = StoredCatalog::new(catalog);
        let of = put_catalog(&store.inner, &store.revision, stored);
        let titles = Arc::new(input.build(&Categories::default()));
        put_titles(
            &store.titles,
            &store.titles_revision,
            &store.revision,
            of,
            titles,
        );
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

    /// How many times this store's titles were put: 0 until the first
    /// catalog's are built, then one more with every build. Whatever is worked
    /// out from the titles compares this per frame, an atomic load, as the
    /// guide does [`CatalogStore::revision`]. Read it before the titles: titles
    /// that land in between are then noticed next time, not missed. It does not
    /// move when the lock changes, so whatever is worked out from the titles
    /// also compares [`CatalogStore::adult_unlocked`].
    pub fn titles_revision(&self) -> u64 {
        self.titles_revision.load(Ordering::Acquire)
    }

    /// What kind of source this catalog is of.
    pub fn kind(&self) -> SourceKind {
        self.portal.kind()
    }

    /// The last fetch's notice, once: the app shows it as a toast.
    pub fn take_notice(&self) -> Option<CatalogNotice> {
        self.notice.lock().take()
    }

    pub fn set_adult_unlocked(&self, on: bool) {
        self.adult_unlocked.store(on, Ordering::Relaxed);
    }

    pub fn adult_unlocked(&self) -> bool {
        self.adult_unlocked.load(Ordering::Relaxed)
    }

    /// May this live channel show now?
    fn shows_live(&self, c: &LiveChannel) -> bool {
        live_shows(self.adult_unlocked(), c)
    }

    /// Whether a stream URL plays adult content, whatever the lock state.
    pub fn is_adult_url(&self, url: &str) -> bool {
        // An M3U list has no adult content: only Xtream portals flag it.
        if self.kind() == SourceKind::M3u {
            return false;
        }
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
    ///
    /// An M3U source with nothing showing yet shows its saved list first
    /// ([`Portal::saved_catalog`]); a fresh list that did not come, or is no
    /// list, leaves what shows in place and a [`CatalogNotice`] to take.
    pub fn spawn_fetch(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let revision = self.revision.clone();
        let portal = self.portal.clone();
        let status = self.status.clone();
        let notice = self.notice.clone();
        let titles = self.titles.clone();
        let titles_revision = self.titles_revision.clone();
        let vod_cache = self.vod_cache.clone();
        let vod_cache_retired = self.vod_cache_retired.clone();
        let categories_wait = self.categories_wait;
        *status.write() = CatalogStatus::Connecting;

        tokio::spawn(async move {
            let t0 = Instant::now();
            // An M3U source shows its saved list at once, while the fresh one
            // comes (a refetch has one showing already).
            let mut showing_saved = false;
            if inner.read().is_none() {
                if let Some(saved) = portal.saved_catalog().await {
                    // An M3U list has no films or series: its titles come with
                    // the fresh list's.
                    let (stored, _) = StoredCatalog::new(saved);
                    put_catalog(&inner, &revision, stored);
                    *status.write() = CatalogStatus::Loaded;
                    showing_saved = true;
                }
            }
            if !showing_saved {
                *status.write() = CatalogStatus::Fetching;
            }
            info!("catalog: starting fetch");

            // The two category lists are asked beside the three lists (spec
            // 2026-10-15 §4.1, §5), in a task of their own: only the titles
            // wait for them.
            let asker = portal.clone();
            let categories = tokio::spawn(async move { asker.fetch_categories().await });
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
                    let (stored, input) = StoredCatalog::new(c);
                    let of = put_catalog(&inner, &revision, stored);
                    *status.write() = CatalogStatus::Loaded;
                    // The channels, zapping, `f` and the guide work from here on,
                    // whatever the categories do. The titles wait for them (15 s
                    // at most), then take up to ~150 ms for a big portal on the
                    // blocking pool: off the screen and off the async workers,
                    // where the relay runs.
                    let categories = categories_after(categories, categories_wait).await;
                    // A newer catalog is in: these would never be put, so none is built or logged.
                    if revision.load(Ordering::Acquire) != of {
                        return;
                    }
                    let build = move || input.build(&categories);
                    match tokio::task::spawn_blocking(build).await {
                        Ok(built) => {
                            let built = Arc::new(built);
                            let put =
                                put_titles(&titles, &titles_revision, &revision, of, built.clone());
                            // The file keeps the titles that show.
                            if let (true, Some(path)) = (put, vod_cache) {
                                write_vod_cache(path, built, vod_cache_retired);
                            }
                        }
                        Err(e) => warn!("vod: titles not built ({e})"),
                    }
                }
                // The same list as the one that shows: it stays as it is, and
                // no titles wait for the categories.
                Err(PortalError::Unchanged) => {
                    categories.abort();
                    *status.write() = CatalogStatus::Loaded;
                }
                Err(e) => {
                    categories.abort();
                    let msg = format!("{}", e);
                    warn!("catalog: fetch failed after {} ms: {}", t0.elapsed().as_millis(), msg);
                    // An M3U source keeps the list it shows, and says why it did
                    // not change. An Xtream portal's refetch stays as it was.
                    let keep = portal.kind() == SourceKind::M3u && inner.read().is_some();
                    if matches!(e, PortalError::NotAList) {
                        *notice.lock() = Some(CatalogNotice::NotAList);
                    } else if keep {
                        *notice.lock() = Some(CatalogNotice::NotUpdated);
                    }
                    *status.write() = if keep {
                        CatalogStatus::Loaded
                    } else {
                        CatalogStatus::Failed(msg)
                    };
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

    /// What up/down zapping steps through ([`ZapRing`]), `None` until a catalog
    /// has loaded. Worked out once per catalog, lock and marks: a press asks
    /// again and gets the same ring, an `Arc`, with nothing copied.
    pub fn zap_ring(&self) -> Option<Arc<ZapRing>> {
        let unlocked = self.adult_unlocked();
        // The revision before the marks: marks set in between are noticed next time.
        let defect_revision = self.defect_revision();
        let defect = self.defect_marks();
        let g = self.inner.read();
        Some(
            g.as_ref()?
                .zap_ring(self.kind(), unlocked, &defect, defect_revision),
        )
    }

    /// The zap ring's channels as a list: a copy, for tests and tools. The app
    /// steps through [`CatalogStore::zap_ring`].
    pub fn zap_channels(&self) -> Vec<LiveChannel> {
        self.zap_ring()
            .map(|ring| ring.channels().cloned().collect())
            .unwrap_or_default()
    }

    /// The name of live channel `sid` when it shows: adult ones while locked do
    /// not; a defect one does (a preset, a favorite or the search still plays
    /// it). One read lock and a scan, no copy of the list.
    pub fn live_name(&self, sid: i64) -> Option<String> {
        let g = self.inner.read();
        g.as_ref()?
            .live
            .iter()
            .find(|c| c.stream_id == sid && self.shows_live(c))
            .map(|c| c.name.clone())
    }

    /// The name of live channel `sid` and whether it has catch-up
    /// (`tv_archive == 1`), when it shows as for [`CatalogStore::live_name`]. One
    /// read lock and a scan, no copy of the list: what a zap's EPG fetch needs
    /// of the channel.
    pub fn live_entry(&self, sid: i64) -> Option<(String, bool)> {
        let g = self.inner.read();
        g.as_ref()?
            .live
            .iter()
            .find(|c| c.stream_id == sid && self.shows_live(c))
            .map(|c| (c.name.clone(), c.tv_archive == 1))
    }

    /// The M3U channels marked defect from now on: stream id → when it failed,
    /// unix seconds. The app hands over every change of its marks.
    pub fn set_defect(&self, marks: HashMap<i64, i64>) {
        *self.defect.write() = Arc::new(marks);
        self.defect_revision.fetch_add(1, Ordering::Release);
    }

    /// Whether channel `sid` is marked defect.
    pub fn is_defect(&self, sid: i64) -> bool {
        self.defect.read().contains_key(&sid)
    }

    /// The marks, as an `Arc`: no copy of the map.
    pub fn defect_marks(&self) -> Arc<HashMap<i64, i64>> {
        self.defect.read().clone()
    }

    /// How many times the marks were set: 0 until the first `set_defect`.
    pub fn defect_revision(&self) -> u64 {
        self.defect_revision.load(Ordering::Acquire)
    }

    /// The channel list's groups ([`ChannelGroups::build`]) of the channels that
    /// show (the adult ones while locked do not), the defect ones in their own
    /// group, with `favorites` as the source's favorites. One read lock to take
    /// the channels (an `Arc`), the work outside it; `None` until a catalog has
    /// loaded.
    pub fn channel_groups(&self, favorites: &HashSet<i64>) -> Option<ChannelGroups> {
        let live = self.inner.read().as_ref()?.live.clone();
        let unlocked = self.adult_unlocked();
        let defect = self.defect_marks();
        Some(ChannelGroups::build(
            live,
            |c| live_shows(unlocked, c),
            &defect,
            favorites,
        ))
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
        // An M3U list's entries are channels of their own, not qualities of one.
        if self.kind() == SourceKind::M3u {
            return QualityNav::default();
        }
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

    /// The catch-up (`tv_archive == 1`) variant of a channel, by name. Used by
    /// the zap toast's EPG fetch path: some Xtream portals (hnlol et al.) only
    /// populate EPG against the tv_archive=1 stream_ids - the live variants
    /// return empty even though they share a programme schedule with their
    /// archive twin. Matching is by normalized name so 'NL | NPO 1' (live) and
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
        for x in c.live.iter() {
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

    /// The thuisbioscoop's titles as the lock stands now ([`Titles`]): those of
    /// the store's catalog once they are built (`put_titles`), else those of the
    /// cache file once read ([`CatalogStore::read_vod_cache`]); none before.
    pub fn titles(&self) -> Option<Titles> {
        let unlocked = self.adult_unlocked();
        let held = self.titles.read();
        held.as_ref().map(|(index, saved_at)| Titles {
            index: index.clone(),
            unlocked,
            saved_at: *saved_at,
        })
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

#[cfg(test)]
mod titles_tests {
    //! What only the crate sees of the titles' input (Rulings 22 and 53) and
    //! the bound of Ruling 1'.
    use super::*;
    use crate::args::XtreamCreds;
    use crate::portal::xtream::XtreamPortal;

    /// A portal that is never asked anything.
    fn idle_portal() -> Arc<dyn Portal> {
        Arc::new(XtreamPortal::new(XtreamCreds {
            username: "u".into(),
            password: "p".into(),
            host: "h".into(),
            port: 80,
        }))
    }

    #[test]
    fn the_stored_lists_give_up_what_only_the_index_reads_and_share_the_rest() {
        let (stored, input) = StoredCatalog::new(PortalCatalog {
            movies: vec![Movie {
                stream_id: 1,
                name: "Een (2001)".into(),
                stream_icon: Some("https://c.example/een.jpg".into()),
                ..Movie::default()
            }],
            series: vec![Series {
                series_id: 9,
                name: "Dark".into(),
                cover: Some("https://c.example/dark.jpg".into()),
                genre: Some("Drama".into()),
                release_date: Some("2017-12-01".into()),
                release_date_camel: Some("2017-12-01".into()),
                ..Series::default()
            }],
            ..PortalCatalog::default()
        });
        // Ruling 22: no address held twice once the index has it.
        assert!(stored.movies.iter().all(|m| m.stream_icon.is_none()));
        assert!(stored
            .series
            .iter()
            .all(|s| s.cover.is_none() && s.genre.is_none() && s.released().is_none()));
        // Ruling 53: the build reads the stored lists themselves, no copy.
        assert!(Arc::ptr_eq(&stored.catalog, &input.lists));
        let titles = input.build(&Categories::default());
        let dark = titles
            .title(
                titles
                    .find(TitleId {
                        kind: crate::vod::Kind::Series,
                        id: 9,
                    })
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            (dark.cover, dark.year, dark.genres),
            (
                Some("https://c.example/dark.jpg"),
                2017,
                crate::vod::genres::Genre::Drama.bit()
            )
        );
        // What the build was handed goes with it: the store keeps one list.
        assert_eq!(Arc::strong_count(&stored.catalog), 1);
    }

    #[test]
    fn the_titles_wait_15_s_for_the_categories_unless_told_otherwise() {
        let store = CatalogStore::new(idle_portal());
        assert_eq!(store.categories_wait, Duration::from_secs(15));
        let store = store.with_categories_wait(Duration::from_millis(5));
        assert_eq!(store.categories_wait, Duration::from_millis(5));
    }

    /// What the log says on this thread while `work` runs: as app.rs's
    /// `LogCapture`, with a second dispatcher, so that another test's thread
    /// that meets a line first cannot silence it here.
    fn logged(work: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, work);
        let bytes = sink.0.lock().clone();
        String::from_utf8(bytes).unwrap()
    }

    /// Carry 2 (the Task 6a review's m3): serde quotes the value it could not
    /// read, and a file may hold anything. The warn line masks the error and
    /// then cuts it at a character's boundary, so a value of 70,000 characters
    /// is no line of 70,000, and an account the cut would split is masked
    /// whole.
    #[test]
    fn an_unreadable_cache_is_logged_masked_and_cut() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod-h_80.json");
        let write = |filler: usize| {
            let value = format!(
                "{}http://bob:s3cr3t@h/{}",
                "x".repeat(filler),
                "\u{20ac}".repeat(70_000)
            );
            let file =
                format!(r#"{{"version":1,"saved_at":"{value}","categories":[],"titles":[]}}"#);
            std::fs::write(&path, file).unwrap();
        };
        // Where the error quotes the value: the account goes across the cut.
        write(0);
        let Err(raw) = crate::vod::cache::load(&path) else {
            panic!("text is no time");
        };
        let before = raw[..raw.find("http://").unwrap()].chars().count();
        write(LOGGED_ERROR_CHARS - before - "http://bob:s3".len());
        let slot = RwLock::new(None);
        let revision = AtomicU64::new(0);
        let text = logged(|| read_titles(&path, &slot, &revision));
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("vod: cache")).collect();
        assert_eq!(lines.len(), 1, "{} lines", lines.len());
        let line = lines[0];
        let chars = line.chars().count();
        assert!(
            chars < LOGGED_ERROR_CHARS + 150,
            "a line of {chars} characters"
        );
        assert!(line.contains(" WARN "), "{line}");
        assert!(
            line.contains("vod: cache unreadable (invalid type: string \"xxx"),
            "{line}"
        );
        assert!(
            line.ends_with("...) - ignored, the next load writes a new one"),
            "{line}"
        );
        assert!(!line.contains("bob") && !line.contains("s3cr3t"), "{line}");
        assert!(slot.read().is_none(), "nothing put");
        assert_eq!(revision.load(Ordering::Acquire), 0);
    }

    /// Carry 3 (Task 6a, concern 2): a save that fails, as one of a file that
    /// another program holds, is one warn line that names the file and not its
    /// folder (which holds the user's name), and leaves the file's place as it
    /// was.
    #[test]
    fn a_save_that_fails_is_one_line_with_the_files_name_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod-h_80.json");
        // Its place is taken.
        std::fs::create_dir(&path).unwrap();
        let titles = TitleIndex::build(
            &[],
            &[Series {
                series_id: 1,
                name: "Dark".into(),
                ..Series::default()
            }],
            &Categories::default(),
            2027,
        );
        let text = logged(|| keep_titles(&path, &titles, &Mutex::new(false)));
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("vod: cache")).collect();
        assert_eq!(lines.len(), 1, "{text}");
        let line = lines[0];
        assert!(line.contains(" WARN "), "{line}");
        assert!(
            line.contains("vod: cache not written (vod-h_80.json: "),
            "{line}"
        );
        let folder = dir.path().file_name().unwrap().to_string_lossy();
        assert!(!line.contains(&*folder), "{line}");
        assert!(!line.contains('\\') && !line.contains('/'), "{line}");
        assert!(path.is_dir() && !path.with_extension("json.tmp").exists());
    }
}
