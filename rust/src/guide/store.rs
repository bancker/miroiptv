//! The guide's EPG in memory, which channels need fetching and in what order,
//! and the filter.

use crate::catalog::GuideChannel;
use crate::epg::{Epg, EpgEntry};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// A channel's EPG as fetched, or as read from the cache file.
#[derive(Debug, Clone)]
pub struct ChannelEpg {
    /// Shared: the cache file is written from copies of these handles, on
    /// another thread.
    pub epg: Arc<Epg>,
    pub fetched_at: DateTime<Utc>,
    /// The portal sends `has_archive = 1` for some programme of this channel,
    /// so its 0 means "not kept" (see `time::playable`). Private: outside this
    /// module it is read through [`ChannelEpg::trusts_flag_for`], which also
    /// looks at when the EPG was fetched.
    trusts_archive_flag: bool,
}

impl ChannelEpg {
    pub fn new(epg: Epg, fetched_at: DateTime<Utc>) -> Self {
        let trusts_archive_flag = epg.entries().iter().any(|e| e.has_archive == Some(true));
        Self {
            epg: Arc::new(epg),
            fetched_at,
            trusts_archive_flag,
        }
    }

    pub fn entries(&self) -> &[EpgEntry] {
        self.epg.entries()
    }

    /// Whether this programme's own `has_archive` may decide (see
    /// `time::playable`): only on a channel that says 1 somewhere, and only for
    /// a programme that had ended when the EPG was fetched. The portal's 0 for a
    /// programme that still ran then says nothing about it now.
    pub fn trusts_flag_for(&self, entry: &EpgEntry) -> bool {
        self.trusts_archive_flag && entry.end <= self.fetched_at
    }
}

/// The EPG of the guide's channels, by catch-up (archive) stream id.
#[derive(Debug, Default)]
pub struct EpgStore {
    channels: HashMap<i64, ChannelEpg>,
    /// Counts every change: what the screen works out from the store (the
    /// filter, the date list) is worked out again when it moves.
    revision: u64,
}

impl EpgStore {
    pub fn get(&self, archive_id: i64) -> Option<&ChannelEpg> {
        self.channels.get(&archive_id)
    }

    /// Keeps `epg` for the channel. A copy fetched earlier than the one kept
    /// (the cache file, read after a fetch already landed) changes nothing.
    pub fn insert(&mut self, archive_id: i64, epg: ChannelEpg) {
        if self
            .channels
            .get(&archive_id)
            .is_some_and(|kept| kept.fetched_at > epg.fetched_at)
        {
            return;
        }
        self.channels.insert(archive_id, epg);
        self.revision += 1;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many channels have EPG.
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// How many programmes the store holds, for the log.
    pub fn programmes(&self) -> usize {
        self.channels.values().map(|c| c.entries().len()).sum()
    }

    /// Whether the channel must be fetched (again): never fetched; fetched 6
    /// hours ago or longer; or with less than 24 hours of programmes ahead and
    /// fetched an hour ago or longer.
    pub fn needs_fetch(&self, archive_id: i64, now: DateTime<Utc>) -> bool {
        let Some(c) = self.channels.get(&archive_id) else {
            return true;
        };
        let age = now - c.fetched_at;
        if age >= Duration::hours(6) {
            return true;
        }
        let ahead = c
            .entries()
            .last()
            .is_some_and(|e| e.end >= now + Duration::hours(24));
        !ahead && age >= Duration::hours(1)
    }

    /// The last day on which any channel has a programme (`day_of` names the
    /// day of a moment). `day_of` is asked once, for the latest start of all: a
    /// later moment is never on an earlier day (see `time::day_of_in`), and a
    /// local-time conversion is an OS call, so not one per channel.
    pub fn last_day(&self, day_of: impl Fn(DateTime<Utc>) -> NaiveDate) -> Option<NaiveDate> {
        self.channels
            .values()
            .filter_map(|c| c.entries().last())
            .map(|e| e.start)
            .max()
            .map(day_of)
    }

    /// Forgets everything: another portal's catalog took over.
    pub fn clear(&mut self) {
        self.channels.clear();
        self.revision += 1;
    }
}

/// The order in which the guide's channels are fetched: the columns on screen
/// (`shown`, a range of indices into the guide's list), then the ones after
/// them, then the ones before them, nearest first. Allocates nothing.
pub fn fetch_order(shown: Range<usize>, total: usize) -> impl Iterator<Item = usize> {
    let start = shown.start.min(total);
    let end = shown.end.min(total).max(start);
    (start..end).chain(end..total).chain((0..start).rev())
}

/// What a filter leaves of the guide on one day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterResult {
    /// Indices into the guide's channel list, in its order.
    pub channels: Vec<usize>,
    /// Per catch-up id: the indices of the programmes whose title holds the
    /// text, ascending.
    pub matches: HashMap<i64, Vec<usize>>,
}

