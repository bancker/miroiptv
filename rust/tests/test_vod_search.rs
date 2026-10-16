//! The title index's queries (vod/index.rs, spec 2026-10-15 §3.3): search by
//! title, the filters with their counts, the four sorts, and "meer zoals dit".
//! The first ten tests are the plan's; after them come the text of a query (the
//! fold: an apostrophe, a bar, a dash), the edges of the filters and their
//! counts, the ties of the sorts, "meer zoals dit", and two tests that hold the
//! queries against a plain reading of the rules.

use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::{
    Counts, Learned, Query, Results, Sort, Spoken, Stars, TitleIndex, Years,
};
use bongo_player::vod::{fold, Kind, TitleId};
use std::cmp::Reverse;
use std::collections::HashSet;

const MAX_YEAR: u16 = 2027;
const DAY: i64 = 86_400;
/// Unix day 20,000 (10 October 2024), in seconds.
const DAY_20000: i64 = 20_000 * DAY;

/// A series with this name, genre field, release year and rating.
fn show(id: i64, name: &str, genre: &str, year: u16, rating: u8) -> Series {
    Series {
        series_id: id,
        name: name.into(),
        genre: Some(genre.into()),
        release_date_camel: (year > 0).then(|| format!("{year}-01-01")),
        rating,
        ..Series::default()
    }
}

/// A film in category `category` (see `categories`).
fn film(id: i64, name: &str, category: i64, rating: u8) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        category_id: category,
        rating,
        ..Movie::default()
    }
}

fn categories() -> Categories {
    Categories {
        vod: vec![
            Category {
                category_id: 1,
                category_name: "NL | Komedie".into(),
            },
            Category {
                category_id: 2,
                category_name: "EN | Drama".into(),
            },
        ],
        series: Vec::new(),
    }
}

fn index(movies: &[Movie], shows: &[Series]) -> TitleIndex {
    TitleIndex::build(movies, shows, &categories(), MAX_YEAR)
}

/// The names of a query's titles, in its order.
fn names(index: &TitleIndex, query: &Query) -> Vec<String> {
    index
        .search(query, false)
        .titles
        .iter()
        .map(|&p| index.title(p as usize).unwrap().name.to_owned())
        .collect()
}

fn text(t: &str) -> Query {
    Query {
        text: t.into(),
        ..Query::default()
    }
}

fn title_id(kind: Kind, id: i64) -> TitleId {
    TitleId { kind, id }
}

fn category(id: i64, name: &str) -> Category {
    Category {
        category_id: id,
        category_name: name.into(),
    }
}

#[test]
fn every_word_must_be_in_the_title_whatever_its_case_or_accents() {
    let idx = index(
        &[],
        &[
            show(1, "Pokémon: De Film", "Kids", 2000, 60),
            show(2, "Pokemon Detective Pikachu", "Kids", 2019, 66),
            show(3, "The Matrix", "Action", 1999, 82),
        ],
    );
    assert_eq!(
        names(&idx, &text("pokemon")),
        ["Pokemon Detective Pikachu", "Pokémon: De Film"]
    );
    assert_eq!(names(&idx, &text("POKÉMON film")), ["Pokémon: De Film"]);
    assert_eq!(
        names(&idx, &text("matrix the")),
        ["The Matrix"],
        "in any order"
    );
    assert!(names(&idx, &text("matrix reloaded")).is_empty());
}

#[test]
fn a_title_that_starts_with_the_query_comes_first_then_the_best_rated() {
    let idx = index(
        &[],
        &[
            show(1, "Batman Begins", "Action", 2005, 70),
            show(2, "The Batman", "Action", 2022, 80),
            show(3, "Batman Returns", "Action", 1992, 75),
        ],
    );
    assert_eq!(
        names(&idx, &text("batman")),
        ["Batman Returns", "Batman Begins", "The Batman"]
    );
}

#[test]
fn the_four_sorts_order_as_the_page_names_them() {
    let shows = [
        Series {
            last_modified: DAY_20000 + 2 * DAY,
            ..show(1, "Beta", "Drama", 2010, 70)
        },
        Series {
            last_modified: DAY_20000 + 5 * DAY,
            ..show(2, "Alpha", "Drama", 2011, 0)
        },
        Series {
            last_modified: DAY_20000 + DAY,
            ..show(3, "Ähnlich", "Drama", 2012, 70)
        },
        show(4, "Gamma", "Drama", 2013, 90),
    ];
    let idx = index(&[], &shows);
    let sorted = |sort| {
        names(
            &idx,
            &Query {
                sort,
                ..Query::default()
            },
        )
    };
    // ★ high to low, none last; on a tie the newest.
    assert_eq!(sorted(Sort::Rating), ["Gamma", "Beta", "Ähnlich", "Alpha"]);
    // Newest first; an unknown day last.
    assert_eq!(sorted(Sort::Newest), ["Alpha", "Beta", "Ähnlich", "Gamma"]);
    // A to Z without accents: "ahnlich" before "alpha".
    assert_eq!(sorted(Sort::Title), ["Ähnlich", "Alpha", "Beta", "Gamma"]);
    // No text: everything starts with it, so ★.
    assert_eq!(
        sorted(Sort::Relevance),
        ["Gamma", "Ähnlich", "Beta", "Alpha"]
    );
}

