use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