/// The channels whose name holds `text` (trimmed, any case), or that have a
/// programme within `range_of(entries)` (the chosen day) whose title holds it.
/// A range that reaches past the entries is cut to them and an inverted one is
/// empty, so a wrong `range_of` cannot panic here (a panic ends the player).
/// Run when the text, the day or the store changes, never per frame.
pub fn filter_channels(
    channels: &[GuideChannel],
    store: &EpgStore,
    text: &str,
    range_of: impl Fn(&[EpgEntry]) -> Range<usize>,
) -> FilterResult {
    let needle = text.trim().to_lowercase();
    let mut result = FilterResult::default();
    if needle.is_empty() {
        result.channels = (0..channels.len()).collect();
        return result;
    }
    for (i, ch) in channels.iter().enumerate() {
        let by_name = ch.name.to_lowercase().contains(&needle);
        let hits: Vec<usize> = store
            .get(ch.archive_id)
            .map(|c| {
                let entries = c.entries();
                let len = entries.len();
                let range = range_of(entries);
                (range.start.min(len)..range.end.min(len))
                    .filter(|&j| entries[j].title.to_lowercase().contains(&needle))
                    .collect()
            })
            .unwrap_or_default();
        if by_name || !hits.is_empty() {
            result.channels.push(i);
            if !hits.is_empty() {
                result.matches.insert(ch.archive_id, hits);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 0, 0, 0).unwrap() + Duration::hours(h)
    }

    fn entry(title: &str, from: i64, to: i64) -> EpgEntry {
        EpgEntry {
            title: title.into(),
            start: t(from),
            end: t(to),
            has_archive: None,
        }
    }

    /// One programme from an hour before `now` to `ahead` hours after it.
    fn epg_until(now: i64, ahead: i64) -> Epg {
        Epg::new(vec![entry("A", now - 1, now + ahead)])
    }

    fn channel(id: i64, name: &str) -> GuideChannel {
        GuideChannel {
            name: name.into(),
            live_id: id - 100,
            archive_id: id,
            archive_days: 7,
            logo: None,
            adult: false,
        }
    }

    #[test]
    fn a_channel_is_fetched_again_when_its_epg_is_old_or_runs_out() {
        let mut store = EpgStore::default();
        let now = t(12);
        assert!(store.needs_fetch(601, now), "never fetched");
        store.insert(601, ChannelEpg::new(epg_until(12, 48), t(11)));
        assert!(!store.needs_fetch(601, now), "an hour old, two days ahead");
        assert!(store.needs_fetch(601, t(17)), "six hours old");
        store.insert(
            602,
            ChannelEpg::new(epg_until(12, 10), now - Duration::minutes(30)),
        );
        assert!(
            !store.needs_fetch(602, now),
            "less than a day ahead, but fetched half an hour ago"
        );
        assert!(
            store.needs_fetch(602, now + Duration::minutes(31)),
            "less than a day ahead and an hour old"
        );
        store.insert(603, ChannelEpg::new(epg_until(12, 24), t(11)));
        assert!(
            !store.needs_fetch(603, now),
            "exactly a day ahead is not less than a day, an hour old"
        );
    }

    #[test]
    fn an_older_copy_never_replaces_a_newer_one() {
        let mut store = EpgStore::default();
        store.insert(
            601,
            ChannelEpg::new(Epg::new(vec![entry("Nieuw", 10, 11)]), t(12)),
        );
        let revision = store.revision();
        store.insert(
            601,
            ChannelEpg::new(Epg::new(vec![entry("Oud", 10, 11)]), t(2)),
        );
        assert_eq!(store.get(601).unwrap().entries()[0].title, "Nieuw");
        assert_eq!(store.revision(), revision, "nothing changed");
        assert_eq!((store.len(), store.programmes()), (1, 1));
    }

    #[test]
    fn the_revision_rises_on_a_first_insert_a_newer_copy_and_a_clear() {
        let mut store = EpgStore::default();
        let at_start = store.revision();
        store.insert(
            601,
            ChannelEpg::new(Epg::new(vec![entry("Eerst", 10, 11)]), t(2)),
        );
        let first = store.revision();
        assert!(first > at_start, "a first insert is a change");
        store.insert(
            601,
            ChannelEpg::new(Epg::new(vec![entry("Later", 10, 11)]), t(12)),
        );
        let newer = store.revision();
        assert!(newer > first, "a newer copy replaces the one kept");
        assert_eq!(store.get(601).unwrap().entries()[0].title, "Later");
        store.clear();
        assert!(
            store.revision() > newer,
            "forgetting everything is a change"
        );
    }

    #[test]
    fn a_channel_that_says_has_archive_1_somewhere_is_trusted_on_its_0() {
        let mut yes = entry("Ja", 1, 2);
        yes.has_archive = Some(true);
        let mut no = entry("Nee", 2, 3);
        no.has_archive = Some(false);
        let both = ChannelEpg::new(Epg::new(vec![yes, no.clone()]), t(3));
        assert!(both.trusts_archive_flag);
        assert!(!ChannelEpg::new(Epg::new(vec![no]), t(3)).trusts_archive_flag);
    }

    #[test]
    fn a_zero_from_before_a_programme_ended_is_not_trusted() {
        let with_flag = |title: &str, from: i64, to: i64, flag: bool| {
            let mut e = entry(title, from, to);
            e.has_archive = Some(flag);
            e
        };
        let says_yes = with_flag("Ja", 1, 2, true);
        let over = with_flag("Klaar", 9, 10, false);
        let running = with_flag("Bezig", 11, 13, false);

        // Fetched at 12:00: "Klaar" had ended, "Bezig" was still on.
        let ch = ChannelEpg::new(
            Epg::new(vec![says_yes.clone(), over.clone(), running.clone()]),
            t(12),
        );
        assert!(ch.trusts_archive_flag);
        assert!(
            ch.trusts_flag_for(&over),
            "ended before the fetch: its 0 is a fact"
        );
        assert!(
            !ch.trusts_flag_for(&running),
            "still on at the fetch: its 0 says nothing about it now"
        );

        // Fetched the moment "Klaar" ended: it had ended.
        let at_its_end = ChannelEpg::new(Epg::new(vec![says_yes, over.clone()]), t(10));
        assert!(at_its_end.trusts_flag_for(&over));

        // A channel that never says 1 trusts no programme's 0.
        let silent = ChannelEpg::new(Epg::new(vec![over.clone(), running.clone()]), t(12));
        assert!(!silent.trusts_archive_flag);
        assert!(!silent.trusts_flag_for(&over));
        assert!(!silent.trusts_flag_for(&running));
    }

    #[test]
    fn the_columns_on_screen_come_first_then_the_pages_after_then_before() {
        let order: Vec<usize> = fetch_order(4..6, 9).collect();
        assert_eq!(order, [4, 5, 6, 7, 8, 3, 2, 1, 0]);
        let clamped: Vec<usize> = fetch_order(7..12, 9).collect();
        assert_eq!(clamped, [7, 8, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(fetch_order(0..5, 0).count(), 0);
    }

    #[test]
    fn the_filter_finds_channels_by_name_and_by_programme() {
        let channels = vec![
            channel(601, "NL | NPO 1"),
            channel(602, "NL | NPO 2"),
            channel(603, "NL | RTL 4"),
        ];
        let mut store = EpgStore::default();
        let npo2 = vec![
            entry("NOS Journaal", 8, 9),
            entry("Nieuwsuur", 21, 22),
            entry("NOS Journaal", 22, 23),
        ];
        store.insert(602, ChannelEpg::new(Epg::new(npo2), t(1)));
        store.insert(
            603,
            ChannelEpg::new(Epg::new(vec![entry("RTL Nieuws", 19, 20)]), t(1)),
        );
        let all = |e: &[EpgEntry]| 0..e.len();
        let r = filter_channels(&channels, &store, "journaal", all);
        assert_eq!(r.channels, [1]);
        assert_eq!(r.matches[&602], [0, 2]);
        let r = filter_channels(&channels, &store, "  NPO  ", all);
        assert_eq!(r.channels, [0, 1], "by name, in any case, trimmed");
        assert!(r.matches.is_empty());
        assert_eq!(
            filter_channels(&channels, &store, "", all).channels,
            [0, 1, 2]
        );
        let blank = filter_channels(&channels, &store, "   ", all);
        assert_eq!(blank.channels, [0, 1, 2], "blank text is no filter");
        assert!(blank.matches.is_empty());
        assert!(filter_channels(&channels, &store, "voetbal", all)
            .channels
            .is_empty());
    }

    #[test]
    fn the_filter_looks_at_the_chosen_day_only() {
        let channels = vec![channel(602, "NL | NPO 2")];
        let mut store = EpgStore::default();
        let two_days = vec![entry("Journaal", 8, 9), entry("Sport", 30, 31)];
        store.insert(602, ChannelEpg::new(Epg::new(two_days), t(1)));
        let second_day = |e: &[EpgEntry]| e.partition_point(|x| x.start < t(24))..e.len();
        assert!(filter_channels(&channels, &store, "journaal", second_day)
            .channels
            .is_empty());
        let r = filter_channels(&channels, &store, "sport", second_day);
        assert_eq!(r.matches[&602], [1]);
    }

    #[test]
    fn a_range_outside_the_entries_is_cut_to_them() {
        let channels = vec![channel(602, "NL | NPO 2")];
        let mut store = EpgStore::default();
        let listing = vec![
            entry("Journaal", 8, 9),
            entry("Sport", 9, 10),
            entry("Journaal", 10, 11),
            entry("Weer", 11, 12),
        ];
        store.insert(602, ChannelEpg::new(Epg::new(listing), t(1)));
        let inside = |r: &FilterResult| {
            r.matches.iter().all(|(id, hits)| {
                let len = store.get(*id).unwrap().entries().len();
                hits.iter().all(|&j| j < len)
            })
        };

        let past_the_end = |e: &[EpgEntry]| 0..e.len() + 5;
        let r = filter_channels(&channels, &store, "journaal", past_the_end);
        assert_eq!(r.matches[&602], [0, 2], "cut to the entries there are");
        assert!(inside(&r));

        let beyond = |e: &[EpgEntry]| (e.len() + 2)..(e.len() + 9);
        let r = filter_channels(&channels, &store, "journaal", beyond);
        assert!(
            r.channels.is_empty() && r.matches.is_empty(),
            "wholly outside"
        );
        assert!(inside(&r));

        // Reversed on purpose: it is the input under test.
        #[allow(clippy::reversed_empty_ranges)]
        let inverted = |_: &[EpgEntry]| 3..1;
        let r = filter_channels(&channels, &store, "journaal", inverted);
        assert!(r.channels.is_empty() && r.matches.is_empty(), "inverted");
        assert!(inside(&r));
    }

    #[test]
    fn the_last_day_is_that_of_the_last_programme_anywhere() {
        let mut store = EpgStore::default();
        assert_eq!(store.last_day(|t| t.date_naive()), None);
        store.insert(601, ChannelEpg::new(Epg::new(vec![entry("A", 1, 2)]), t(1)));
        store.insert(
            602,
            ChannelEpg::new(Epg::new(vec![entry("B", 50, 51)]), t(1)),
        );
        assert_eq!(store.last_day(|t| t.date_naive()), Some(t(50).date_naive()));
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn the_last_day_is_worked_out_once_however_many_channels_there_are() {
        let asked = std::cell::Cell::new(0);
        let day_of = |moment: DateTime<Utc>| {
            asked.set(asked.get() + 1);
            moment.date_naive()
        };
        let mut store = EpgStore::default();
        assert_eq!(store.last_day(day_of), None);
        assert_eq!(asked.get(), 0, "no programme, nothing to convert");
        for (id, from) in [(601, 5), (602, 50), (603, 20), (604, 30)] {
            let epg = Epg::new(vec![entry("A", from, from + 1)]);
            store.insert(id, ChannelEpg::new(epg, t(1)));
        }
        assert_eq!(store.last_day(day_of), Some(t(50).date_naive()));
        assert_eq!(
            asked.get(),
            1,
            "once: a local-time conversion is an OS call"
        );
    }
}
