//! Days, targets and what a programme is for the viewer. Every function that
//! depends on the time zone takes it as a parameter (`_in`), with a wrapper in
//! the computer's local time, so the tests can use fixed zones and a zone with
//! a daylight-saving switch.

use crate::epg::EpgEntry;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use std::ops::Range;

/// Primetime starts at 19:00 (the docx: "focus on prime time (19:00)").
pub const PRIMETIME_HOUR: u32 = 19;

/// What a programme in the guide is for the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playable {
    /// On now: plays live.
    Live,
    /// Over and kept by the portal: plays from its start through catch-up.
    Replay,
    /// Over and not (or no longer) kept.
    Unavailable,
    /// Not on yet.
    Future,
}

/// What `entry` is at `now` on a channel whose catch-up reaches `archive_days`
/// back (0: the portal did not say, so every past programme counts). The
/// programme's own `has_archive` counts only when `trust_flag` is set, that is
/// when the portal sends `has_archive = 1` for some programme of this channel:
/// a portal that sends 0 (or nothing) for everything would otherwise make
/// nothing watchable.
pub fn playable(
    entry: &EpgEntry,
    archive_days: u32,
    trust_flag: bool,
    now: DateTime<Utc>,
) -> Playable {
    if entry.start > now {
        return Playable::Future;
    }
    if now < entry.end {
        return Playable::Live;
    }
    // A window reaching back past the first date there is covers everything.
    let in_window = archive_days == 0
        || now
            .checked_sub_signed(Duration::days(i64::from(archive_days)))
            .is_none_or(|oldest| entry.start >= oldest);
    let flag_allows = !trust_flag || entry.has_archive != Some(false);
    if in_window && flag_allows {
        Playable::Replay
    } else {
        Playable::Unavailable
    }
}

