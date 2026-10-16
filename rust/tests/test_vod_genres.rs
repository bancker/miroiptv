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
    assert_eq!(genres_in("Komoedie"), comedy);
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

#[test]
fn a_long_chain_of_prefixes_is_read_in_linear_time() {
    // Prefixes of labels leave a name by loops (dashes, groups in front) and by
    // the bar rules; a loop that looked through the rest of the name for each of
    // n prefixes would cost n squared, and a portal's name has no bound.
    use std::time::{Duration, Instant};
    let within = |name: &str| {
        let started = Instant::now();
        let p = parse_name(name, MAX_YEAR);
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(3),
            "{} bytes took {took:?}",
            name.len()
        );
        (p.title.to_string(), p.year, p.labels)
    };
    for unit in [
        "nl - ",
        "nl \u{2013} ",
        "nl \u{2014} ",
        "[nl]",
        "(nl) ",
        "nl | ",
        "nl \u{ff5c} ",
        "nl \u{2503} ",
        "nl \u{2502} ",
    ] {
        let name = unit.repeat(40_000) + "Frozen II (2019)";
        assert_eq!(
            within(&name),
            ("Frozen II".to_string(), 2019, Labels::NL),
            "{unit:?} x 40000"
        );
    }
    // Labels after bars: eight leave as labels, the rest by the prefix rule.
    for unit in [" | nl", " \u{ff5c} nl", " \u{2503} nl", " \u{2502} nl"] {
        let name = "Frozen II (2019)".to_string() + &unit.repeat(40_000);
        let (title, ..) = within(&name);
        assert!(!title.is_empty(), "{unit:?} x 40000");
    }
}

#[test]
fn the_genres_are_rulings_10_names_and_their_order_is_the_bit_order() {
    // The order is an on-disk format: the index cache keeps genre masks.
    let nl: Vec<&str> = Genre::ALL.iter().map(|g| t(Lang::Nl, g.text())).collect();
    assert_eq!(
        nl,
        [
            "Actie",
            "Avontuur",
            "Animatie",
            "Anime",
            "Biografie",
            "Kerst",
            "Komedie",
            "Misdaad",
            "Documentaire",
            "Drama",
            "Familie",
            "Fantasy",
            "Geschiedenis",
            "Horror",
            "Kinderen",
            "Muziek",
            "Mysterie",
            "Reality",
            "Romantiek",
            "Sciencefiction",
            "Sport",
            "Thriller",
            "Oorlog",
            "Western"
        ]
    );
    for (i, g) in Genre::ALL.iter().enumerate() {
        assert_eq!(g.bit(), 1 << i, "{g:?}");
    }
}

#[test]
fn aliases_are_whole_words_wherever_they_stand() {
    assert_eq!(genres_in("Reaction"), 0, "action ends a longer word");
    assert_eq!(genres_in("Warfare"), 0);
    assert_eq!(genres_in("Reaction | Action"), Genre::Action.bit());
    assert_eq!(genres_in("Stand-up"), Genre::Comedy.bit());
    assert_eq!(genres_in("Cabaret"), Genre::Comedy.bit());
}

#[test]
fn the_fold_table_is_pinned_beyond_e_ss_c_ae_o() {
    assert_eq!(
        fold("Señor Üner Ångström Œuvre Þor Łódź Ďábel Ěva Ğ Ş Ř"),
        "senor uner angstrom oeuvre thor lodz dabel eva g s r"
    );
}

