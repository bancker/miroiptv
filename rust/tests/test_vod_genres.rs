//! What names say (vod/genres.rs, spec 2026-10-15 §4.2): the fold the search
//! compares with, the 24 genres and their aliases, the language markers, the
//! labels of a version and the year of a name.

use bongo_player::i18n::{t, Lang};
use bongo_player::vod::fold;
use bongo_player::vod::genres::{genres_in, genres_of, parse_name, Genre, Labels, Language};

#[test]
fn fold_lowers_the_case_drops_accents_and_makes_punctuation_one_space() {
    assert_eq!(fold("Pokémon: De Film!"), "pokemon de film");
    assert_eq!(fold("  Amélie  "), "amelie");
    assert_eq!(fold("Straße"), "strasse");
    assert_eq!(fold("Ça va? Ærø"), "ca va aero");
    assert_eq!(fold("Poke\u{301}mon"), "pokemon", "a combining accent");
    assert_eq!(fold("Spider-Man: No Way Home"), "spider man no way home");
    assert_eq!(fold("Пётр"), "пётр", "another script stays, in lower case");
    assert_eq!(fold("--"), "");
}

#[test]
fn there_are_24_genres_each_named_in_every_language() {
    assert_eq!(Genre::ALL.len(), 24);
    let all = Genre::ALL.iter().fold(0u32, |mask, g| {
        assert_eq!(mask & g.bit(), 0, "{g:?} shares a bit");
        mask | g.bit()
    });
    assert_eq!(all.count_ones(), 24);
    for g in Genre::ALL {
        for lang in Lang::ALL {
            assert!(!t(lang, g.text()).is_empty(), "{g:?} in {lang:?}");
        }
    }
    assert_eq!(t(Lang::Nl, Genre::Comedy.text()), "Komedie");
    assert_eq!(t(Lang::De, Genre::Comedy.text()), "Komödie");
}

#[test]
fn comedy_komedie_and_komoedie_are_one_genre() {
    let comedy = Genre::Comedy.bit();
    assert_eq!(genres_in("Comedy"), comedy);
    assert_eq!(genres_in("Komedie"), comedy);
    assert_eq!(genres_in("Komödie"), comedy);
}

#[test]
fn every_alias_is_written_folded_and_names_its_genre() {
    for g in Genre::ALL {
        for alias in g.aliases() {
            assert_eq!(fold(alias), *alias, "{g:?}: write {alias:?} folded");
            assert_ne!(genres_in(alias) & g.bit(), 0, "{g:?}: {alias:?}");
        }
    }
}

#[test]
fn a_category_or_genre_field_names_its_genres_as_whole_words() {
    use Genre::*;
    let mask = |gs: &[Genre]| gs.iter().fold(0, |m, g| m | g.bit());
    assert_eq!(genres_in("NL | Komedie"), mask(&[Comedy]));
    assert_eq!(genres_in("Action & Adventure"), mask(&[Action, Adventure]));
    assert_eq!(
        genres_in("Sci-Fi & Fantasy"),
        mask(&[ScienceFiction, Fantasy])
    );
    assert_eq!(genres_in("Drama, Mystery"), mask(&[Drama, Mystery]));
    assert_eq!(genres_in("Kids & Familie"), mask(&[Kids, Family]));
    assert_eq!(genres_in("Horror/Thriller"), mask(&[Horror, Thriller]));
    assert_eq!(genres_in("Star Wars"), 0, "no war in wars");
    assert_eq!(genres_in("NL | Nieuw"), 0);
    assert_eq!(genres_in(""), 0);
    let named: Vec<Genre> = genres_of(mask(&[Western, Action])).collect();
    assert_eq!(named, [Action, Western], "in the fixed order");
}

/// The latest year a name may give in these tests: 2026's next.
const MAX_YEAR: u16 = 2027;

/// The title, year and labels `parse_name` reads from `name`.
fn parsed(name: &str) -> (&str, u16, Labels) {
    let p = parse_name(name, MAX_YEAR);
    (p.title, p.year, p.labels)
}

fn labels(ls: &[Labels]) -> Labels {
    ls.iter().fold(Labels::default(), |all, l| all.with(*l))
}

#[test]
fn a_name_gives_its_title_year_and_labels() {
    use bongo_player::vod::genres::Labels as L;
    let cases: &[(&str, &str, u16, Labels)] = &[
        (
            "NL | The Matrix (1999) [4K HDR]",
            "The Matrix",
            1999,
            labels(&[L::NL, L::UHD, L::HDR]),
        ),
        ("The Matrix 1999 1080p", "The Matrix", 1999, L::FHD),
        ("4K - The Matrix", "The Matrix", 0, L::UHD),
        (
            "Mission: Impossible - Fallout (2018)",
            "Mission: Impossible - Fallout",
            2018,
            L::default(),
        ),
        (
            "Toy Story 4 (2019) NL Gesproken",
            "Toy Story 4",
            2019,
            L::NL,
        ),
        ("The Matrix NL SUBS", "The Matrix", 0, L::NL_SUBS),
        (
            "Pokémon: De Film (2000) [NL]",
            "Pokémon: De Film",
            2000,
            L::NL,
        ),
        ("EN | Breaking Bad", "Breaking Bad", 0, L::EN),
        ("Frozen II 2019", "Frozen II", 2019, L::default()),
        ("The Movie (Extended) 4K", "The Movie (Extended)", 0, L::UHD),
        ("Title –", "Title", 0, L::default()),
    ];
    for (name, title, year, ls) in cases {
        assert_eq!(parsed(name), (*title, *year, *ls), "{name}");
    }
}