#[test]
fn values_of_one_group_add_up_and_the_groups_narrow_each_other() {
    let movies = [
        film(10, "Komedie Film (2021)", 1, 72),
        film(11, "Drama Film (2015)", 2, 85),
    ];
    let shows = [
        show(1, "Komedie Serie", "Comedy", 2021, 81),
        show(2, "Drama Serie", "Drama", 2003, 65),
        show(3, "Horror Serie", "Horror", 1985, 90),
    ];
    let idx = index(&movies, &shows);
    let q = |f: &dyn Fn(&mut Query)| {
        let mut query = Query {
            sort: Sort::Title,
            ..Query::default()
        };
        f(&mut query);
        names(&idx, &query)
    };
    let comedy_or_drama = Genre::Comedy.bit() | Genre::Drama.bit();
    assert_eq!(
        q(&|x| x.genres = comedy_or_drama),
        ["Drama Film", "Drama Serie", "Komedie Film", "Komedie Serie"]
    );
    assert_eq!(
        q(&|x| {
            x.genres = comedy_or_drama;
            x.kind = Some(Kind::Film);
        }),
        ["Drama Film", "Komedie Film"]
    );
    assert_eq!(
        q(&|x| x.years = Years::Since2020.bit() | Years::Before2000.bit()),
        ["Horror Serie", "Komedie Film", "Komedie Serie"]
    );
    assert_eq!(
        q(&|x| x.stars = Stars::EightUp.bit()),
        ["Drama Film", "Horror Serie", "Komedie Serie"]
    );
    assert_eq!(q(&|x| x.spoken = Spoken::Dutch.bit()), ["Komedie Film"]);
    assert_eq!(
        q(&|x| x.spoken = Spoken::Other.bit()),
        ["Drama Serie", "Horror Serie", "Komedie Serie"],
        "neither Dutch nor English: unknown included"
    );
}

#[test]
fn the_counts_show_what_each_value_would_give_with_the_other_filters() {
    let movies = [
        film(10, "Komedie Film (2021)", 1, 72),
        film(11, "Drama Film (2015)", 2, 85),
    ];
    let shows = [
        show(1, "Komedie Serie", "Comedy", 2021, 81),
        show(2, "Drama Serie", "Drama", 2003, 65),
        show(3, "Horror Serie", "Horror", 1985, 90),
    ];
    let idx = index(&movies, &shows);
    let query = Query {
        text: "i".into(),
        kind: Some(Kind::Film),
        genres: Genre::Comedy.bit(),
        ..Query::default()
    };
    let results = idx.search(&query, false);
    assert_eq!(results.titles.len(), 1, "the comedy film");
    let c = &results.counts;
    // Soort: with the genre filter, not the kind's own.
    assert_eq!((c.all, c.series, c.films), (2, 1, 1));
    // Genre: films (the kind filter), any genre (not the genre's own).
    let genre = |g: Genre| c.genres[Genre::ALL.iter().position(|x| *x == g).unwrap()];
    assert_eq!(
        (
            genre(Genre::Comedy),
            genre(Genre::Drama),
            genre(Genre::Horror)
        ),
        (1, 1, 0)
    );
    // Jaar, Waardering, Taal: the comedy films.
    assert_eq!(c.years, [1, 0, 0, 0]);
    assert_eq!(c.stars, [0, 1]);
    assert_eq!(c.spoken, [1, 0, 0]);
}

#[test]
fn an_unknown_year_is_in_no_year_bucket() {
    let idx = index(
        &[],
        &[
            show(1, "Old", "Drama", 1950, 0),
            show(2, "Unknown", "Drama", 0, 0),
        ],
    );
    let results = idx.search(&Query::default(), false);
    assert_eq!(results.counts.years, [0, 0, 0, 1]);
    let before = Query {
        years: Years::Before2000.bit(),
        ..Query::default()
    };
    assert_eq!(names(&idx, &before), ["Old"]);
    assert_eq!(Years::of(2020), Some(Years::Since2020));
    assert_eq!(Years::of(2019), Some(Years::The2010s));
    assert_eq!(Years::of(2000), Some(Years::The2000s));
    assert_eq!(Years::of(1999), Some(Years::Before2000));
    assert_eq!(Years::of(0), None);
}

#[test]
fn adult_titles_are_found_and_counted_only_while_unlocked() {
    let shows = [
        show(1, "Night Show", "Drama", 2020, 70),
        Series {
            is_adult: true,
            ..show(2, "Night Club", "Drama", 2020, 70)
        },
    ];
    let idx = index(&[], &shows);
    let q = text("night");
    let locked = idx.search(&q, false);
    assert_eq!(locked.titles.len(), 1);
    assert_eq!(locked.counts.all, 1, "nor counted");
    let unlocked = idx.search(&q, true);
    assert_eq!(unlocked.titles.len(), 2);
    assert_eq!(unlocked.counts.all, 2);
}

#[test]
fn more_like_this_shares_a_genre_or_the_category_best_rated_first() {
    let movies = [
        film(10, "Seed (2020)", 1, 70),
        film(11, "Same Category (2019)", 1, 60),
        film(12, "Drama Film (2018)", 2, 99),
    ];
    let shows = [
        show(1, "Comedy Show", "Comedy", 2019, 80),
        show(2, "Seen Comedy", "Comedy", 2018, 95),
        show(3, "Disliked Comedy", "Comedy", 2018, 90),
        show(4, "Unrated Comedy", "Comedy", 2018, 0),
        Series {
            is_adult: true,
            ..show(5, "Adult Comedy", "Comedy", 2018, 99)
        },
    ];
    let idx = index(&movies, &shows);
    let seed = idx
        .find(TitleId {
            kind: Kind::Film,
            id: 10,
        })
        .unwrap();
    let watched: HashSet<TitleId> = [TitleId {
        kind: Kind::Series,
        id: 2,
    }]
    .into();
    let disliked: HashSet<TitleId> = [TitleId {
        kind: Kind::Series,
        id: 3,
    }]
    .into();
    let like: Vec<&str> = idx
        .more_like(seed, 6, false, &watched, &disliked)
        .iter()
        .map(|&p| idx.title(p as usize).unwrap().name)
        .collect();
    assert_eq!(like, ["Comedy Show", "Same Category", "Unrated Comedy"]);
    let top: Vec<u32> = idx.more_like(seed, 1, false, &watched, &disliked);
    assert_eq!(top.len(), 1);
    assert_eq!(idx.title(top[0] as usize).unwrap().name, "Comedy Show");
    assert!(idx
        .more_like(seed, 0, false, &watched, &disliked)
        .is_empty());
    let unlocked = idx.more_like(seed, 1, true, &watched, &disliked);
    assert_eq!(
        idx.title(unlocked[0] as usize).unwrap().name,
        "Adult Comedy"
    );
}

