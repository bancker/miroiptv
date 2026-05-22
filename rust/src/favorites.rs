use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub stream_id: i64,
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Favorites {
    entries: Vec<Entry>,
}

impl Favorites {
    pub fn contains(&self, stream_id: i64) -> bool {
        self.entries.iter().any(|e| e.stream_id == stream_id)
    }

    pub fn toggle(&mut self, stream_id: i64, name: &str) {
        if let Some(pos) = self.entries.iter().position(|e| e.stream_id == stream_id) {
            self.entries.remove(pos);
        } else {
            self.entries.push(Entry {
                stream_id,
                name: name.to_owned(),
            });
        }
    }

    pub fn remove(&mut self, stream_id: i64) {
        self.entries.retain(|e| e.stream_id != stream_id);
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Entry> {
        self.entries.iter()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&s)?)
    }
}
