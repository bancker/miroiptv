//! Films merged into titles with versions (vod/versions.rs, spec 2026-10-15
//! §4.2): one key per name and year, the highest quality the default.

use bongo_player::portal::Movie;
use bongo_player::vod::genres::Labels;
use bongo_player::vod::versions::{merge, FilmTitle};

fn film(stream_id: i64, name: &str) -> Movie {
    Movie {
        stream_id,
        name: name.into(),
        ..Movie::default()
    }
}

/// Each title's text, year and the stream ids of its versions, default first.
fn shape<'a>(movies: &'a [Movie], titles: &[FilmTitle<'a>]) -> Vec<(&'a str, u16, Vec<i64>)> {
    titles
        .iter()
        .map(|t| {
            let ids = t
                .versions
                .iter()
                .map(|v| movies[v.index as usize].stream_id)
                .collect();
            (t.title, t.year, ids)
        })
        .collect()
}

#[test]
fn the_versions_of_a_film_are_one_title_with_the_highest_quality_first() {
    let movies = [
        film(1, "The Matrix (1999) 1080p"),
        film(2, "NL | The Matrix (1999) [4K]"),
        film(3, "The Matrix (1999) NL SUBS"),
        film(4, "The Matrix Reloaded (2003)"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(
        shape(&movies, &titles),
        [
            ("The Matrix", 1999, vec![2, 1, 3]),
            ("The Matrix Reloaded", 2003, vec![4])
        ]
    );
    assert_eq!(titles[0].key, "the matrix");
    let first = titles[0].versions[0].labels;
    assert!(first.has(Labels::UHD) && first.has(Labels::NL));
    assert!(titles[0].versions[2].labels.has(Labels::NL_SUBS));
}

#[test]
fn another_year_is_another_title_and_no_year_is_one_too() {
    let movies = [
        film(1, "Dune (1984)"),
        film(2, "Dune (2021)"),
        film(3, "Dune"),
    ];
    let titles = merge(&movies, 2027);
    let years: Vec<u16> = titles.iter().map(|t| t.year).collect();
    assert_eq!(years, [1984, 2021, 0]);
}

#[test]
fn a_tie_in_quality_goes_to_hdr_then_to_the_lowest_stream_id() {
    let movies = [
        film(9, "X (2020) 4K"),
        film(5, "X (2020) 4K"),
        film(7, "X (2020) 4K HDR"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(shape(&movies, &titles), [("X", 2020, vec![7, 5, 9])]);
}

#[test]
fn names_that_differ_in_case_accents_or_punctuation_are_one_title() {
    let movies = [
        film(1, "Amélie (2001)"),
        film(2, "AMELIE (2001) HD"),
        film(3, "Spider-Man (2002)"),
        film(4, "Spider Man (2002)"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(
        shape(&movies, &titles),
        [
            ("AMELIE", 2001, vec![2, 1]),
            ("Spider-Man", 2002, vec![3, 4])
        ],
        "the title shown is the default version's, as its name has it"
    );
}

#[test]
fn titles_keep_the_order_of_their_first_film() {
    let movies = [
        film(1, "B (2001)"),
        film(2, "A (2001)"),
        film(3, "B (2001) 4K"),
    ];
    let titles = merge(&movies, 2027);
    let keys: Vec<&str> = titles.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["b", "a"]);
}

#[test]
fn no_films_no_titles() {
    assert!(merge(&[], 2027).is_empty());
}