#[test]
fn a_title_without_genre_or_category_is_like_nothing() {
    let idx = index(&[film(1, "Lonely", 0, 50), film(2, "Other", 0, 50)], &[]);
    let none = HashSet::new();
    assert!(idx.more_like(0, 6, false, &none, &none).is_empty());
    assert!(
        idx.more_like(99, 6, false, &none, &none).is_empty(),
        "no such title"
    );
}

#[test]
fn a_query_without_letters_or_digits_is_no_text_filter() {
    let idx = index(
        &[],
        &[
            show(1, "Alpha", "Drama", 2001, 50),
            show(2, "Beta", "Drama", 2002, 60),
        ],
    );
    for q in ["", " ", "!!!", " - ", "★"] {
        assert_eq!(idx.search(&text(q), false).titles.len(), 2, "{q:?}");
    }
}

// ---- the text of a query: the search folds it as the index folds the names ----

#[test]
fn the_query_is_folded_as_the_names_are_so_an_apostrophe_is_no_gap() {
    let idx = index(
        &[film(10, "Ocean’s Twelve (2004)", 0, 70)],
        &[
            show(1, "Ocean's Eleven", "Crime", 2001, 75),
            show(2, "Oceans Thirteen", "Crime", 2007, 65),
            show(3, "Ocean Waves", "Drama", 2010, 50),
        ],
    );
    // "Ocean's" is "oceans": one title, however the apostrophe is typed.
    for typed in [
        "ocean's eleven",
        "oceans eleven",
        "Ocean’s Eleven",
        "OCEAN`S ELEVEN",
        "ocean´s eleven",
        "ocean‘s eleven",
        "ocean\u{2bc}s eleven",
    ] {
        assert_eq!(names(&idx, &text(typed)), ["Ocean's Eleven"], "{typed:?}");
    }
    // It is no space either: "ocean s" is in "Ocean Waves", "oceans" is not.
    assert_eq!(
        names(&idx, &text("ocean's")),
        ["Ocean's Eleven", "Ocean’s Twelve", "Oceans Thirteen"]
    );
    assert_eq!(names(&idx, &text("oceans")), names(&idx, &text("ocean's")));
    // A film's key comes from the merge, and holds no apostrophe either.
    assert_eq!(names(&idx, &text("ocean's twelve")), ["Ocean’s Twelve"]);
    assert_eq!(names(&idx, &text("oceans twelve")), ["Ocean’s Twelve"]);
    // Typed key by key, the title stays found.
    let typed = "Ocean's Eleven";
    for end in 1..=typed.len() {
        let found = names(&idx, &text(&typed[..end]));
        assert!(
            found.iter().any(|n| n == typed),
            "{:?} lost it: {found:?}",
            &typed[..end]
        );
    }
}

#[test]
fn a_bar_a_dash_or_a_space_between_the_words_of_a_query_is_one_gap() {
    let idx = index(
        &[],
        &[
            show(1, "The Dark Knight", "Action", 2008, 90),
            show(2, "Dark City", "Fantasy", 1998, 77),
            show(3, "Knight Rider", "Action", 1982, 70),
        ],
    );
    for gap in [
        " ", "  ", "|", " | ", " ｜ ", " ┃ ", " │ ", "-", " - ", " – ", " — ", " ' ", " | - | ",
    ] {
        let typed = format!("dark{gap}knight");
        assert_eq!(names(&idx, &text(&typed)), ["The Dark Knight"], "{typed:?}");
    }
    // At either end of a query a gap is nothing at all.
    for typed in [
        "| dark knight",
        "dark knight |",
        " - dark knight - ",
        "' dark knight '",
        "｜dark│knight┃",
    ] {
        assert_eq!(names(&idx, &text(typed)), ["The Dark Knight"], "{typed:?}");
    }
    assert_eq!(
        names(&idx, &text("dark │")),
        ["Dark City", "The Dark Knight"]
    );
}

#[test]
fn a_query_of_only_apostrophes_bars_or_dashes_is_no_text_filter() {
    let idx = index(
        &[film(10, "Beta (2001)", 1, 50)],
        &[
            show(1, "Alpha", "Drama", 2001, 50),
            show(2, "Gamma's Law", "Drama", 2002, 60),
        ],
    );
    let all = idx.search(&Query::default(), false);
    assert_eq!(all.titles.len(), 3);
    // The same titles, in the same order, with the same counts: what an empty
    // query shows.
    for typed in [
        "'",
        "''",
        "' '",
        "’",
        "‘’",
        "`",
        "´",
        "\u{2bc}",
        "'`’‘´\u{2bc}",
        "|",
        "||",
        "｜",
        "┃",
        "│",
        "| ｜ ┃ │",
        "-",
        "–",
        "—",
        "- – —",
        " ' | - ' ",
        "\u{301}",
        "\u{200b}",
    ] {
        assert_eq!(idx.search(&text(typed), false), all, "{typed:?}");
    }
    // An apostrophe beside a word is not the word.
    assert_eq!(names(&idx, &text("' gamma")), ["Gamma's Law"]);
}