#[test]
fn every_label_of_the_spec_is_read_from_a_name() {
    use bongo_player::vod::genres::Labels as L;
    for (suffix, label) in [
        ("4K", L::UHD),
        ("UHD", L::UHD),
        ("2160p", L::UHD),
        ("Ultra HD", L::UHD),
        ("1080p", L::FHD),
        ("FHD", L::FHD),
        ("Full HD", L::FHD),
        ("720p", L::HD),
        ("HD", L::HD),
        ("SD", L::SD),
        ("HDR", L::HDR),
        ("Dolby Vision", L::HDR),
        ("3D", L::THREE_D),
        ("NL", L::NL),
        ("Nederlands", L::NL),
        ("Dutch", L::NL),
        ("NL Gesproken", L::NL),
        ("Nederlands Gesproken", L::NL),
        ("NL SUBS", L::NL_SUBS),
        ("NL SUB", L::NL_SUBS),
        ("NLSUBS", L::NL_SUBS),
        ("MULTI", L::MULTI),
        ("DUAL", L::DUAL),
        ("EN", L::EN),
        ("English", L::EN),
        ("UK", L::EN),
    ] {
        assert_eq!(
            parsed(&format!("Film (2019) {suffix}")),
            ("Film", 2019, label),
            "{suffix}"
        );
    }
    // eleven distinct single bits
    let all = [
        L::UHD,
        L::FHD,
        L::HD,
        L::SD,
        L::HDR,
        L::THREE_D,
        L::NL,
        L::NL_SUBS,
        L::MULTI,
        L::DUAL,
        L::EN,
    ];
    let mut seen = 0u16;
    for l in all {
        assert_eq!(l.0.count_ones(), 1);
        assert_eq!(seen & l.0, 0, "{l:?} shares a bit");
        seen |= l.0;
    }
}

#[test]
fn near_misses_of_the_two_word_labels_stay_in_the_title() {
    let none = Labels::default();
    assert_eq!(parsed("Night Vision"), ("Night Vision", 0, none));
    assert_eq!(parsed("Film NL Zeep"), ("Film NL Zeep", 0, none));
    assert_eq!(parsed("Film Xx Gesproken"), ("Film Xx Gesproken", 0, none));
    assert_eq!(
        parsed("NL SUBS"),
        ("NL SUBS", 0, none),
        "a name of labels keeps its title"
    );
}

#[test]
fn the_year_has_bounds_and_the_first_group_wins() {
    let none = Labels::default();
    assert_eq!(parse_name("Film (2027)", MAX_YEAR).year, 2027);
    assert_eq!(parsed("Film (2028)"), ("Film (2028)", 0, none));
    assert_eq!(parse_name("Film (1900)", MAX_YEAR).year, 1900);
    assert_eq!(parse_name("Film (1899)", MAX_YEAR).year, 0);
    assert_eq!(parsed("Title (1999) (2019)"), ("Title", 1999, none));
    assert_eq!(parsed("Title 2019 (2020)"), ("Title 2019", 2020, none));
    assert_eq!(parsed("Title: (2019)"), ("Title", 2019, none));
}

#[test]
fn a_prefix_before_the_bar_gives_its_labels_whatever_else_it_holds() {
    assert_eq!(parsed("Netflix NL | Film"), ("Film", 0, Labels::NL));
}

#[test]
fn a_category_says_its_language_by_a_trailing_label_too() {
    use bongo_player::vod::genres::category_labels;
    assert_eq!(category_labels("Komedie [NL]").language(), Language::Dutch);
    assert_eq!(category_labels("Films (EN)").language(), Language::English);
}

#[test]
fn text_names_every_tier_and_the_rest() {
    assert_eq!(Labels::HD.text(), "720p");
    assert_eq!(Labels::SD.text(), "SD");
    assert_eq!(Labels::THREE_D.text(), "3D");
    assert_eq!(Labels::EN.text(), "EN");
    assert_eq!(Labels::MULTI.with(Labels::DUAL).text(), "MULTI DUAL");
}

