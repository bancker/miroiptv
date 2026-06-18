use crate::portal::{Catalog as PortalCatalog, LiveChannel, Portal};
use crate::search::{rank, ItemKind, SearchItem};
use parking_lot::RwLock;
use std::collections::HashMap;
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
/// beats its archive (catch-up) twin, then higher quality wins.
fn variant_score(ch: &LiveChannel) -> u16 {
    let live = if ch.tv_archive == 0 { 1u16 } else { 0 };
    live * 10 + quality_rank(&ch.name) as u16
}

/// Collapse a live-channel list to ONE entry per distinct channel (keyed by
/// [`normalize_channel_name`]), keeping the highest-scoring variant so the
/// user doesn't zap through NPO1 HD / UHD / FHD / catch-up duplicates. Order
/// follows each channel's first appearance in the portal list.
pub fn dedupe_zap(channels: &[LiveChannel]) -> Vec<LiveChannel> {
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
        }
    }

    #[test]
    fn keeps_highest_quality_per_channel() {
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
        assert_eq!(npo1.stream_id, 2, "UHD variant wins");
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStatus {
    Idle,
    Fetching(String),  // current step description
    Loaded,
    Failed(String),
}

pub struct CatalogStore {
    inner: Arc<RwLock<Option<PortalCatalog>>>,
    portal: Arc<dyn Portal>,
    status: Arc<RwLock<CatalogStatus>>,
}

impl CatalogStore {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            portal,
            status: Arc::new(RwLock::new(CatalogStatus::Idle)),
        }
    }

    /// Spawn an async fetch on the current tokio runtime. Non-blocking.
    /// Safe to call repeatedly: subsequent calls trigger a re-fetch.
    pub fn spawn_fetch(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let portal = self.portal.clone();
        let status = self.status.clone();
        *status.write() = CatalogStatus::Fetching("connecting...".into());

        tokio::spawn(async move {
            let t0 = Instant::now();
            *status.write() = CatalogStatus::Fetching("fetching catalog...".into());
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
                    *inner.write() = Some(c);
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
            .map(|c| c.live.clone())
            .unwrap_or_default()
    }

    /// Deduplicated channel list for up/down zapping: one entry per distinct
    /// channel, highest-quality live variant. See [`dedupe_zap`].
    pub fn zap_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| dedupe_zap(&c.live))
            .unwrap_or_default()
    }

    /// Quality ladder for the channel currently playing `sid`, plus the index
    /// within it — for the +/- quality switcher. One read-lock, no full-catalog
    /// clone (only this channel's handful of variants are cloned). Empty ladder
    /// when the catalog isn't loaded or `sid` is unknown.
    pub fn quality_nav(&self, sid: i64) -> QualityNav {
        let g = self.inner.read();
        let Some(c) = g.as_ref() else {
            return QualityNav::default();
        };
        let Some(cur) = c.live.iter().find(|x| x.stream_id == sid) else {
            return QualityNav::default();
        };
        let key = normalize_channel_name(&cur.name);
        let ladder = quality_ladder(&c.live, &key);
        let pos = quality_pos(&ladder, sid, quality_rank(&cur.name));
        QualityNav { ladder, pos }
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
                    .filter(|x| x.tv_archive == 1)
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
                .find(|x| x.tv_archive != 1 && normalize_channel_name(&x.name) == target)
                .map(|x| x.stream_id)
        })
    }

    /// Reverse of live_id_by_name. Used by the guide / zap-toast EPG fetch
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
                .find(|x| x.tv_archive == 1 && normalize_channel_name(&x.name) == target)
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
            v.push(SearchItem {
                id: x.stream_id,
                name: x.name.clone(),
                kind: ItemKind::Live,
            });
        }
        for x in &c.movies {
            v.push(SearchItem {
                id: x.stream_id,
                name: x.name.clone(),
                kind: ItemKind::Movie,
            });
        }
        for x in &c.series {
            v.push(SearchItem {
                id: x.series_id,
                name: x.name.clone(),
                kind: ItemKind::Series,
            });
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