// ---- the filters and their counts ----

#[test]
fn the_filter_values_are_single_bits_in_the_order_of_their_counts() {
    assert_eq!(Years::ALL.map(Years::bit), [1, 2, 4, 8]);
    assert_eq!(Stars::ALL.map(Stars::bit), [1, 2]);
    assert_eq!(Spoken::ALL.map(Spoken::bit), [1, 2, 4]);
    assert_eq!(Stars::ALL.map(Stars::floor), [80, 70]);
    assert_eq!(Sort::default(), Sort::Relevance);
    assert_eq!(Query::default().sort, Sort::Relevance);
    assert_eq!(Genre::ALL.len(), Counts::default().genres.len());
}

#[test]
fn the_year_buckets_break_where_the_page_says() {
    let bucket = |year| Years::of(year);
    assert_eq!(bucket(0), None);
    for (year, want) in [
        (1, Years::Before2000),
        (1900, Years::Before2000),
        (1999, Years::Before2000),
        (2000, Years::The2000s),
        (2009, Years::The2000s),
        (2010, Years::The2010s),
        (2019, Years::The2010s),
        (2020, Years::Since2020),
        (2027, Years::Since2020),
        (u16::MAX, Years::Since2020),
    ] {
        assert_eq!(bucket(year), Some(want), "{year}");
    }
}

#[test]
fn stars_8_and_7_overlap_and_a_rating_on_the_floor_counts() {
    let shows = [
        show(1, "Alfa", "Drama", 2001, 100),
        show(2, "Bravo", "Drama", 2001, 80),
        show(3, "Charlie", "Drama", 2001, 79),
        show(4, "Delta", "Drama", 2001, 70),
        show(5, "Echo", "Drama", 2001, 69),
        show(6, "Foxtrot", "Drama", 2001, 0),
    ];
    let idx = index(&[], &shows);
    let with = |stars| {
        names(
            &idx,
            &Query {
                stars,
                sort: Sort::Title,
                ..Query::default()
            },
        )
    };
    assert_eq!(with(Stars::EightUp.bit()), ["Alfa", "Bravo"]);
    assert_eq!(
        with(Stars::SevenUp.bit()),
        ["Alfa", "Bravo", "Charlie", "Delta"],
        "an 8 is a 7 too"
    );
    assert_eq!(
        with(Stars::EightUp.bit() | Stars::SevenUp.bit()),
        ["Alfa", "Bravo", "Charlie", "Delta"],
        "values add up and a title is found once"
    );
    assert_eq!(idx.search(&Query::default(), false).counts.stars, [2, 4]);
    // With ★ 8 chosen, ★ 7 still says what choosing it would give.
    let eight = Query {
        stars: Stars::EightUp.bit(),
        ..Query::default()
    };
    assert_eq!(idx.search(&eight, false).counts.stars, [2, 4]);
}

#[test]
fn a_film_with_a_dutch_and_an_english_version_is_found_under_both_languages() {
    let movies = [
        film(1, "NL | Heat (1995)", 0, 80),
        film(2, "EN | Heat (1995)", 0, 80),
        film(3, "Ronin (1998)", 0, 70),
    ];
    let idx = index(&movies, &[]);
    assert_eq!(idx.len(), 2, "the two Heats are one title");
    let spoken = |s: Spoken| {
        names(
            &idx,
            &Query {
                spoken: s.bit(),
                ..Query::default()
            },
        )
    };
    assert_eq!(spoken(Spoken::Dutch), ["Heat"]);
    assert_eq!(spoken(Spoken::English), ["Heat"]);
    assert_eq!(spoken(Spoken::Other), ["Ronin"]);
    let both = Query {
        spoken: Spoken::Dutch.bit() | Spoken::English.bit(),
        ..Query::default()
    };
    assert_eq!(names(&idx, &both), ["Heat"], "found once, not twice");
    assert_eq!(
        idx.search(&Query::default(), false).counts.spoken,
        [1, 1, 1]
    );
}

#[test]
fn each_count_leaves_its_own_group_out_and_obeys_the_three_others() {
    let cats = Categories {
        vod: vec![
            category(1, "NL | Komedie"),
            category(2, "NL | Drama"),
            category(3, "EN | Komedie"),
            category(4, "Komedie"),
        ],
        series: Vec::new(),
    };
    let movies = [
        film(1, "Alfa (2021)", 1, 85),
        film(2, "Bravo (2021)", 1, 75),
        film(3, "Charlie (2015)", 1, 85),
        film(4, "Delta (2021)", 2, 85),
        film(5, "Echo (2021)", 3, 85),
        film(6, "Foxtrot (2021)", 4, 85),
        film(7, "Golf (1990)", 1, 75),
    ];
    let idx = TitleIndex::build(&movies, &[], &cats, MAX_YEAR);
    // Dutch comedies since 2020 with ★ 8 or more: Alfa alone. Each of the others
    // fails one group, or two (Golf).
    let query = Query {
        genres: Genre::Comedy.bit(),
        years: Years::Since2020.bit(),
        stars: Stars::EightUp.bit(),
        spoken: Spoken::Dutch.bit(),
        ..Query::default()
    };
    let results = idx.search(&query, false);
    assert_eq!(names(&idx, &query), ["Alfa"]);
    let c = &results.counts;
    // Soort: all the films that pass the four filters.
    assert_eq!((c.all, c.films, c.series), (1, 1, 0));
    // Genre, without the genre filter: Alfa, and Delta's drama.
    let mut genres = [0; 24];
    genres[Genre::ALL.iter().position(|g| *g == Genre::Comedy).unwrap()] = 1;
    genres[Genre::ALL.iter().position(|g| *g == Genre::Drama).unwrap()] = 1;
    assert_eq!(c.genres, genres);
    // Jaar, without the year filter: Alfa (2021) and Charlie (2015); Golf (1990)
    // is ★ 7.
    assert_eq!(c.years, [1, 1, 0, 0]);
    // Waardering, without it: Alfa (both) and Bravo (★ 7 only).
    assert_eq!(c.stars, [1, 2]);
    // Taal, without it: Alfa (Dutch), Echo (English), Foxtrot (no language).
    assert_eq!(c.spoken, [1, 1, 1]);
}

