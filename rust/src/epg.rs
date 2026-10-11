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
}

#[derive(Debug, Default, Clone)]
pub struct Epg {
    entries: Vec<EpgEntry>,
}

impl Epg {
    pub fn new(mut entries: Vec<EpgEntry>) -> Self {
        entries.sort_by_key(|e| e.start);
        Self { entries }
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
    }
}
