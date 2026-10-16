//! What names say (vod/genres.rs, spec 2026-10-15 §4.2): the fold the search
//! compares with, and the 24 genres and their aliases.

use bongo_player::i18n::{t, Lang};
use bongo_player::vod::fold;
use bongo_player::vod::genres::{genres_in, genres_of, Genre};

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