#[test]
fn bits_in_a_query_that_name_no_value_match_nothing_and_panic_nowhere() {
    let idx = index(
        &[film(1, "Alfa (2021)", 1, 85)],
        &[show(1, "Bravo", "Drama", 2001, 50)],
    );
    assert_eq!(idx.search(&Query::default(), false).titles.len(), 2);
    for query in [
        Query {
            genres: !0 << Genre::ALL.len(),
            ..Query::default()
        },
        Query {
            years: !0 << Years::ALL.len(),
            ..Query::default()
        },
        Query {
            stars: !0 << Stars::ALL.len(),
            ..Query::default()
        },
        Query {
            spoken: !0 << Spoken::ALL.len(),
            ..Query::default()
        },
    ] {
        let results = idx.search(&query, true);
        assert!(results.titles.is_empty(), "{query:?}");
        assert_eq!(results.counts.all, 0, "{query:?}");
    }
}

// ---- the sorts ----

#[test]
fn every_tie_ends_in_a_to_z_and_equal_titles_in_the_order_of_the_index() {
    // The same ★ and the same day: A to Z decides, whatever the sort.
    let shows = [
        Series {
            last_modified: DAY_20000,
            ..show(1, "Charlie", "Drama", 2001, 70)
        },
        Series {
            last_modified: DAY_20000,
            ..show(2, "alpha", "Drama", 2002, 70)
        },
        Series {
            last_modified: DAY_20000,
            ..show(3, "Bravo", "Drama", 2003, 70)
        },
    ];
    let idx = index(&[], &shows);
    for sort in [Sort::Relevance, Sort::Rating, Sort::Newest, Sort::Title] {
        let query = Query {
            sort,
            ..Query::default()
        };
        assert_eq!(
            names(&idx, &query),
            ["alpha", "Bravo", "Charlie"],
            "{sort:?}"
        );
    }
    // Titles that fold alike: series before films, the lower id first.
    let shows = [
        show(2, "Dark", "", 0, 0),
        show(1, "dark", "", 0, 0),
        show(3, "Dárk", "", 0, 0),
    ];
    let idx = index(&[film(7, "DARK (2020)", 0, 0)], &shows);
    let want = [
        title_id(Kind::Series, 1),
        title_id(Kind::Series, 2),
        title_id(Kind::Series, 3),
        title_id(Kind::Film, 7),
    ];
    for sort in [Sort::Relevance, Sort::Rating, Sort::Newest, Sort::Title] {
        let query = Query {
            sort,
            ..text("dark")
        };
        let ids: Vec<TitleId> = idx
            .search(&query, false)
            .titles
            .iter()
            .map(|&p| idx.title(p as usize).unwrap().id)
            .collect();
        assert_eq!(ids, want, "{sort:?}");
    }
}

#[test]
fn a_displaced_duplicate_takes_no_place_in_the_a_to_z_order() {
    // Two series of one id: the first stays, and "Aaa" is not in the order.
    let shows = [
        show(1, "Zed", "Drama", 2001, 0),
        show(1, "Aaa", "Drama", 2001, 0),
        show(2, "Mid", "Drama", 2001, 0),
    ];
    let idx = index(&[], &shows);
    assert_eq!(idx.len(), 2);
    let query = Query {
        sort: Sort::Title,
        ..Query::default()
    };
    assert_eq!(idx.search(&query, false).titles, [1, 0]);
    assert_eq!(names(&idx, &query), ["Mid", "Zed"]);
}

#[test]
fn an_empty_index_answers_nothing() {
    let idx = index(&[], &[]);
    assert_eq!(idx.search(&text("anything"), true), Results::default());
    assert_eq!(idx.search(&Query::default(), false), Results::default());
    let none = HashSet::new();
    assert!(idx.more_like(0, 5, true, &none, &none).is_empty());
}

#[test]
fn what_details_learn_shows_in_the_next_search_and_a_stray_genre_bit_is_counted_nowhere() {
    let idx = index(
        &[],
        &[
            show(1, "Plain", "", 0, 50),
            show(2, "Known", "Comedy", 2001, 50),
        ],
    );
    let drama = Query {
        genres: Genre::Drama.bit(),
        ..Query::default()
    };
    let since = Query {
        years: Years::Since2020.bit(),
        ..Query::default()
    };
    assert!(names(&idx, &drama).is_empty());
    assert!(names(&idx, &since).is_empty());
    assert_eq!(
        idx.search(&Query::default(), false).counts.years,
        [0, 0, 1, 0]
    );
    // Plain learns a year and a genre, and a bit that is no genre.
    let plain = idx.find(title_id(Kind::Series, 1)).unwrap();
    idx.learn(
        plain,
        Learned {
            year: 2022,
            genres: Genre::Drama.bit() | 1 << 24 | 1 << 31,
            trailer: false,
        },
    );
    assert_eq!(names(&idx, &drama), ["Plain"]);
    assert_eq!(names(&idx, &since), ["Plain"]);
    let counts = idx.search(&Query::default(), false).counts;
    assert_eq!(counts.years, [1, 0, 1, 0]);
    // Known's comedy and Plain's drama; the bits past the 24 count for nothing.
    assert_eq!(counts.genres.iter().sum::<usize>(), 2);
}