#[test]
fn a_group_of_labels_in_front_is_a_prefix() {
    let nl = Labels::NL;
    let uhd = Labels::UHD;
    for (name, title, year, ls) in [
        ("[NL] Frozen II (2019)", "Frozen II", 2019, nl),
        (
            "[NL][4K] Frozen II (2019)",
            "Frozen II",
            2019,
            labels(&[nl, uhd]),
        ),
        ("[4K] [NL] Frozen II", "Frozen II", 0, labels(&[nl, uhd])),
        ("(NL) Frozen II (2019)", "Frozen II", 2019, nl),
        ("[NL] - Frozen II (2019)", "Frozen II", 2019, nl),
        // a group that is not labels only is part of the title
        ("[REC] (2007)", "[REC]", 2007, Labels::default()),
        ("[NL] [REC] (2007)", "[REC]", 2007, nl),
        (
            "(500) Days of Summer (2009)",
            "(500) Days of Summer",
            2009,
            Labels::default(),
        ),
        // a word that is a title stays one behind the group (Ruling 15)
        ("[NL] Dual (2022)", "Dual", 2022, nl),
        ("[NL] 1917", "1917", 0, nl),
        // nothing else: the group is the title
        ("[NL]", "[NL]", 0, nl),
    ] {
        assert_eq!(parsed(name), (title, year, ls), "{name}");
    }
}

#[test]
fn a_part_after_the_last_bar_that_is_labels_only_is_labels() {
    use bongo_player::vod::genres::category_labels;
    let nl = Labels::NL;
    let uhd = Labels::UHD;
    for (name, title, year, ls) in [
        (
            "Frozen II (2019) 4K | NL",
            "Frozen II",
            2019,
            labels(&[uhd, nl]),
        ),
        ("Frozen II | NL", "Frozen II", 0, nl),
        (
            "Frozen II (2019) | 4K | NL",
            "Frozen II",
            2019,
            labels(&[uhd, nl]),
        ),
        (
            "NL | Frozen II (2019) | 4K",
            "Frozen II",
            2019,
            labels(&[nl, uhd]),
        ),
        ("Film [4K | NL]", "Film", 0, labels(&[uhd, nl])),
        ("NL - 4K - Frozen II", "Frozen II", 0, labels(&[nl, uhd])),
        // a prefix is still a prefix
        ("NL | Frozen II (2019)", "Frozen II", 2019, nl),
        ("Netflix | Frozen II", "Frozen II", 0, Labels::default()),
        // labels before it: it is the title, even a word that is a label
        ("NL | Dual", "Dual", 0, nl),
        ("NL | 4K | Dual", "Dual", 0, labels(&[nl, uhd])),
    ] {
        assert_eq!(parsed(name), (title, year, ls), "{name}");
    }
    assert_eq!(category_labels("Films | NL").language(), Language::Dutch);
}

#[test]
fn dutch_film_compounds_and_inflections_name_their_genre() {
    use Genre::*;
    for (text, genre) in [
        ("Misdaadfilm", Crime),
        ("Misdaadfilms", Crime),
        ("Thrillerfilm", Thriller),
        ("Thrillerfilms", Thriller),
        ("Dramafilm", Drama),
        ("Dramafilms", Drama),
        ("Avonturenfilm", Adventure),
        ("Avonturenfilms", Adventure),
        ("Komediefilm", Comedy),
        ("Komediefilms", Comedy),
        ("Historisch", History),
        ("Historische films", History),
        ("Fantasie", Fantasy),
    ] {
        assert_eq!(genres_in(text), genre.bit(), "{text}");
    }
}