/// The moment `hour`:00 on `day` in `tz`, as UTC: the first of the two when
/// the clock goes back over it, the next whole hour that exists when the clock
/// skips it.
pub fn at_hour_in<Tz: TimeZone>(day: NaiveDate, hour: u32, tz: &Tz) -> DateTime<Utc> {
    (hour..24)
        .find_map(|h| {
            tz.from_local_datetime(&day.and_hms_opt(h, 0, 0)?)
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
        // No zone skips the rest of a day: read the date as UTC then.
        .unwrap_or_else(|| {
            Utc.from_utc_datetime(&day.and_hms_opt(hour.min(23), 0, 0).expect("a whole hour"))
        })
}

/// The moment the day `day` starts in `tz` (local midnight), as UTC.
pub fn day_start_in<Tz: TimeZone>(day: NaiveDate, tz: &Tz) -> DateTime<Utc> {
    at_hour_in(day, 0, tz)
}

/// The day `t` falls on in `tz`. Monotone: a later moment is never on an
/// earlier day, so of several moments the latest is on the latest day, and a
/// caller with many of them converts only that one (a local-time conversion is
/// an OS call; see `EpgStore::last_day`).
pub fn day_of_in<Tz: TimeZone>(t: DateTime<Utc>, tz: &Tz) -> NaiveDate {
    t.with_timezone(tz).date_naive()
}

/// The programmes of `day` in a cleaned listing (sorted, no overlaps; see
/// [`crate::epg::clean_entries`]): those that start on that day, plus the one
/// that runs at its midnight. Two binary searches: cheap enough per frame.
pub fn day_slice_in<Tz: TimeZone>(entries: &[EpgEntry], day: NaiveDate, tz: &Tz) -> Range<usize> {
    let from = day_start_in(day, tz);
    let to = day_start_in(day.succ_opt().unwrap_or(day), tz);
    let first = entries.partition_point(|e| e.end <= from);
    let end = entries.partition_point(|e| e.start < to);
    first..end.max(first)
}

/// [`at_hour_in`] in the computer's local time.
pub fn at_hour(day: NaiveDate, hour: u32) -> DateTime<Utc> {
    at_hour_in(day, hour, &Local)
}

/// [`day_start_in`] in the computer's local time.
pub fn day_start(day: NaiveDate) -> DateTime<Utc> {
    day_start_in(day, &Local)
}

/// [`day_of_in`] in the computer's local time.
pub fn day_of(t: DateTime<Utc>) -> NaiveDate {
    day_of_in(t, &Local)
}

/// [`day_slice_in`] in the computer's local time.
pub fn day_slice(entries: &[EpgEntry], day: NaiveDate) -> Range<usize> {
    day_slice_in(entries, day, &Local)
}

/// The row a column lines up on: within `range`, the programme running at `t`;
/// in a gap the first one after it; after the last one, the last. `None` when
/// the range is empty.
pub fn target_index(entries: &[EpgEntry], range: Range<usize>, t: DateTime<Utc>) -> Option<usize> {
    let day = entries.get(range.clone())?;
    if day.is_empty() {
        return None;
    }
    let i = day.partition_point(|e| e.end <= t);
    Some(range.start + i.min(day.len() - 1))
}

/// With a filter: the first match (entry indices, ascending) that runs at or
/// after `t`, else the last match.
pub fn match_target(entries: &[EpgEntry], matches: &[usize], t: DateTime<Utc>) -> Option<usize> {
    matches
        .iter()
        .copied()
        .find(|&i| entries.get(i).is_some_and(|e| e.end > t))
        .or_else(|| matches.last().copied())
}

/// The days the date list offers: from `today` minus the longest catch-up of
/// the guide's channels (7 days when no channel says; never over 31) to the
/// last day with programmes, at least today and at most a week ahead.
pub fn date_range(
    today: NaiveDate,
    max_archive_days: u32,
    last_day: Option<NaiveDate>,
) -> Vec<NaiveDate> {
    let back = if max_archive_days == 0 {
        7
    } else {
        max_archive_days.min(31)
    };
    let first = today - Duration::days(i64::from(back));
    let last = last_day
        .unwrap_or(today)
        .clamp(today, today + Duration::days(7));
    first.iter_days().take_while(|d| *d <= last).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::offset::MappedLocalTime;
    use chrono::{FixedOffset, NaiveDateTime};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn date(y: i32, mo: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap()
    }

    fn entry(title: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> EpgEntry {
        EpgEntry {
            title: title.into(),
            start,
            end,
            has_archive: None,
        }
    }

    fn east(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).unwrap()
    }

    /// Central European time around the switch of 25 October 2026: +02:00
    /// until 01:00 UTC, +01:00 after, so that day lasts 25 hours.
    #[derive(Clone, Copy, Debug)]
    struct SwitchZone;

    fn switch_utc() -> NaiveDateTime {
        date(2026, 10, 25).and_hms_opt(1, 0, 0).unwrap()
    }

    impl TimeZone for SwitchZone {
        type Offset = FixedOffset;

        fn from_offset(_: &FixedOffset) -> Self {
            SwitchZone
        }

        fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<FixedOffset> {
            self.offset_from_local_datetime(&local.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_local_datetime(
            &self,
            local: &NaiveDateTime,
        ) -> MappedLocalTime<FixedOffset> {
            // 02:00-03:00 local happens twice: in summer time, then in winter time.
            if *local < switch_utc() + Duration::hours(1) {
                MappedLocalTime::Single(east(2))
            } else if *local >= switch_utc() + Duration::hours(2) {
                MappedLocalTime::Single(east(1))
            } else {
                MappedLocalTime::Ambiguous(east(2), east(1))
            }
        }

        fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            if *utc < switch_utc() {
                east(2)
            } else {
                east(1)
            }
        }
    }

    #[test]
    fn what_a_programme_is_for_the_viewer() {
        let now = utc(2026, 10, 12, 20, 0);
        let past = entry(
            "Journaal",
            utc(2026, 10, 12, 18, 0),
            utc(2026, 10, 12, 18, 30),
        );
        let live = entry("Nu", utc(2026, 10, 12, 19, 45), utc(2026, 10, 12, 20, 15));
        let later = entry(
            "Straks",
            utc(2026, 10, 12, 20, 15),
            utc(2026, 10, 12, 21, 0),
        );
        assert_eq!(playable(&live, 7, false, now), Playable::Live);
        assert_eq!(playable(&later, 7, false, now), Playable::Future);
        assert_eq!(playable(&past, 7, false, now), Playable::Replay);
        // A programme that starts at `now` is on; one that ended at `now` is over.
        let starts_now = entry("Begint", now, now + Duration::minutes(30));
        assert_eq!(playable(&starts_now, 7, false, now), Playable::Live);
        let ended_now = entry("Klaar", now - Duration::minutes(30), now);
        assert_eq!(playable(&ended_now, 7, false, now), Playable::Replay);
    }

    #[test]
    fn catch_up_reaches_as_far_back_as_the_channel_says() {
        let now = utc(2026, 10, 12, 20, 0);
        let start = now - Duration::days(8);
        let old = entry("Oud", start, start + Duration::minutes(30));
        assert_eq!(playable(&old, 7, false, now), Playable::Unavailable);
        assert_eq!(
            playable(&old, 0, false, now),
            Playable::Replay,
            "0 days: the portal did not say"
        );
    }

    #[test]
    fn a_catch_up_window_wider_than_dates_go_back_reaches_everything() {
        // The portal's `tv_archive_duration` is an i32: a "forever" such as
        // 99999999 days reaches back past the first date chrono has. The
        // subtraction must not panic (the release build aborts on a panic).
        let now = utc(2026, 10, 12, 20, 0);
        let start = now - Duration::days(400);
        let old = entry("Oud", start, start + Duration::minutes(30));
        assert_eq!(playable(&old, 99_999_999, false, now), Playable::Replay);
        assert_eq!(
            playable(&old, i32::MAX as u32, false, now),
            Playable::Replay
        );
        assert_eq!(playable(&old, u32::MAX, false, now), Playable::Replay);
    }

    #[test]
    fn has_archive_counts_only_for_a_channel_that_also_says_yes() {
        let now = utc(2026, 10, 12, 20, 0);
        let mut not_kept = entry("Niet", now - Duration::hours(2), now - Duration::hours(1));
        not_kept.has_archive = Some(false);
        assert_eq!(playable(&not_kept, 7, true, now), Playable::Unavailable);
        assert_eq!(playable(&not_kept, 7, false, now), Playable::Replay);
    }

    #[test]
    fn a_day_runs_from_local_midnight_to_local_midnight() {
        let cest = east(2);
        let entries = vec![
            // 23:30-00:30 local: over midnight.
            entry("Laat", utc(2026, 10, 11, 21, 30), utc(2026, 10, 11, 22, 30)),
            entry("Nacht", utc(2026, 10, 11, 22, 30), utc(2026, 10, 12, 4, 0)),
            entry(
                "Ochtend",
                utc(2026, 10, 12, 4, 0),
                utc(2026, 10, 12, 21, 50),
            ),
            // 23:50-00:10 local.
            entry(
                "Avond",
                utc(2026, 10, 12, 21, 50),
                utc(2026, 10, 12, 22, 10),
            ),
        ];
        // 12 October: what runs at its midnight, and what starts on it.
        assert_eq!(day_slice_in(&entries, date(2026, 10, 12), &cest), 0..4);
        // 11 October: "Laat" starts on it.
        assert_eq!(day_slice_in(&entries, date(2026, 10, 11), &cest), 0..1);
        // 13 October: only "Avond", which runs at its midnight.
        assert_eq!(day_slice_in(&entries, date(2026, 10, 13), &cest), 3..4);
        assert!(day_slice_in(&entries, date(2026, 10, 20), &cest).is_empty());
    }

    #[test]
    fn the_day_the_clocks_go_back_lasts_25_hours() {
        let z = SwitchZone;
        let day = date(2026, 10, 25);
        assert_eq!(day_start_in(day, &z), utc(2026, 10, 24, 22, 0));
        assert_eq!(
            day_start_in(date(2026, 10, 26), &z),
            utc(2026, 10, 25, 23, 0)
        );
        // 23:30 local on the 25th is 22:30 UTC: still the 25th, though 24 hours
        // after its midnight have passed.
        let late = vec![entry(
            "Laat",
            utc(2026, 10, 25, 22, 30),
            utc(2026, 10, 25, 22, 55),
        )];
        assert_eq!(day_slice_in(&late, day, &z), 0..1);
        assert!(day_slice_in(&late, date(2026, 10, 26), &z).is_empty());
        // Primetime on that day: 19:00 winter time.
        assert_eq!(
            at_hour_in(day, PRIMETIME_HOUR, &z),
            utc(2026, 10, 25, 18, 0)
        );
        assert_eq!(day_of_in(utc(2026, 10, 25, 22, 30), &z), day);
    }

    #[test]
    fn a_column_lines_up_on_what_runs_else_what_comes_next_else_the_last() {
        let t = |h: u32| utc(2026, 10, 12, h, 0);
        let entries = vec![
            entry("A", t(10), t(11)),
            entry("B", t(12), t(13)),
            entry("C", t(13), t(14)),
        ];
        assert_eq!(
            target_index(&entries, 0..3, t(10) + Duration::minutes(30)),
            Some(0)
        );
        assert_eq!(
            target_index(&entries, 0..3, t(11) + Duration::minutes(30)),
            Some(1),
            "in a gap: the next one"
        );
        assert_eq!(
            target_index(&entries, 0..3, t(15)),
            Some(2),
            "after the last: the last"
        );
        assert_eq!(
            target_index(&entries, 1..3, t(9)),
            Some(1),
            "within the day's range"
        );
        assert_eq!(target_index(&entries, 2..2, t(9)), None);
    }

    #[test]
    fn with_a_filter_a_column_lines_up_on_the_next_match() {
        let t = |h: u32| utc(2026, 10, 12, h, 0);
        let entries: Vec<EpgEntry> = (8..20).map(|h| entry("x", t(h), t(h + 1))).collect();
        assert_eq!(match_target(&entries, &[1, 5, 9], t(10)), Some(5));
        assert_eq!(
            match_target(&entries, &[1, 5, 9], t(19)),
            Some(9),
            "none later: the last"
        );
        assert_eq!(match_target(&entries, &[], t(10)), None);
    }

    #[test]
    fn the_date_list_reaches_back_as_far_as_catch_up_and_ahead_as_far_as_programmes() {
        let today = date(2026, 10, 12);
        let days = date_range(today, 7, Some(date(2026, 10, 14)));
        assert_eq!(days.first(), Some(&date(2026, 10, 5)));
        assert_eq!(days.last(), Some(&date(2026, 10, 14)));
        assert_eq!(days.len(), 10);
        // No channel says how far back: a week. Nothing known ahead: today.
        let days = date_range(today, 0, None);
        assert_eq!((days[0], *days.last().unwrap()), (date(2026, 10, 5), today));
        // Never more than a week ahead, never before today, never over a month back.
        let far = date_range(today, 7, Some(date(2026, 12, 1)));
        assert_eq!(*far.last().unwrap(), date(2026, 10, 19));
        let before = date_range(today, 7, Some(date(2026, 10, 1)));
        assert_eq!(*before.last().unwrap(), today);
        assert_eq!(date_range(today, 365, None)[0], date(2026, 9, 11));
    }
}