// ---- meer zoals dit ----

/// The names `more_like` gives for title `pos`, in its order.
fn like_names(
    idx: &TitleIndex,
    pos: usize,
    limit: usize,
    watched: &HashSet<TitleId>,
    disliked: &HashSet<TitleId>,
) -> Vec<String> {
    idx.more_like(pos, limit, false, watched, disliked)
        .iter()
        .map(|&p| idx.title(p as usize).unwrap().name.to_owned())
        .collect()
}

#[test]
fn a_shared_category_is_enough_and_the_two_id_spaces_are_two_categories() {
    // Films and series number their categories apart: series category 7 is not
    // film category 7. None of these has a genre.
    let cats = Categories {
        vod: vec![category(7, "Nieuw"), category(8, "Overig")],
        series: vec![category(7, "Nieuw")],
    };
    let movies = [
        film(1, "Seed (2020)", 7, 50),
        film(2, "Same (2019)", 7, 40),
        film(3, "Elsewhere (2018)", 8, 90),
    ];
    let shows = [Series {
        category_id: 7,
        ..show(1, "Series Seven", "", 2020, 95)
    }];
    let idx = TitleIndex::build(&movies, &shows, &cats, MAX_YEAR);
    let seed = idx.find(title_id(Kind::Film, 1)).unwrap();
    assert_eq!(idx.title(seed).unwrap().genres, 0);
    let none = HashSet::new();
    assert_eq!(like_names(&idx, seed, 9, &none, &none), ["Same"]);
    // And from the series' side: the film of category 7 is not its like.
    let series = idx.find(title_id(Kind::Series, 1)).unwrap();
    assert!(like_names(&idx, series, 9, &none, &none).is_empty());
}

#[test]
fn what_was_watched_or_disliked_is_told_by_kind_and_by_id() {
    // Film 5 and series 5 are two titles.
    let movies = [
        film(5, "Film Five (2020)", 1, 70),
        film(9, "Seed (2020)", 1, 60),
    ];
    let shows = [show(5, "Series Five", "Comedy", 2020, 80)];
    let idx = index(&movies, &shows);
    let seed = idx.find(title_id(Kind::Film, 9)).unwrap();
    let (film5, series5) = (title_id(Kind::Film, 5), title_id(Kind::Series, 5));
    let like = |watched: &[TitleId], disliked: &[TitleId]| {
        let (w, d) = (
            watched.iter().copied().collect(),
            disliked.iter().copied().collect(),
        );
        like_names(&idx, seed, 9, &w, &d)
    };
    assert_eq!(like(&[], &[]), ["Series Five", "Film Five"]);
    assert_eq!(like(&[film5], &[]), ["Series Five"]);
    assert_eq!(like(&[series5], &[]), ["Film Five"]);
    assert_eq!(like(&[], &[film5]), ["Series Five"]);
    assert_eq!(like(&[], &[series5]), ["Film Five"]);
    assert!(like(&[series5], &[film5]).is_empty());
}

#[test]
fn more_like_keeps_the_best_of_a_limit_in_order_and_ties_go_to_the_newest_then_a_to_z() {
    let day = |n: i64| DAY_20000 + n * DAY;
    let shows = [
        show(1, "Seed", "Comedy", 2020, 50),
        Series {
            last_modified: day(5),
            ..show(2, "Tie New", "Comedy", 2020, 80)
        },
        Series {
            last_modified: day(1),
            ..show(3, "Tie Old", "Comedy", 2020, 80)
        },
        show(4, "Top", "Comedy", 2020, 90),
        show(5, "Same B", "Comedy", 2020, 70),
        show(6, "Same A", "Comedy", 2020, 70),
        Series {
            last_modified: day(5),
            ..show(7, "Unrated New", "Comedy", 2020, 0)
        },
        Series {
            last_modified: day(1),
            ..show(8, "Unrated Old", "Comedy", 2020, 0)
        },
    ];
    let idx = index(&[], &shows);
    let seed = idx.find(title_id(Kind::Series, 1)).unwrap();
    let none = HashSet::new();
    let full = like_names(&idx, seed, 99, &none, &none);
    assert_eq!(
        full,
        [
            "Top",
            "Tie New",
            "Tie Old",
            "Same A",
            "Same B",
            "Unrated New",
            "Unrated Old"
        ]
    );
    for limit in 1..=full.len() {
        assert_eq!(
            like_names(&idx, seed, limit, &none, &none),
            full[..limit],
            "limit {limit}"
        );
    }
    assert_eq!(like_names(&idx, seed, full.len() + 1, &none, &none), full);
}

// ---- the queries held against a plain reading of the rules ----
//
// A catalog whose titles differ in every way a filter, a count or a sort can
// tell apart (and many that fold alike, so that ties are the rule), and the
// rules of Rulings 25 to 27 written out in the plainest way, title by title.
// The index must give the same titles in the same order, and the same counts.

/// A small generator: the same catalog and the same queries on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as usize % n
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len())]
    }
}

const WORDS: [&str; 10] = [
    "dark", "city", "night", "sun", "moon", "river", "ähnlich", "alpha", "beta", "gamma's",
];

/// One to three of `WORDS`, the first letter in capitals.
fn made_up_title(rng: &mut Rng) -> String {
    let words: Vec<&str> = (0..1 + rng.below(3)).map(|_| *rng.pick(&WORDS)).collect();
    let title = words.join(" ");
    let mut chars = title.chars();
    let first: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    first + chars.as_str()
}

/// An add day: unknown, or one of a few days after unix day 20,000.
fn made_up_day(rng: &mut Rng) -> i64 {
    match rng.below(5) {
        0 => 0,
        n => DAY_20000 + n as i64 * DAY,
    }
}

