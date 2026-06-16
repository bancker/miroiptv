use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preset {
    pub stream_id: i64,
    pub name: String,
}

/// Ten car-radio style channel presets addressed by digit keys 0-9.
/// Long-press a digit to store the current channel; tap it to recall it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Presets {
    slots: [Option<Preset>; 10],
}

impl Presets {
    /// Stored preset for digit 0-9, if any.
    pub fn get(&self, digit: u8) -> Option<&Preset> {
        self.slots.get(digit as usize).and_then(|s| s.as_ref())
    }

    /// Store a channel into a digit slot (overwrites any existing one).
    pub fn set(&mut self, digit: u8, stream_id: i64, name: &str) {
        if let Some(slot) = self.slots.get_mut(digit as usize) {
            *slot = Some(Preset {
                stream_id,
                name: name.to_owned(),
            });
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_get_roundtrips() {
        let mut p = Presets::default();
        assert!(p.get(3).is_none());
        p.set(3, 42, "NPO 3");
        assert_eq!(p.get(3).map(|e| e.stream_id), Some(42));
        assert_eq!(p.get(3).map(|e| e.name.as_str()), Some("NPO 3"));
    }

    #[test]
    fn set_overwrites_slot() {
        let mut p = Presets::default();
        p.set(0, 1, "A");
        p.set(0, 2, "B");
        assert_eq!(p.get(0).map(|e| e.stream_id), Some(2));
    }

    #[test]
    fn out_of_range_digit_is_ignored() {
        let mut p = Presets::default();
        p.set(10, 99, "nope"); // only 0-9 exist
        assert!(p.get(10).is_none());
    }
}
