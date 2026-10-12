//! Rules for the news picker (key N): which channel to look at, which titles
//! count as news there, the badge to show and how many seconds to skip at the
//! start (RTL's ads).

use crate::catalog::normalize_channel_name;
use crate::portal::LiveChannel;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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

/// The default rules as one shared copy, for whoever only reads them.
pub fn default_rules_static() -> &'static [NewsRule] {
    static RULES: OnceLock<Vec<NewsRule>> = OnceLock::new();
    RULES.get_or_init(default_rules)
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

/// The first channel of one kind (catch-up or live) that a rule's name
/// matches exactly, and the first one that merely contains it.
#[derive(Default)]
struct Matches<'a> {
    exact: Option<&'a LiveChannel>,
    containing: Option<&'a LiveChannel>,
}

impl<'a> Matches<'a> {
    fn best(&self) -> Option<&'a LiveChannel> {
        self.exact.or(self.containing)
    }
}

/// Where each rule points, in the order of `rules`: the channel's stream id,
/// whether it is the catch-up twin, and its name. A rule takes its catch-up
/// twin when there is one (EPG and replays live there), else the live channel.
/// Within a kind, the exact normalized name wins over a name that contains it
/// ("CNN" finds "CNN International"), whatever their order in the catalog. A
/// rule whose name normalizes to nothing finds nothing.
///
/// One pass over the channels (a portal has some 13,000): every catalog name is
/// normalized once, not once per rule and per attempt.
pub fn resolve_channels<'a>(
    channels: impl IntoIterator<Item = &'a LiveChannel>,
    rules: &[NewsRule],
) -> Vec<Option<(i64, bool, String)>> {
    let keys: Vec<String> = rules
        .iter()
        .map(|r| normalize_channel_name(&r.channel))
        .collect();
    // Per rule: its matches among the catch-up channels, then among the live ones.
    let mut found: Vec<[Matches<'a>; 2]> = keys.iter().map(|_| Default::default()).collect();
    for channel in channels {
        let name = normalize_channel_name(&channel.name);
        let kind = usize::from(channel.tv_archive != 1);
        for (key, kinds) in keys.iter().zip(found.iter_mut()) {
            let slot = &mut kinds[kind];
            // An exact match settles the slot: nothing later can change it.
            if key.is_empty() || slot.exact.is_some() {
                continue;
            }
            if name == *key {
                slot.exact = Some(channel);
            } else if slot.containing.is_none() && name.contains(key.as_str()) {
                slot.containing = Some(channel);
            }
        }
    }
    found
        .iter()
        .map(|[archive, live]| {
            let pick = |c: &LiveChannel, is_archive| (c.stream_id, is_archive, c.name.clone());
            archive
                .best()
                .map(|c| pick(c, true))
                .or_else(|| live.best().map(|c| pick(c, false)))
        })
        .collect()
}

/// The channel one rule points at (see [`resolve_channels`]).
pub fn resolve_channel(live: &[LiveChannel], rule: &NewsRule) -> Option<(i64, bool, String)> {
    resolve_channels(live, std::slice::from_ref(rule))
        .pop()
        .flatten()
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

    fn lc(id: i64, name: &str, archive: i32) -> crate::portal::LiveChannel {
        crate::portal::LiveChannel {
            stream_id: id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: archive,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    fn rule(channel: &str) -> NewsRule {
        NewsRule {
            channel: channel.into(),
            patterns: "*".into(),
            label: String::new(),
            skip_secs: 0,
        }
    }

    #[test]
    fn a_rule_finds_the_catch_up_twin_first() {
        let live = vec![
            lc(1, "NL | NPO 1", 0),
            lc(2, "NL | NPO1 HD", 1),
            lc(3, "CNN International", 0),
        ];
        assert_eq!(
            resolve_channel(&live, &rule("NPO 1")),
            Some((2, true, "NL | NPO1 HD".into()))
        );
        assert_eq!(
            resolve_channel(&live, &rule("CNN")),
            Some((3, false, "CNN International".into()))
        );
        assert_eq!(resolve_channel(&live, &rule("RTL 4")), None);
        assert_eq!(resolve_channel(&live, &rule("  ")), None);
    }

    /// A catalog in which every way of matching has its trap.
    fn trap_catalog() -> Vec<crate::portal::LiveChannel> {
        vec![
            lc(1, "CNN International", 0), // contains "cnn", and comes before the exact one
            lc(2, "NL | CNN", 0),
            lc(3, "NL | NPO 1", 0),
            lc(4, "NL | NPO1 HD", 1), // the catch-up twin
            lc(5, "NL | RTL 4", 0),
            lc(6, "NL | RTL 4 Plus", 1), // catch-up, but only contains "rtl4"
            lc(7, "BBC News", 0),
            lc(8, "BBC News HD", 0), // the same channel again
        ]
    }

    #[test]
    fn an_exact_name_beats_a_containing_one_that_comes_first() {
        let live = trap_catalog();
        assert_eq!(
            resolve_channel(&live, &rule("CNN")),
            Some((2, false, "NL | CNN".into()))
        );
        // Without the exact one, the containing name is what is left.
        let without_exact: Vec<_> = live.iter().filter(|c| c.stream_id != 2).collect();
        assert_eq!(
            resolve_channels(without_exact, &[rule("CNN")]),
            [Some((1, false, "CNN International".into()))]
        );
    }

    #[test]
    fn a_containing_catch_up_name_beats_an_exact_live_one() {
        // Replays and the EPG live on the catch-up channel, so it wins even when the
        // live channel is the exact match.
        let live = trap_catalog();
        assert_eq!(
            resolve_channel(&live, &rule("RTL 4")),
            Some((6, true, "NL | RTL 4 Plus".into()))
        );
    }

    #[test]
    fn the_first_of_several_exact_names_wins() {
        let live = trap_catalog();
        assert_eq!(
            resolve_channel(&live, &rule("BBC News")),
            Some((7, false, "BBC News".into()))
        );
    }

    #[test]
    fn a_rule_that_normalizes_to_nothing_finds_nothing() {
        // Not even the first channel of a catalog whose names all "contain" nothing.
        let live = trap_catalog();
        for channel in ["", "  ", "NL | ", "|"] {
            assert_eq!(resolve_channel(&live, &rule(channel)), None, "{channel:?}");
        }
    }

    /// The first version of `resolve_channel`, kept as the oracle for the batch:
    /// one rule, up to four scans of the list, every name normalized again for
    /// each scan. Slow, and simple enough to be right.
    fn naive_resolve(
        live: &[crate::portal::LiveChannel],
        rule: &NewsRule,
    ) -> Option<(i64, bool, String)> {
        use crate::catalog::normalize_channel_name;
        let key = normalize_channel_name(&rule.channel);
        if key.is_empty() {
            return None;
        }
        let pick = |archive: bool| {
            let kind = |c: &&crate::portal::LiveChannel| (c.tv_archive == 1) == archive;
            live.iter()
                .filter(kind)
                .find(|c| normalize_channel_name(&c.name) == key)
                .or_else(|| {
                    live.iter()
                        .filter(kind)
                        .find(|c| normalize_channel_name(&c.name).contains(&key))
                })
                .map(|c| (c.stream_id, archive, c.name.clone()))
        };
        pick(true).or_else(|| pick(false))
    }

    #[test]
    fn one_pass_over_several_rules_gives_what_the_original_gives_for_each() {
        let live = trap_catalog();
        let rules = vec![
            rule("CNN"),
            rule("NPO 1"),
            rule("RTL 4"),
            rule("bbc news"),
            rule("RTL Z"), // in no channel's name
            rule("  "),
            rule("NPO1 HD"), // names the same channel as "NPO 1"
            rule("NPO 1"),   // the very same rule twice
        ];
        let batch = resolve_channels(&live, &rules);
        assert_eq!(batch.len(), rules.len());
        for (rule, found) in rules.iter().zip(&batch) {
            let original = naive_resolve(&live, rule);
            assert_eq!(found, &original, "{:?}", rule.channel);
            assert_eq!(resolve_channel(&live, rule), original, "{:?}", rule.channel);
        }
        // The cases above are not all the same answer.
        assert_eq!(batch[1], Some((4, true, "NL | NPO1 HD".into())));
        assert_eq!(batch[1], batch[6], "two rules may name one channel");
        assert_eq!(batch[1], batch[7]);
        assert_eq!(batch[4], None);
        assert_eq!(batch[5], None);
    }

    /// A small deterministic dice (xorshift64): the same catalog on every run.
    struct Dice(u64);

    impl Dice {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Channels from a small vocabulary, so that names repeat, contain one
    /// another and differ only in kind or in quality marker.
    fn generated_catalog(dice: &mut Dice, size: i64) -> Vec<crate::portal::LiveChannel> {
        const COUNTRIES: [&str; 3] = ["", "NL | ", "BE | "];
        const WORDS: [&str; 8] = ["NPO", "RTL", "CNN", "BBC", "News", "1", "4", "Plus"];
        const MARKERS: [&str; 6] = ["", "", " HD", " FHD", " UHD", " SD"];
        (0..size)
            .map(|id| {
                let mut name = COUNTRIES[dice.below(COUNTRIES.len())].to_string();
                for word in 0..=dice.below(3) {
                    if word > 0 && dice.below(2) == 0 {
                        name.push(' ');
                    }
                    name.push_str(WORDS[dice.below(WORDS.len())]);
                }
                name.push_str(MARKERS[dice.below(MARKERS.len())]);
                // 0 live, 1 catch-up, 2 neither: that counts as live.
                lc(id, &name, dice.below(3) as i32)
            })
            .collect()
    }

    #[test]
    fn the_batch_agrees_with_the_original_on_a_generated_catalog() {
        use crate::catalog::normalize_channel_name;
        let mut dice = Dice(0x9E37_79B9_7F4A_7C15);
        let live = generated_catalog(&mut dice, 150);
        let mut rules = vec![rule(""), rule("  "), rule("no such channel")];
        for _ in 0..100 {
            let name = live[dice.below(live.len())].name.clone();
            // A channel's own name, and a stretch of it: exact and containing matches.
            let key = normalize_channel_name(&name);
            let from = dice.below(key.len());
            let to = from + 1 + dice.below(key.len() - from);
            rules.push(rule(&key[from..to]));
            rules.push(rule(&name));
        }
        let batch = resolve_channels(&live, &rules);
        for (rule, found) in rules.iter().zip(&batch) {
            assert_eq!(found, &naive_resolve(&live, rule), "{:?}", rule.channel);
        }
        // Not a comparison of nothings: most rules find a channel, of both kinds.
        let found = |archive: bool| {
            batch
                .iter()
                .flatten()
                .filter(|(_, is_archive, _)| *is_archive == archive)
                .count()
        };
        assert!(
            found(true) > 20 && found(false) > 20,
            "{} / {}",
            found(true),
            found(false)
        );
    }

    #[test]
    fn no_rules_or_no_channels_resolve_to_nothing() {
        assert!(resolve_channels(&trap_catalog(), &[]).is_empty());
        assert_eq!(resolve_channels(&[], &[rule("CNN")]), [None]);
    }

    #[test]
    fn the_default_rules_are_one_shared_copy() {
        assert_eq!(default_rules_static(), default_rules().as_slice());
        assert!(std::ptr::eq(default_rules_static(), default_rules_static()));
    }
}