fn varied() -> (Vec<Movie>, Vec<Series>, Categories) {
    let mut rng = Rng(42);
    let years = [0u16, 1985, 1999, 2000, 2009, 2010, 2019, 2020, 2025];
    let ratings = [0u8, 55, 69, 70, 79, 80, 95];
    let movies: Vec<Movie> = (0..160)
        .map(|i| {
            let year = *rng.pick(&years);
            let prefix = *rng.pick(&["", "NL | ", "EN | "]);
            let title = made_up_title(&mut rng);
            let year = if year > 0 {
                format!(" ({year})")
            } else {
                String::new()
            };
            let label = *rng.pick(&["", " 4K", " 1080p"]);
            Movie {
                stream_id: 1000 + i,
                name: format!("{prefix}{title}{year}{label}"),
                is_adult: rng.below(17) == 0,
                rating: *rng.pick(&ratings),
                added: made_up_day(&mut rng),
                category_id: rng.below(5) as i64,
                ..Movie::default()
            }
        })
        .collect();
    let genres = [
        "",
        "Drama",
        "Comedy, Crime",
        "Horror",
        "Kids",
        "Sci-Fi & Fantasy",
    ];
    let shows: Vec<Series> = (0..100)
        .map(|i| {
            let year = *rng.pick(&years);
            let prefix = *rng.pick(&["", "NL | "]);
            Series {
                series_id: 1 + i,
                name: format!("{prefix}{}", made_up_title(&mut rng)),
                is_adult: rng.below(19) == 0,
                rating: *rng.pick(&ratings),
                release_date_camel: (year > 0).then(|| format!("{year}-06-01")),
                genre: Some((*rng.pick(&genres)).to_owned()),
                category_id: rng.below(3) as i64,
                last_modified: made_up_day(&mut rng),
                ..Series::default()
            }
        })
        .collect();
    let categories = Categories {
        vod: vec![
            category(1, "NL | Komedie"),
            category(2, "EN | Drama"),
            category(3, "Nieuw"),
            category(4, "Kids"),
        ],
        series: vec![category(1, "NL | Thriller"), category(2, "Nieuw")],
    };
    (movies, shows, categories)
}

/// What the reference needs of a title.
struct Row {
    id: TitleId,
    /// Its folded name: what a query's words are looked for in.
    key: String,
    genres: u32,
    category: Option<(Kind, i64)>,
    year: u16,
    rating: u8,
    added: u16,
    dutch: bool,
    english: bool,
    adult: bool,
}

fn rows(index: &TitleIndex) -> Vec<Row> {
    (0..index.len())
        .map(|pos| {
            let t = index.title(pos).unwrap();
            Row {
                id: t.id,
                key: fold(t.name),
                genres: t.genres,
                category: t.category.map(|c| (c.kind, c.id)),
                year: t.year,
                rating: t.rating,
                added: t.added_day,
                dutch: t.dutch,
                english: t.english,
                adult: t.adult,
            }
        })
        .collect()
}

/// Each title's place in the A to Z order of the folded titles; equal ones in
/// the order of the index.
fn ranks(rows: &[Row]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| rows[a].key.cmp(&rows[b].key).then(a.cmp(&b)));
    let mut rank = vec![0; rows.len()];
    for (place, pos) in order.into_iter().enumerate() {
        rank[pos] = place;
    }
    rank
}

/// ★ high to low, a title without one last.
fn by_stars(row: &Row) -> (bool, Reverse<u8>) {
    (row.rating == 0, Reverse(row.rating))
}

/// The newest first, an unknown day last.
fn by_day(row: &Row) -> (bool, Reverse<u16>) {
    (row.added == 0, Reverse(row.added))
}

fn speaks(row: &Row, spoken: Spoken) -> bool {
    match spoken {
        Spoken::Dutch => row.dutch,
        Spoken::English => row.english,
        Spoken::Other => !row.dutch && !row.english,
    }
}

/// Does the title pass the filter of each group: kind, genre, year, stars,
/// language? A group with nothing chosen lets everything through.
fn passes(row: &Row, q: &Query) -> [bool; 5] {
    [
        q.kind.is_none_or(|k| k == row.id.kind),
        q.genres == 0 || row.genres & q.genres != 0,
        q.years == 0 || Years::of(row.year).is_some_and(|y| q.years & y.bit() != 0),
        q.stars == 0
            || Stars::ALL
                .iter()
                .any(|s| q.stars & s.bit() != 0 && row.rating >= s.floor()),
        q.spoken == 0
            || Spoken::ALL
                .iter()
                .any(|s| q.spoken & s.bit() != 0 && speaks(row, *s)),
    ]
}

