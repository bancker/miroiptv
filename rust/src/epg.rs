use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// "HH:MM" of `t` in time zone `tz`.
pub fn hhmm_in<Tz: chrono::TimeZone>(t: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    t.with_timezone(tz).format("%H:%M").to_string()
}

/// "HH:MM" of `t` in the computer's local time: how programme times show.
pub fn hhmm(t: DateTime<Utc>) -> String {
    hhmm_in(t, &chrono::Local)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpgEntry {
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Whether the portal says the programme can be watched back
    /// (`has_archive`); `None` when it did not say, as in the short EPG.
    #[serde(default)]
    pub has_archive: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct Epg {
    entries: Vec<EpgEntry>,
}

impl Epg {
    /// The portal's listing, cleaned by [`clean_entries`]: sorted, and one
    /// programme at a time.
    pub fn new(entries: Vec<EpgEntry>) -> Self {
        Self {
            entries: clean_entries(entries),
        }
    }

    pub fn entries(&self) -> &[EpgEntry] {
        &self.entries
    }

    pub fn current_at(&self, t: DateTime<Utc>) -> Option<&EpgEntry> {
        self.entries.iter().find(|e| e.start <= t && t < e.end)
    }

    pub fn next_at(&self, t: DateTime<Utc>) -> Option<&EpgEntry> {
        self.entries.iter().find(|e| e.start > t)
    }
}

/// Whether two titles name the same programme: trimmed, in any case.
fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
}

/// `has_archive` of two entries of one programme, merged: watchable when either
/// says so, unknown when neither says anything.
fn merge_archive(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, None) => None,
        _ => Some(false),
    }
}

/// A portal listing made fit for a guide in which one programme runs at a time.
/// This portal's EPG holds the same programme twice a few minutes apart, and
/// programmes that overlap the next one; the 0.5 guide showed both as LIVE.
///
/// - Entries that do not end after they start go.
/// - The rest is sorted by start; entries that start together keep the
///   portal's order.
/// - An entry that overlaps the one kept before it:
///   - with the same title (trimmed, any case) is merged into it, which then
///     ends at the later of the two ends;
///   - that starts together with it, or lies wholly inside it, is dropped;
///   - otherwise ends that one: it ends where this one starts, and its
///     `has_archive` becomes unknown, because the portal's flag described the
///     uncut programme.
///
/// The result is sorted, no two entries overlap, and each ends after it starts.
pub fn clean_entries(mut entries: Vec<EpgEntry>) -> Vec<EpgEntry> {
    entries.retain(|e| e.end > e.start);
    entries.sort_by_key(|e| e.start);
    let mut out: Vec<EpgEntry> = Vec::with_capacity(entries.len());
    for e in entries {
        let Some(prev) = out.last_mut() else {
            out.push(e);
            continue;
        };
        if e.start >= prev.end {
            out.push(e);
        } else if same_title(&prev.title, &e.title) {
            prev.end = prev.end.max(e.end);
            prev.has_archive = merge_archive(prev.has_archive, e.has_archive);
        } else if e.start == prev.start || e.end <= prev.end {
            // The one already kept stays.
        } else {
            prev.end = e.start;
            prev.has_archive = None;
            out.push(e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programme_times_show_in_the_given_time_zone() {
        use chrono::{FixedOffset, TimeZone};
        let t = Utc.with_ymd_and_hms(2026, 10, 11, 18, 30, 0).unwrap();
        let cest = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(hhmm_in(t, &cest), "20:30");
        assert_eq!(hhmm_in(t, &Utc), "18:30");

        // Offsets with minutes, east and west of Greenwich.
        let midday = Utc.with_ymd_and_hms(2026, 10, 11, 12, 15, 0).unwrap();
        let india = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
        let newfoundland = FixedOffset::west_opt(3 * 3600 + 30 * 60).unwrap();
        assert_eq!(hhmm_in(midday, &india), "17:45");
        assert_eq!(hhmm_in(midday, &newfoundland), "08:45");

        // The clock wraps over midnight: 23:30 UTC is already the next day at +02:00.
        let late = Utc.with_ymd_and_hms(2026, 10, 11, 23, 30, 0).unwrap();
        assert_eq!(hhmm_in(late, &cest), "01:30");
    }

    #[test]
    fn merging_archive_flags() {
        assert_eq!(merge_archive(Some(true), Some(false)), Some(true));
        assert_eq!(merge_archive(Some(false), None), Some(false));
        assert_eq!(merge_archive(None, None), None);
    }
}