#[test]
fn a_bar_or_a_dash_of_any_spelling_separates_a_prefix() {
    // U+FF5C a fullwidth bar, U+2503 and U+2502 box-drawing bars, U+2013 an en
    // dash, U+2014 an em dash.
    let nl = Labels::NL;
    let nl_uhd = labels(&[Labels::NL, Labels::UHD]);
    for (name, ls) in [
        ("NL \u{2013} Frozen II (2019)", nl),
        ("NL \u{2014} Frozen II (2019)", nl),
        ("\u{2503}NL\u{2503} Frozen II (2019)", nl),
        ("NL \u{ff5c} Frozen II (2019)", nl),
        ("Frozen II (2019) \u{2502} NL", nl),
        // more than one, of mixed spellings
        ("NL \u{2013} 4K \u{2014} Frozen II (2019)", nl_uhd),
        ("NL - 4K \u{2013} Frozen II (2019)", nl_uhd),
        ("Frozen II (2019) 4K \u{ff5c} NL", nl_uhd),
        ("Frozen II (2019) \u{2503} 4K \u{2502} NL", nl_uhd),
        ("\u{2502}NL\u{2502}4K\u{2502} Frozen II (2019)", nl_uhd),
        // behind a group of labels in front, with or without spaces
        ("[NL]\u{2013}Frozen II (2019)", nl),
        ("[NL]\u{2014}Frozen II (2019)", nl),
        ("[NL] \u{2014} Frozen II (2019)", nl),
        ("[NL] \u{2503} Frozen II (2019)", nl),
    ] {
        assert_eq!(parsed(name), ("Frozen II", 2019, ls), "{name:?}");
    }
    // One at the end of a word leaves the title (the end trim).
    for bar in [
        "|", "\u{ff5c}", "\u{2503}", "\u{2502}", "\u{2013}", "\u{2014}",
    ] {
        let name = format!("Frozen II{bar}");
        assert_eq!(
            parsed(&name),
            ("Frozen II", 0, Labels::default()),
            "{name:?}"
        );
    }
    // A dash in the title is no separator when what is before it is no label.
    assert_eq!(
        parsed("Mission: Impossible \u{2013} Fallout (2018)"),
        (
            "Mission: Impossible \u{2013} Fallout",
            2018,
            Labels::default()
        )
    );
    assert_eq!(
        parsed("Mission: Impossible \u{2014} Fallout (2018)"),
        (
            "Mission: Impossible \u{2014} Fallout",
            2018,
            Labels::default()
        )
    );
}

#[test]
fn an_apostrophe_joins_its_word_instead_of_spacing_it() {
    // ' and ` (ASCII), U+2019 and U+2018 (the curly ones), U+02BC (a letter) and
    // U+00B4 (an accent): none of them is a space, none is a letter.
    for (text, folded) in [
        ("Ocean's Eleven", "oceans eleven"),
        ("Ocean`s Eleven", "oceans eleven"),
        ("Don\u{2019}t Look Up", "dont look up"),
        ("Don\u{2018}t", "dont"),
        ("Don\u{2bc}t", "dont"),
        ("Don\u{b4}t", "dont"),
        ("Schindler\u{2019}s List", "schindlers list"),
        ("'t Schaep met de 5 pooten", "t schaep met de 5 pooten"),
        ("rock ' n roll", "rock n roll"),
        ("A'", "a"),
        ("'", ""),
        ("'\u{2019}\u{2018}\u{2bc}\u{b4}`", ""),
        ("  '  ", ""),
    ] {
        assert_eq!(fold(text), folded, "{text:?}");
        assert_eq!(fold(folded), folded, "twice: {text:?}");
    }
}

#[test]
fn an_apostrophe_s_still_names_its_genre() {
    // The fold joins the s: "Marvel's" is "marvels" and "Children's" is
    // "childrens", so each has an alias of its own beside "marvel", "children".
    assert_eq!(genres_in("Marvel's"), Genre::Action.bit());
    assert_eq!(genres_in("Marvel\u{2019}s"), Genre::Action.bit());
    assert_eq!(genres_in("Children's Movies"), Genre::Kids.bit());
    assert_eq!(genres_in("Kids' Movies"), Genre::Kids.bit());
}

#[test]
fn a_number_in_brackets_in_a_categorys_name_is_a_label_not_a_year() {
    use bongo_player::vod::genres::category_labels;
    assert!(category_labels("Films (2160)").has(Labels::UHD));
    assert!(category_labels("Films [2160]").has(Labels::UHD));
}