/// The positions a query gives, in order, and the counts beside them.
fn reference(rows: &[Row], q: &Query, unlocked: bool) -> (Vec<u32>, Counts) {
    let folded = fold(&q.text);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    let rank = ranks(rows);
    let mut counts = Counts::default();
    let mut found: Vec<usize> = Vec::new();
    for (pos, row) in rows.iter().enumerate() {
        if (row.adult && !unlocked) || !words.iter().all(|w| row.key.contains(w)) {
            continue;
        }
        let pass = passes(row, q);
        // Would it pass, were only the filter of group `skip` not asked?
        let but = |skip: usize| (0..5).all(|g| g == skip || pass[g]);
        if but(0) {
            counts.all += 1;
            match row.id.kind {
                Kind::Film => counts.films += 1,
                Kind::Series => counts.series += 1,
            }
        }
        if but(1) {
            for (i, g) in Genre::ALL.iter().enumerate() {
                counts.genres[i] += usize::from(row.genres & g.bit() != 0);
            }
        }
        if let (true, Some(y)) = (but(2), Years::of(row.year)) {
            counts.years[Years::ALL.iter().position(|x| *x == y).unwrap()] += 1;
        }
        if but(3) {
            for (i, s) in Stars::ALL.iter().enumerate() {
                counts.stars[i] += usize::from(row.rating >= s.floor());
            }
        }
        if but(4) {
            for (i, s) in Spoken::ALL.iter().enumerate() {
                counts.spoken[i] += usize::from(speaks(row, *s));
            }
        }
        if pass.iter().all(|p| *p) {
            found.push(pos);
        }
    }
    let starts = |p: usize| rows[p].key.starts_with(&folded);
    found.sort_by(|&a, &b| match q.sort {
        Sort::Relevance => (!starts(a), by_stars(&rows[a]), rank[a]).cmp(&(
            !starts(b),
            by_stars(&rows[b]),
            rank[b],
        )),
        Sort::Rating => (by_stars(&rows[a]), by_day(&rows[a]), rank[a]).cmp(&(
            by_stars(&rows[b]),
            by_day(&rows[b]),
            rank[b],
        )),
        Sort::Newest => (by_day(&rows[a]), by_stars(&rows[a]), rank[a]).cmp(&(
            by_day(&rows[b]),
            by_stars(&rows[b]),
            rank[b],
        )),
        Sort::Title => rank[a].cmp(&rank[b]),
    });
    (found.into_iter().map(|p| p as u32).collect(), counts)
}

fn made_up_query(rng: &mut Rng) -> Query {
    const TEXTS: [&str; 14] = [
        "",
        "dark",
        "night",
        "dark city",
        "city dark",
        "sun",
        "a",
        "zzz",
        "ahn",
        "gamma",
        "gammas",
        "gamma's",
        "dark | city",
        "NIGHT moon",
    ];
    const GENRES: [Genre; 9] = [
        Genre::Comedy,
        Genre::Drama,
        Genre::Crime,
        Genre::Horror,
        Genre::Kids,
        Genre::ScienceFiction,
        Genre::Fantasy,
        Genre::Thriller,
        Genre::Action,
    ];
    let mut genres = 0;
    if rng.below(3) > 0 {
        for _ in 0..1 + rng.below(3) {
            genres |= rng.pick(&GENRES).bit();
        }
    }
    Query {
        text: (*rng.pick(&TEXTS)).to_owned(),
        kind: *rng.pick(&[None, Some(Kind::Series), Some(Kind::Film)]),
        genres,
        years: if rng.below(2) == 0 {
            0
        } else {
            rng.below(16) as u8
        },
        stars: if rng.below(2) == 0 {
            0
        } else {
            rng.below(4) as u8
        },
        spoken: if rng.below(2) == 0 {
            0
        } else {
            rng.below(8) as u8
        },
        sort: *rng.pick(&[Sort::Relevance, Sort::Rating, Sort::Newest, Sort::Title]),
    }
}

#[test]
fn search_agrees_with_a_plain_reading_of_the_rules() {
    let (movies, shows, cats) = varied();
    let idx = TitleIndex::build(&movies, &shows, &cats, MAX_YEAR);
    let rows = rows(&idx);
    assert!(rows.len() > 150, "{} titles", rows.len());
    let mut rng = Rng(7);
    let (mut some, mut adult_hidden) = (0, 0);
    for n in 0..600 {
        let query = made_up_query(&mut rng);
        let unlocked = rng.below(2) == 0;
        let (titles, counts) = reference(&rows, &query, unlocked);
        let got = idx.search(&query, unlocked);
        let what = format!("query {n}: {query:?}, unlocked {unlocked}");
        assert_eq!(got.titles, titles, "{what}");
        assert_eq!(got.counts, counts, "{what}");
        some += usize::from(!titles.is_empty());
        let with_adult = idx.search(&query, true).counts.all;
        adult_hidden += usize::from(!unlocked && got.counts.all < with_adult);
    }
    // The comparison had something to compare.
    assert!(some > 250, "{some} of 600 queries found a title");
    assert!(
        adult_hidden > 20,
        "{adult_hidden} queries hid an adult title"
    );
}

#[test]
fn more_like_agrees_with_a_plain_reading_of_the_rule() {
    let (movies, shows, cats) = varied();
    let idx = TitleIndex::build(&movies, &shows, &cats, MAX_YEAR);
    let rows = rows(&idx);
    let rank = ranks(&rows);
    let watched: HashSet<TitleId> = rows.iter().step_by(4).map(|r| r.id).collect();
    let disliked: HashSet<TitleId> = rows.iter().skip(1).step_by(6).map(|r| r.id).collect();
    let mut some = 0;
    for (pos, seed) in rows.iter().enumerate().step_by(3) {
        for (limit, unlocked) in [
            (0, false),
            (1, false),
            (3, true),
            (10, false),
            (10_000, true),
        ] {
            let mut like: Vec<usize> = (0..rows.len())
                .filter(|&p| {
                    let r = &rows[p];
                    p != pos
                        && (r.genres & seed.genres != 0
                            || (seed.category.is_some() && r.category == seed.category))
                        && (unlocked || !r.adult)
                        && !watched.contains(&r.id)
                        && !disliked.contains(&r.id)
                })
                .collect();
            like.sort_by_key(|&p| (by_stars(&rows[p]), by_day(&rows[p]), rank[p]));
            like.truncate(limit);
            let want: Vec<u32> = like.into_iter().map(|p| p as u32).collect();
            let got = idx.more_like(pos, limit, unlocked, &watched, &disliked);
            assert_eq!(got, want, "title {pos}, limit {limit}, unlocked {unlocked}");
            some += usize::from(!want.is_empty());
        }
    }
    assert!(some > 100, "{some} answers had a title in them");
}