#[test]
fn a_word_that_is_a_title_stays_a_title() {
    assert_eq!(parsed("Dual (2022)"), ("Dual", 2022, Labels::default()));
    assert_eq!(parsed("1917"), ("1917", 0, Labels::default()));
    assert_eq!(parsed("1917 (2019)"), ("1917", 2019, Labels::default()));
    assert_eq!(
        parsed("Romeo en Julia"),
        ("Romeo en Julia", 0, Labels::default()),
        "en is Dutch for and"
    );
    assert_eq!(
        parsed("Blade Runner 2049"),
        ("Blade Runner 2049", 0, Labels::default()),
        "a year to come is no year"
    );
    assert_eq!(parsed(""), ("", 0, Labels::default()));
}

#[test]
fn labels_rank_as_channel_qualities_and_say_the_language() {
    assert_eq!(Labels::UHD.rank(), 4);
    assert_eq!(Labels::FHD.rank(), 3);
    assert_eq!(Labels::HD.rank(), 2);
    assert_eq!(
        Labels::default().rank(),
        1,
        "unmarked, as quality_rank has it"
    );
    assert_eq!(Labels::SD.rank(), 0);
    assert_eq!(labels(&[Labels::UHD, Labels::SD]).rank(), 4);
    assert_eq!(Labels::NL.language(), Language::Dutch);
    assert_eq!(Labels::EN.language(), Language::English);
    assert_eq!(
        labels(&[Labels::NL, Labels::EN]).language(),
        Language::Dutch,
        "Dutch first"
    );
    assert_eq!(
        Labels::NL_SUBS.language(),
        Language::Unknown,
        "Dutch subtitles, not Dutch speech"
    );
}

#[test]
fn labels_are_named_as_the_version_choice_shows_them() {
    assert_eq!(labels(&[Labels::UHD, Labels::HDR]).text(), "4K HDR");
    assert_eq!(
        labels(&[Labels::FHD, Labels::NL_SUBS]).text(),
        "1080p NL SUBS"
    );
    assert_eq!(Labels::NL.text(), "NL");
    assert_eq!(Labels::default().text(), "");
}

#[test]
fn odd_names_never_panic_and_keep_a_title() {
    // Portals send what they send; the release build aborts on a panic.
    for name in [
        "Amélie\u{a0}(2001)",
        "Film (2001",
        "[4K Film",
        "Title –",
        "NL |",
        "|",
        "4K",
        "(2001)",
        "🎬 Film 🎬 (2020)",
        "A\u{301}",
        "((()))",
        "NL | (2020) [4K]",
    ] {
        let p = parse_name(name, MAX_YEAR);
        assert!(!p.title.is_empty(), "{name:?}: {p:?}");
    }
    assert_eq!(
        parsed("Amélie\u{a0}(2001)"),
        ("Amélie", 2001, Labels::default()),
        "a no-break space"
    );
    assert_eq!(
        parsed("Film (2001"),
        ("Film", 2001, Labels::default()),
        "an unclosed bracket is punctuation"
    );
    assert_eq!(
        parsed("4K"),
        ("4K", 0, Labels::default()),
        "a name of one label is its title"
    );
    assert_eq!(parsed("  "), ("", 0, Labels::default()));
}

#[test]
fn a_category_named_by_labels_alone_says_its_language() {
    use bongo_player::vod::genres::category_labels;
    assert_eq!(category_labels("Nederlands").language(), Language::Dutch);
    assert_eq!(category_labels("NL Gesproken").language(), Language::Dutch);
    assert_eq!(category_labels("NL | Komedie").language(), Language::Dutch);
    assert_eq!(category_labels("English").language(), Language::English);
    // A section named after its language, without a `|`.
    assert_eq!(category_labels("NL FILMS").language(), Language::Dutch);
    assert_eq!(
        category_labels("NL Gesproken Kids").language(),
        Language::Dutch
    );
    assert_eq!(category_labels("UK Series").language(), Language::English);
    assert_eq!(
        category_labels("Films en Series").language(),
        Language::Unknown
    );
    assert_eq!(category_labels("Kids").language(), Language::Unknown);
}

#[test]
fn a_name_of_many_unclosed_brackets_is_read_in_linear_time() {
    // A bracket with no closer used to search the rest of the name for one, so
    // n of them cost n squared: 20,000 took seconds. A catalog reads ~58,000
    // names, and a portal's name has no bound.
    use std::time::{Duration, Instant};
    for unit in ["(", "[", "([", "(("] {
        let name = unit.repeat(40_000);
        let started = Instant::now();
        let p = parse_name(&name, MAX_YEAR);
        assert!(!p.title.is_empty(), "{unit:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{unit:?} x 40000 took {:?}",
            started.elapsed()
        );
    }
    // An opener without a closer stays a word of its own, and the other kind
    // of bracket after it still closes.
    assert_eq!(
        parsed("Film (2001 [4K]"),
        ("Film", 2001, Labels::UHD),
        "an unclosed ( before a closed ["
    );
    assert_eq!(
        parsed("Film [x (2001)"),
        ("Film [x", 2001, Labels::default()),
        "an unclosed [ before a closed ("
    );
}
