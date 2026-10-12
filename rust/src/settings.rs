//! User preferences in %APPDATA%\BongoPlayer\config\settings.json. Portal
//! credentials stay in BongoPlayer.ini next to the exe (see args.rs).

use crate::i18n::Lang;
use crate::news::{default_rules, default_rules_static, NewsRule};
use crate::pin::PinHash;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// "nl" / "en" / "de"; absent until the first-start language choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub swap_zap_keys: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<PinHash>,
    /// Absent = the default rules; an empty list is the user's own choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub news_rules: Option<Vec<NewsRule>>,
}

impl Settings {
    /// The saved preferences. A missing file gives the defaults silently; an
    /// unreadable or broken file logs a warning and returns defaults; one bad
    /// value costs only that value.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "settings: {} is unreadable ({}) - using defaults",
                        path.display(),
                        e
                    );
                }
                return Self::default();
            }
        };
        // Decode with lossy conversion: a stray byte becomes a replacement
        // character, so the PIN survives an ANSI-saved file or UTF-16LE becomes
        // garbage, triggering the JSON error path with a warning.
        let text = String::from_utf8_lossy(&bytes);
        // Notepad's "UTF-8 with BOM" puts U+FEFF first; JSON does not allow it.
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => Self::from_value(&v),
            Err(e) => {
                tracing::warn!(
                    "settings: {} is not valid JSON ({}) - using defaults",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    fn from_value(v: &serde_json::Value) -> Self {
        Self {
            language: v
                .get("language")
                .and_then(|x| x.as_str())
                .filter(|code| Lang::from_code(code).is_some())
                .map(str::to_string),
            swap_zap_keys: v
                .get("swap_zap_keys")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            pin: v
                .get("pin")
                .and_then(|x| serde_json::from_value(x.clone()).ok()),
            news_rules: v
                .get("news_rules")
                .and_then(|x| serde_json::from_value(x.clone()).ok()),
        }
    }

    /// Write atomically: a temp file next to it, then a rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    pub fn lang(&self) -> Option<Lang> {
        self.language.as_deref().and_then(Lang::from_code)
    }

    /// The rules in force, borrowed: the saved ones, else the defaults.
    pub fn news_rules(&self) -> &[NewsRule] {
        match &self.news_rules {
            Some(rules) => rules,
            None => default_rules_static(),
        }
    }

    /// The same rules as an owned list, for whoever moves them elsewhere.
    pub fn news_rules_or_default(&self) -> Vec<NewsRule> {
        self.news_rules.clone().unwrap_or_else(default_rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_in_force_are_borrowed_the_saved_ones_else_the_defaults() {
        let mut s = Settings::default();
        assert_eq!(s.news_rules(), default_rules().as_slice());
        // Borrowed, not built per call: the same shared copy every time.
        assert!(std::ptr::eq(s.news_rules(), s.news_rules()));
        // The user's own choice, an empty list included, replaces the defaults.
        s.news_rules = Some(Vec::new());
        assert!(s.news_rules().is_empty());
        let mine = vec![NewsRule {
            channel: "CNN".into(),
            patterns: "*".into(),
            label: String::new(),
            skip_secs: 0,
        }];
        s.news_rules = Some(mine.clone());
        assert_eq!(s.news_rules(), mine.as_slice());
        let saved = s.news_rules.as_deref().unwrap_or_default();
        assert!(
            std::ptr::eq(s.news_rules(), saved),
            "no copy of the saved rules"
        );
    }
}
