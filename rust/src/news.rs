//! Rules for the news picker (key N): which channel to look at, which titles
//! count as news there, the badge to show and how many seconds to skip at the
//! start (RTL's ads).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsRule {
    /// As the user types it ("NPO 1"); found by normalized name.
    pub channel: String,
    /// `;`-separated: `*` = any text, a leading `!` excludes. Empty = every
    /// programme.
    #[serde(default)]
    pub patterns: String,
    /// Badge text; empty = derived from the channel name.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub skip_secs: u32,
}

const NOS_PATTERNS: &str =
    "*journaal*; !*sport*; !*jeugd*; !*regio*; !*makkelijke taal*; !*gebaren*";

/// The rules 0.5.x had built in.
pub fn default_rules() -> Vec<NewsRule> {
    let rule = |channel: &str, patterns: &str, label: &str, skip_secs: u32| NewsRule {
        channel: channel.into(),
        patterns: patterns.into(),
        label: label.into(),
        skip_secs,
    };
    vec![
        rule("NPO 1", NOS_PATTERNS, "NOS", 0),
        rule("NPO 2", NOS_PATTERNS, "NOS", 0),
        rule("RTL 4", "*rtl nieuws*", "RTL", 30),
        rule("CNN", "*", "CNN", 0),
        rule("BBC News", "*", "BBC", 0),
        rule("VRT 1", "*journaal*", "VRT", 0),
        rule("Omroep Zeeland", "*nieuws*", "ZLD", 0),
    ]
}

/// Whether `title` counts as news: it matches at least one plain pattern (or
/// there is none) and no `!`-pattern. Case-insensitive.
pub fn title_matches(patterns: &str, title: &str) -> bool {
    let title = title.to_lowercase();
    let (mut any_plain, mut plain_hit) = (false, false);
    for p in patterns.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(excluded) = p.strip_prefix('!') {
            if glob(&excluded.trim().to_lowercase(), &title) {
                return false;
            }
        } else {
            any_plain = true;
            plain_hit |= glob(&p.to_lowercase(), &title);
        }
    }
    !any_plain || plain_hit
}

/// The badge text: the rule's label, else the channel name's first word.
pub fn rule_label(rule: &NewsRule) -> String {
    let label = rule.label.trim();
    if !label.is_empty() {
        return label.to_string();
    }
    rule.channel
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_uppercase()
}

/// `*`-only wildcard match over the whole text.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_with_stars_only() {
        assert!(glob("*journaal*", "nos journaal 20 uur"));
        assert!(glob("nos journaal*", "nos journaal"));
        assert!(!glob("nos journaal*", "het nos journaal"));
        assert!(glob("*", "anything"));
        assert!(glob("", ""));
        assert!(!glob("", "x"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }

    #[test]
    fn titles_match_a_plain_pattern_and_never_an_exclusion() {
        assert!(title_matches("NOS Journaal*", "NOS Journaal"));
        assert!(
            title_matches("nos journaal*", "NOS JOURNAAL 20.00 uur"),
            "case"
        );
        assert!(!title_matches(
            "NOS Journaal*; !*makkelijke taal*",
            "NOS Journaal in Makkelijke Taal"
        ));
        assert!(title_matches("", "whatever"), "empty = every programme");
        assert!(title_matches("*", "whatever"));
        assert!(
            title_matches("!*sport*", "Nieuws"),
            "only exclusions: the rest counts"
        );
        assert!(!title_matches("!*sport*", "Sport"));
        assert!(
            title_matches("*rtl nieuws*; *news*", "CNN News"),
            "any plain pattern"
        );
    }

    /// 0.5.x's hard-coded sources: (channel key, title keyword, exclusions).
    const OLD: &[(&str, &str, &[&str])] = &[
        (
            "npo1",
            "journaal",
            &["sport", "jeugd", "regio", "makkelijke taal", "gebaren"],
        ),
        (
            "npo2",
            "journaal",
            &["sport", "jeugd", "regio", "makkelijke taal", "gebaren"],
        ),
        ("rtl4", "rtl nieuws", &[]),
        ("cnn", "", &[]),
        ("bbcnews", "", &[]),
        ("vrt1", "journaal", &[]),
        ("omroepzeeland", "nieuws", &[]),
    ];

    #[test]
    fn the_default_rules_pick_exactly_what_0_5_picked() {
        let titles = [
            "NOS Journaal",
            "NOS Journaal 20.00 uur",
            "NOS Jeugdjournaal",
            "NOS Sportjournaal",
            "NOS Journaal in Makkelijke Taal",
            "NOS Journaal met Gebarentaal",
            "Regionaal Journaal",
            "RTL Nieuws",
            "RTL Nieuws 19:30",
            "RTL Late Night",
            "Het Journaal",
            "Zeeland Nieuws",
            "Omroep Zeeland Nieuws",
            "Tekst Televisie",
            "CNN Newsroom",
            "",
        ];
        let rules = default_rules();
        assert_eq!(rules.len(), OLD.len());
        for (rule, (key, kw, excl)) in rules.iter().zip(OLD) {
            assert_eq!(crate::catalog::normalize_channel_name(&rule.channel), *key);
            for t in titles {
                let l = t.to_lowercase();
                let old = (kw.is_empty() || l.contains(kw)) && !excl.iter().any(|x| l.contains(x));
                assert_eq!(
                    title_matches(&rule.patterns, t),
                    old,
                    "rule {} on {t:?}",
                    rule.channel
                );
            }
        }
        assert_eq!(rules[2].skip_secs, 30, "RTL skips its ads");
        assert_eq!(rule_label(&rules[0]), "NOS");
    }

    #[test]
    fn an_empty_label_comes_from_the_channel_name() {
        let r = NewsRule {
            channel: "Omroep Brabant".into(),
            patterns: String::new(),
            label: "  ".into(),
            skip_secs: 0,
        };
        assert_eq!(rule_label(&r), "OMROEP");
    }
}
