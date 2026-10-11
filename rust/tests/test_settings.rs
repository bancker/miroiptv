use bongo_player::i18n::Lang;
use bongo_player::news::{default_rules, NewsRule};
use bongo_player::pin::PinHash;
use bongo_player::settings::Settings;

fn temp() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    (dir, path)
}

#[test]
fn saves_and_loads_every_field() {
    let (_d, p) = temp();
    let s = Settings {
        language: Some("de".into()),
        swap_zap_keys: true,
        pin: Some(PinHash::with_salt("1234", [3; 16]).unwrap()),
        news_rules: Some(vec![NewsRule {
            channel: "CNN".into(),
            patterns: "*".into(),
            label: "CNN".into(),
            skip_secs: 0,
        }]),
    };
    s.save(&p).unwrap();
    let back = Settings::load(&p);
    assert_eq!(back, s);
    assert_eq!(back.lang(), Some(Lang::De));
    assert!(
        !p.with_extension("json.tmp").exists(),
        "no temp file left behind"
    );
}

#[test]
fn a_missing_file_gives_the_defaults() {
    let (_d, p) = temp();
    let s = Settings::load(&p);
    assert_eq!(s, Settings::default());
    assert_eq!(s.lang(), None, "first start: the language screen shows");
    assert_eq!(s.news_rules_or_default(), default_rules());
}

#[test]
fn a_file_saved_with_a_bom_still_loads() {
    // Notepad's "UTF-8 with BOM" must not cost the language and the PIN.
    let (_d, p) = temp();
    std::fs::write(&p, "\u{feff}{\"language\":\"en\",\"swap_zap_keys\":true}").unwrap();
    let s = Settings::load(&p);
    assert_eq!(s.lang(), Some(Lang::En));
    assert!(s.swap_zap_keys);
}

#[test]
fn a_broken_file_gives_the_defaults_without_panicking() {
    let (_d, p) = temp();
    std::fs::write(&p, "{ this is not json").unwrap();
    assert_eq!(Settings::load(&p), Settings::default());
}

#[test]
fn one_bad_field_costs_only_that_field() {
    let (_d, p) = temp();
    let pin = PinHash::with_salt("1234", [5; 16]).unwrap();
    let json = serde_json::json!({
        "language": "fr",
        "swap_zap_keys": "yes please",
        "pin": pin,
        "news_rules": [{"label": "a rule without a channel"}],
        "unknown": 1
    });
    std::fs::write(&p, json.to_string()).unwrap();
    let s = Settings::load(&p);
    assert_eq!(s.language, None);
    assert!(!s.swap_zap_keys);
    assert_eq!(s.pin, Some(pin), "the PIN survives a hand edit elsewhere");
    assert_eq!(s.news_rules, None);
}

#[test]
fn no_news_rules_and_an_empty_list_differ() {
    let s = Settings {
        news_rules: Some(Vec::new()),
        ..Settings::default()
    };
    assert!(
        s.news_rules_or_default().is_empty(),
        "the user removed every rule"
    );
}
