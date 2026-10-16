//! The title index's build (vod/index.rs, spec 2026-10-15 §4.2): what a
//! record holds, where its year, genres and language come from, which ids fit,
//! lookups by id and by version, what details add, its size, and that a genre
//! name is read once and not once per title.

use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Learned, TitleIndex};
use bongo_player::vod::versions::merge;
use bongo_player::vod::{Kind, TitleId};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const MAX_YEAR: u16 = 2027;
/// 15 October 2026, 00:00 UTC, as unix seconds: unix day 20,741.
const OCT_15: i64 = 1_792_022_400;
const DAY: i64 = 86_400;

fn film(stream_id: i64, name: &str) -> Movie {
    Movie {
        stream_id,
        name: name.into(),
        container_extension: Some("mkv".into()),
        ..Movie::default()
    }
}

fn series(series_id: i64, name: &str) -> Series {
    Series {
        series_id,
        name: name.into(),
        ..Series::default()
    }
}

fn category(id: i64, name: &str) -> Category {
    Category {
        category_id: id,
        category_name: name.into(),
    }
}

fn categories() -> Categories {
    Categories {
        vod: vec![
            category(12, "NL | Komedie"),
            category(13, "EN | Action & Adventure"),
            category(14, "Nieuw"),
        ],
        series: vec![category(12, "Series | Drama"), category(7, "Kids")],
    }
}

fn film_id(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

fn series_id(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

#[test]
fn a_catalog_becomes_its_series_and_its_films_merged_into_titles() {
    let movies = [
        Movie {
            stream_icon: Some(" https://img.example/matrix.jpg ".into()),
            rating: 82,
            added: OCT_15 - 3 * DAY,
            category_id: 13,
            ..film(1, "The Matrix (1999) 1080p")
        },
        Movie {
            added: OCT_15 - 10 * DAY,
            category_id: 12,
            container_extension: Some("mp4".into()),
            ..film(2, "NL | The Matrix (1999) [4K]")
        },
        film(3, "Other (2001)"),
    ];
    let shows = [Series {
        cover: Some("https://img.example/dark.jpg".into()),
        rating: 87,
        release_date_camel: Some("2017-12-01".into()),
        genre: Some("Drama, Mystery".into()),
        category_id: 7,
        last_modified: OCT_15,
        ..series(4321, "Dark")
    }];
    let index = TitleIndex::build(&movies, &shows, &categories(), MAX_YEAR);
    assert_eq!(index.len(), 3, "one series, two film titles");
    // Series come first: the records are in (kind, id) order.
    let dark = index.title(0).unwrap();
    assert_eq!(dark.id, series_id(4321));
    assert_eq!((dark.name, dark.year, dark.rating), ("Dark", 2017, 87));
    assert_eq!(dark.cover, Some("https://img.example/dark.jpg"));
    assert_eq!(dark.genres, Genre::Drama.bit() | Genre::Mystery.bit());
    assert_eq!(
        dark.category.map(|c| (c.kind, c.id, c.name)),
        Some((Kind::Series, 7, "Kids"))
    );
    assert_eq!(dark.added_day, 20_741);
    assert!(dark.versions.is_empty());

    let matrix = index.title(index.find(film_id(2)).unwrap()).unwrap();
    assert_eq!(matrix.id, film_id(2), "the 4K version is the title's id");
    assert_eq!(
        (matrix.name, matrix.year, matrix.rating),
        ("The Matrix", 1999, 82)
    );
    assert_eq!(
        matrix.cover,
        Some("https://img.example/matrix.jpg"),
        "the first version that has one, trimmed"
    );
    assert_eq!(matrix.added_day, 20_741 - 10, "as new as its first version");
    assert_eq!(
        matrix.category.map(|c| c.name),
        Some("NL | Komedie"),
        "the default version's"
    );
    let versions: Vec<(i64, Labels, &str)> = matrix
        .versions
        .iter()
        .map(|v| (v.stream_id, v.labels, v.ext))
        .collect();
    assert_eq!(
        versions,
        [
            (2, Labels::UHD.with(Labels::NL), "mp4"),
            (1, Labels::FHD, "mkv")
        ]
    );
    assert_eq!(
        index.find(film_id(1)),
        None,
        "a version that is not the default is no title id"
    );
    assert_eq!(index.find(series_id(2)), None);
}

#[test]
fn a_film_title_takes_genres_and_languages_from_every_versions_category_and_labels() {
    let movies = [
        Movie {
            category_id: 12,
            ..film(1, "Hotel (2020)")
        },
        Movie {
            category_id: 13,
            ..film(2, "Hotel (2020) 1080p")
        },
        Movie {
            category_id: 14,
            ..film(3, "Plain (2020)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &categories(), MAX_YEAR);
    let hotel = index.title(index.find(film_id(2)).unwrap()).unwrap();
    assert_eq!(
        hotel.genres,
        Genre::Comedy.bit() | Genre::Action.bit() | Genre::Adventure.bit()
    );
    assert!(hotel.dutch && hotel.english, "NL | Komedie and EN | Action");
    let plain = index.title(index.find(film_id(3)).unwrap()).unwrap();
    assert_eq!(plain.genres, 0, "Nieuw names no genre: the details may");
    assert!(!plain.dutch && !plain.english);
}

#[test]
fn a_series_takes_its_genres_from_its_field_else_from_its_category() {
    let shows = [
        Series {
            genre: Some("Comedy".into()),
            category_id: 12,
            ..series(1, "A")
        },
        Series {
            genre: Some("".into()),
            category_id: 12,
            ..series(2, "B")
        },
        Series {
            category_id: 99,
            ..series(3, "C")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &categories(), MAX_YEAR);
    let genres: Vec<u32> = (0..3).map(|p| index.title(p).unwrap().genres).collect();
    assert_eq!(genres, [Genre::Comedy.bit(), Genre::Drama.bit(), 0]);
    assert!(
        index.title(2).unwrap().category.is_none(),
        "an unknown category is none"
    );
}

#[test]
fn a_series_year_is_its_release_dates_else_its_names() {
    let shows = [
        Series {
            release_date: Some("2005-03-24".into()),
            ..series(1, "The Office (US) (2005)")
        },
        series(2, "Fargo (2014)"),
        Series {
            release_date_camel: Some("unknown".into()),
            ..series(3, "Lost")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    let years: Vec<(&str, u16)> = (0..3)
        .map(|p| {
            let t = index.title(p).unwrap();
            (t.name, t.year)
        })
        .collect();
    assert_eq!(
        years,
        [("The Office (US)", 2005), ("Fargo", 2014), ("Lost", 0)]
    );
}

#[test]
fn a_title_is_adult_when_one_of_its_versions_is() {
    let movies = [
        film(1, "Night (2020)"),
        Movie {
            is_adult: true,
            ..film(2, "Night (2020) 4K")
        },
        film(3, "Day (2020)"),
    ];
    let shows = [Series {
        is_adult: true,
        ..series(9, "Late")
    }];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let adult = |id: TitleId| index.title(index.find(id).unwrap()).unwrap().adult;
    assert!(adult(film_id(2)));
    assert!(!adult(film_id(3)));
    assert!(adult(series_id(9)));
}

#[test]
fn what_plays_tells_which_title_it_is() {
    let movies = [
        film(10, "A (2001) 1080p"),
        film(11, "A (2001) 4K"),
        film(20, "B (2002)"),
    ];
    let shows = [series(5, "S")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let a = index.find(film_id(11)).unwrap();
    assert_eq!(index.title_of_version(10), Some(a), "the 1080p version");
    assert_eq!(index.title_of_version(11), Some(a));
    assert_eq!(index.title_of_version(20), index.find(film_id(20)));
    assert_eq!(index.title_of_version(5), None, "a series id is no version");
    assert_eq!(index.title_of_version(-1), None);
}

#[test]
fn an_id_that_does_not_fit_is_left_out_and_counted() {
    // Xtream ids are positive (Ruling 21) and the portal's unreadable id is 0
    // (Ruling 3): 1 to u32::MAX fit, and 0 fits no more than a negative id.
    let movies = [
        film(1, "Fits (2001)"),
        film(-4, "Below (2001)"),
        film(5_000_000_000, "Above (2001)"),
        film(0, "Zero (2001)"),
    ];
    let shows = [series(-1, "Negative"), series(0, "Zero")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.stats().skipped, 5);
    assert_eq!(index.find(film_id(0)), None);
    assert_eq!(index.find(series_id(0)), None);
    assert_eq!(index.title_of_version(0), None);
}

#[test]
fn two_series_of_one_id_keep_the_first() {
    let shows = [series(7, "First"), series(7, "Second")];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.title(0).unwrap().name, "First");
}

#[test]
fn details_fill_what_the_lists_left_open_and_change_nothing_else() {
    let movies = [
        film(1, "No Year"),
        Movie {
            category_id: 12,
            ..film(2, "Known (2001)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &categories(), MAX_YEAR);
    let learned = Learned {
        year: 1994,
        genres: Genre::Horror.bit(),
        trailer: true,
    };
    let (open, known) = (
        index.find(film_id(1)).unwrap(),
        index.find(film_id(2)).unwrap(),
    );
    index.learn(open, learned);
    index.learn(known, learned);
    let t = index.title(open).unwrap();
    assert_eq!(
        (t.year, t.genres, t.trailer),
        (1994, Genre::Horror.bit(), true)
    );
    let t = index.title(known).unwrap();
    assert_eq!(
        (t.year, t.genres, t.trailer),
        (2001, Genre::Comedy.bit(), true),
        "the name's year and the category's genre stay"
    );
    index.learn(99, learned); // no such title: nothing happens
}

#[test]
fn the_stats_count_what_the_lists_gave() {
    let movies = [
        Movie {
            stream_icon: Some("https://x/1.jpg".into()),
            rating: 70,
            category_id: 12,
            ..film(1, "A (2001)")
        },
        film(2, "B"),
        film(3, "B 4K"),
    ];
    let shows = [Series {
        genre: Some("Drama".into()),
        ..series(1, "S EN")
    }];
    let index = TitleIndex::build(&movies, &shows, &categories(), MAX_YEAR);
    let s = index.stats();
    assert_eq!((s.series, s.films, s.versions), (1, 2, 3));
    assert_eq!(
        (s.with_cover, s.with_rating, s.with_year, s.with_genre),
        (1, 1, 1, 2)
    );
    assert_eq!((s.dutch, s.english, s.skipped), (1, 1, 0));
}

#[test]
fn the_index_keeps_small_records_and_one_text_table_per_text() {
    let movies: Vec<Movie> = (0..1_000)
        .map(|i| Movie {
            stream_icon: Some(format!("https://image.tmdb.org/t/p/w600/{i:027}.jpg")),
            // Ids from 1: the portal's 0 fits no title.
            ..film(i + 1, &format!("Film number {i} (2001)"))
        })
        .collect();
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1_000);
    // A record is 20 bytes, a version 8, a place in the A-Z order 4, and a
    // name and its search name ~18 bytes each with 4 for where they end.
    let per_title = index.heap_bytes() / 1_000;
    assert!(per_title <= 90, "{per_title} bytes a title");
    assert!(
        index.cover_bytes() >= 1_000 * 67,
        "the covers are counted apart"
    );
}

#[test]
fn no_lists_no_titles() {
    let index = TitleIndex::build(&[], &[], &Categories::default(), MAX_YEAR);
    assert!(index.is_empty());
    assert!(index.title(0).is_none());
    assert_eq!(index.find(film_id(1)), None);
    assert_eq!(index.categories().count(), 0);
}

#[test]
fn the_largest_id_that_fits_is_kept_and_a_wider_one_is_never_cut_to_32_bits() {
    let movies = [
        film(1, "One (2001)"),
        film(4_294_967_295, "Last (2001)"),
        film(4_294_967_296, "Over (2001)"),
        film(4_294_967_297, "Over too (2001)"),
    ];
    let shows = [series(1, "One"), series(4_294_967_297, "Over too")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 3, "series 1, film 1 and film u32::MAX");
    assert_eq!(index.stats().skipped, 3);
    let last = index.find(film_id(4_294_967_295)).unwrap();
    assert_eq!(index.title(last).unwrap().name, "Last");
    assert_eq!(index.title_of_version(4_294_967_295), Some(last));
    // An id above u32::MAX finds nothing; it is not cut to 32 bits, where
    // 2^32 + 1 would be 1.
    assert_eq!(index.find(film_id(4_294_967_296)), None);
    assert_eq!(index.find(film_id(4_294_967_297)), None);
    assert_eq!(index.find(series_id(4_294_967_297)), None);
    assert_eq!(index.title_of_version(4_294_967_296), None);
    assert_eq!(index.title_of_version(4_294_967_297), None);
}

#[test]
fn a_version_whose_id_does_not_fit_is_left_out_of_its_title() {
    // The 4K copy has the id 0 that an unreadable one becomes. `merge` sorts a
    // version that does not fit last, so the title is the 1080p copy's, 7,
    // with that one version; the 4K copy is counted as left out.
    let movies = [
        film(0, "Frozen II (2019) 4K"),
        film(7, "Frozen II (2019) 1080p"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    let frozen = index.title(index.find(film_id(7)).unwrap()).unwrap();
    assert_eq!(frozen.id, film_id(7));
    assert_eq!((frozen.name, frozen.year), ("Frozen II", 2019));
    let versions: Vec<(i64, Labels)> = frozen
        .versions
        .iter()
        .map(|v| (v.stream_id, v.labels))
        .collect();
    assert_eq!(versions, [(7, Labels::FHD)]);
    assert_eq!(index.stats().skipped, 1);
    assert_eq!(index.stats().versions, 1);
    assert_eq!(index.find(film_id(0)), None);
    assert_eq!(index.title_of_version(0), None);
    assert_eq!(index.title_of_version(7), index.find(film_id(7)));

    // Several among versions that fit: each is left out and counted, the
    // others keep their order.
    let movies = [
        film(3, "Dune (2021) 1080p"),
        film(0, "Dune (2021) 720p"),
        film(9, "Dune (2021) 4K"),
        film(-2, "Dune (2021)"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    let dune = index.title(index.find(film_id(9)).unwrap()).unwrap();
    let ids: Vec<i64> = dune.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(ids, [9, 3]);
    assert_eq!(index.stats().skipped, 2);
}

#[test]
fn a_name_that_says_both_languages_makes_a_title_both_dutch_and_english() {
    // Ruling 20: a title's language is a set, read from each version's own
    // labels. `Labels::language()` names one language and Dutch wins, so a
    // version labelled NL and EN would count as Dutch only.
    let movies = [
        film(1, "Frozen II (2019) NL EN"),
        film(2, "Hotel (2020) NL"),
        film(3, "Hotel (2020) EN"),
        film(4, "Soon (2021) NL"),
        film(5, "Later (2022) EN"),
        film(6, "Subs (2023) NL SUBS"),
        film(7, "Unmarked (2024)"),
    ];
    let shows = [
        series(1, "Dark NL EN"),
        series(2, "Baantjer NL"),
        series(3, "Sherlock EN"),
        series(4, "Plain"),
    ];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let languages = |id: TitleId| {
        let t = index.title(index.find(id).unwrap()).unwrap();
        (t.dutch, t.english)
    };
    assert_eq!(languages(film_id(1)), (true, true), "one version, NL EN");
    assert_eq!(
        languages(series_id(1)),
        (true, true),
        "a series named NL EN"
    );
    assert_eq!(
        languages(film_id(2)),
        (true, true),
        "a Dutch and an English one"
    );
    assert_eq!(languages(film_id(4)), (true, false));
    assert_eq!(languages(film_id(5)), (false, true));
    assert_eq!(
        languages(film_id(6)),
        (false, false),
        "NL SUBS is no marker"
    );
    assert_eq!(languages(film_id(7)), (false, false));
    assert_eq!(languages(series_id(2)), (true, false));
    assert_eq!(languages(series_id(3)), (false, true));
    assert_eq!(languages(series_id(4)), (false, false));
    let s = index.stats();
    assert_eq!((s.dutch, s.english), (5, 5));
}

#[test]
fn a_categorys_language_is_read_from_the_label_words_of_its_name() {
    // `category_labels`: the label words a name starts with, and the labels
    // before a year at its end. A category names one language, Dutch first.
    let categories = Categories {
        vod: vec![
            category(1, "NL FILMS"),
            category(2, "Films NL (2019)"),
            category(3, "UK Series"),
            category(4, "Films en Series"),
        ],
        series: vec![category(1, "Nederlands"), category(2, "Series")],
    };
    let movies = [
        Movie {
            category_id: 1,
            ..film(1, "A (2001)")
        },
        Movie {
            category_id: 2,
            ..film(2, "B (2001)")
        },
        Movie {
            category_id: 3,
            ..film(3, "C (2001)")
        },
        Movie {
            category_id: 4,
            ..film(4, "D (2001)")
        },
    ];
    let shows = [
        Series {
            category_id: 1,
            ..series(1, "E")
        },
        Series {
            category_id: 2,
            ..series(2, "F")
        },
    ];
    let index = TitleIndex::build(&movies, &shows, &categories, MAX_YEAR);
    let languages = |id: TitleId| {
        let t = index.title(index.find(id).unwrap()).unwrap();
        (t.dutch, t.english)
    };
    assert_eq!(languages(film_id(1)), (true, false));
    assert_eq!(
        languages(film_id(2)),
        (true, false),
        "the year leaves the end"
    );
    assert_eq!(languages(film_id(3)), (false, true));
    assert_eq!(
        languages(film_id(4)),
        (false, false),
        "\"en\" is Dutch for and"
    );
    assert_eq!(languages(series_id(1)), (true, false));
    assert_eq!(languages(series_id(2)), (false, false));
}

#[test]
fn the_names_are_read_by_the_current_rules() {
    // Rulings 12 and 16 as the review changed them: labels after the last bar
    // and a group of labels in front are labels, the bars are alike, and an
    // apostrophe joins its word. Each of these shapes is one title.
    let movies = [
        film(1, "Frozen II (2019)"),
        film(2, "Frozen II (2019) 4K | NL"),
        film(3, "[NL] Frozen II (2019)"),
        film(4, "NL \u{ff5c} Frozen II (2019) 1080p"),
        film(5, "Ocean's Eleven (2001)"),
        film(6, "Oceans Eleven (2001) HD"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 2);
    let frozen = index.title(index.find(film_id(2)).unwrap()).unwrap();
    assert_eq!((frozen.name, frozen.year), ("Frozen II", 2019));
    let ids: Vec<i64> = frozen.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(ids, [2, 4, 1, 3]);
    assert!(
        frozen.dutch && !frozen.english,
        "the NL after the bar counts"
    );
    let oceans = index.title(index.find(film_id(6)).unwrap()).unwrap();
    assert_eq!((oceans.name, oceans.year), ("Oceans Eleven", 2001));
    let ids: Vec<i64> = oceans.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(ids, [6, 5]);
}

/// The system allocator, counting the requests a thread makes while it counts
/// (`allocations`): the threads of the other tests add nothing to it.
struct Counting;

thread_local! {
    // `const` and without a destructor, so the allocator may read them at any
    // time, also while a thread starts or ends.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn note() {
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            let _ = COUNT.try_with(|n| n.set(n.get() + 1));
        }
    });
}

// SAFETY: every call goes on to `System` as it came.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        System.realloc(ptr, layout, new_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What `f` returns, and how often it asked the allocator for memory.
fn allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    COUNT.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    let result = f();
    COUNTING.with(|on| on.set(false));
    (result, COUNT.with(Cell::get))
}

#[test]
fn the_genre_names_are_read_once_each_and_not_once_per_title() {
    // `genres_in` costs 7 to 13 us a call and allocates on each: 3,000 series
    // would add 30 ms to the build. It is called once per category and once
    // per distinct genre field of the series, never per title or per version.
    // The allocations count it, as they do not depend on the machine's speed.
    const N: i64 = 3_000;
    let movies: Vec<Movie> = (0..N)
        .flat_map(|i| {
            let category_id = 12 + i % 3;
            [
                Movie {
                    category_id,
                    ..film(2 * i + 1, &format!("Film x{i} (2001)"))
                },
                Movie {
                    category_id,
                    ..film(2 * i + 2, &format!("Film x{i} (2001) 4K"))
                },
            ]
        })
        .collect();
    // Three genre fields shared by all, in two categories.
    let shared: Vec<Series> = (0..N)
        .map(|i| Series {
            genre: Some(["Drama", "Comedy, Mystery", ""][(i % 3) as usize].into()),
            category_id: [12, 7][(i % 2) as usize],
            ..series(i + 1, &format!("Show x{i}"))
        })
        .collect();
    // The control: a genre field of its own for each.
    let distinct: Vec<Series> = (0..N)
        .map(|i| Series {
            genre: Some(format!("Drama {i}")),
            ..series(i + 1, &format!("Show x{i}"))
        })
        .collect();
    let categories = categories();

    // The films' part of a build allocates for each title it keeps (`merge`):
    // what the build adds to that must not grow with the titles. The first
    // merge only warms the name reader's scratch, which a build finds warm.
    let _ = merge(&movies, MAX_YEAR);
    let (_, merging) = allocations(|| merge(&movies, MAX_YEAR));
    let (index, building) =
        allocations(|| TitleIndex::build(&movies, &shared, &categories, MAX_YEAR));
    assert_eq!(index.len(), 2 * N as usize);
    let added = building.saturating_sub(merging);
    assert!(
        added < 300,
        "the build added {added} allocations to the merge's {merging} for {N} films of two \
         versions and {N} series: a genre name was read for each title or version"
    );

    // The count sees a call when there is one: the control costs one each.
    let (_, building_distinct) =
        allocations(|| TitleIndex::build(&movies, &distinct, &categories, MAX_YEAR));
    assert!(
        building_distinct >= building + N as usize,
        "{building_distinct} allocations for {N} different genre fields, {building} for three"
    );
}

/// What Task 4's A-Z rank adds to `heap_bytes` a title: 0 until the rank is
/// there, then 4 (a `u32` a title). The tests that count the heap exactly add it.
const RANK: usize = 0;

#[test]
fn every_flag_is_a_bit_of_its_own_and_learning_a_trailer_sets_that_bit_only() {
    // One title with each flag alone or in a pair, so that two flags sharing a
    // bit, or a learn that replaces the flags instead of adding one, show on some
    // title.
    let movies = [
        Movie {
            is_adult: true,
            ..film(1, "Adult (2001) NL")
        },
        film(2, "Plain (2002)"),
        film(3, "English (2003) EN"),
    ];
    let shows = [Series {
        is_adult: true,
        ..series(1, "Late EN")
    }];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let flags = |id: TitleId| {
        let t = index.title(index.find(id).unwrap()).unwrap();
        (t.adult, t.dutch, t.english, t.trailer, t.id.kind)
    };
    assert_eq!(flags(film_id(1)), (true, true, false, false, Kind::Film));
    assert_eq!(flags(film_id(2)), (false, false, false, false, Kind::Film));
    assert_eq!(flags(film_id(3)), (false, false, true, false, Kind::Film));
    assert_eq!(
        flags(series_id(1)),
        (true, false, true, false, Kind::Series)
    );
    for id in [film_id(1), film_id(3), series_id(1)] {
        let pos = index.find(id).unwrap();
        let before = flags(id);
        index.learn(
            pos,
            Learned {
                trailer: true,
                ..Learned::default()
            },
        );
        let (adult, dutch, english, trailer, kind) = flags(id);
        assert_eq!(
            (adult, dutch, english, kind),
            (before.0, before.1, before.2, before.4),
            "learning a trailer changes no other flag"
        );
        assert!(trailer);
        assert_eq!(index.find(id), Some(pos), "and the title is still found");
    }
}

#[test]
fn a_version_without_a_container_has_none_beside_one_that_has() {
    let movies = [
        Movie {
            container_extension: None,
            ..film(1, "Heat (1995) 4K")
        },
        Movie {
            container_extension: Some(" mkv ".into()),
            ..film(2, "Heat (1995) 1080p")
        },
        Movie {
            container_extension: Some("".into()),
            ..film(3, "Heat (1995) 720p")
        },
        Movie {
            container_extension: Some("mkv".into()),
            ..film(4, "Heat (1995) SD")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let heat = index.title(index.find(film_id(1)).unwrap()).unwrap();
    let exts: Vec<(i64, &str)> = heat.versions.iter().map(|v| (v.stream_id, v.ext)).collect();
    assert_eq!(
        exts,
        [(1, ""), (2, "mkv"), (3, ""), (4, "mkv")],
        "none is none, a padded one is trimmed, and mkv is one container"
    );
}

#[test]
fn a_portal_may_name_more_containers_than_the_index_has_codes_for() {
    // 600 films over 300 different containers: the first 255 are kept, in record
    // order; a later one that is not among them has none, and one that is still
    // has its own, however late it comes.
    let movies: Vec<Movie> = (1..=600)
        .map(|i| Movie {
            container_extension: Some(format!("x{}", (i - 1) % 300 + 1)),
            ..film(i, &format!("Film x{i}"))
        })
        .collect();
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 600);
    for i in 1..=600i64 {
        let n = (i - 1) % 300 + 1;
        let t = index.title(index.find(film_id(i)).unwrap()).unwrap();
        let want = if n <= 255 {
            format!("x{n}")
        } else {
            String::new()
        };
        assert_eq!(t.versions.get(0).unwrap().ext, want, "film {i}");
    }
}

#[test]
fn a_category_is_found_by_its_kind_and_id_and_the_first_of_an_id_counts() {
    let cats = Categories {
        vod: vec![
            category(0, "NL | Komedie"),
            category(5, "EN | Action"),
            category(5, "NL | Drama"),
            category(9, "Kids"),
        ],
        series: vec![category(5, "Series | Drama"), category(7, "Nieuw")],
    };
    let movies = [
        // 0 is the portal's "none" (and its unreadable id), though a category has it.
        Movie {
            category_id: 0,
            ..film(1, "Zero (2001)")
        },
        // The first category of an id counts.
        Movie {
            category_id: 5,
            ..film(2, "Five (2001)")
        },
        // 7 is a series category only.
        Movie {
            category_id: 7,
            ..film(3, "Seven (2001)")
        },
        Movie {
            category_id: -5,
            ..film(4, "Minus (2001)")
        },
    ];
    let shows = [
        Series {
            category_id: 5,
            ..series(1, "A")
        },
        // 9 is a film category only.
        Series {
            category_id: 9,
            ..series(2, "B")
        },
        Series {
            category_id: 0,
            ..series(3, "C")
        },
    ];
    let index = TitleIndex::build(&movies, &shows, &cats, MAX_YEAR);
    let of = |id: TitleId| {
        let t = index.title(index.find(id).unwrap()).unwrap();
        (
            t.category.map(|c| (c.kind, c.id, c.name)),
            t.genres,
            t.dutch,
            t.english,
        )
    };
    assert_eq!(of(film_id(1)), (None, 0, false, false), "id 0 names none");
    assert_eq!(
        of(film_id(2)),
        (
            Some((Kind::Film, 5, "EN | Action")),
            Genre::Action.bit(),
            false,
            true
        ),
        "the first of two categories of one id"
    );
    assert_eq!(of(film_id(3)), (None, 0, false, false), "a series category");
    assert_eq!(of(film_id(4)), (None, 0, false, false));
    assert_eq!(
        of(series_id(1)),
        (
            Some((Kind::Series, 5, "Series | Drama")),
            Genre::Drama.bit(),
            false,
            false
        ),
        "the series own id space"
    );
    assert_eq!(of(series_id(2)), (None, 0, false, false), "a film category");
    assert_eq!(of(series_id(3)), (None, 0, false, false));
}

#[test]
fn the_categories_are_listed_films_first_in_the_portals_order() {
    let index = TitleIndex::build(&[], &[], &categories(), MAX_YEAR);
    let listed: Vec<(Kind, i64, &str)> =
        index.categories().map(|c| (c.kind, c.id, c.name)).collect();
    assert_eq!(
        listed,
        [
            (Kind::Film, 12, "NL | Komedie"),
            (Kind::Film, 13, "EN | Action & Adventure"),
            (Kind::Film, 14, "Nieuw"),
            (Kind::Series, 12, "Series | Drama"),
            (Kind::Series, 7, "Kids"),
        ]
    );
}

#[test]
fn a_cover_and_a_rating_come_from_the_first_version_that_has_one() {
    let movies = [
        // The default has a cover and no rating; the next has both.
        Movie {
            stream_icon: Some("https://x/4k.jpg".into()),
            rating: 0,
            ..film(1, "Dune (2021) 4K")
        },
        Movie {
            stream_icon: Some("https://x/hd.jpg".into()),
            rating: 77,
            ..film(2, "Dune (2021) 1080p")
        },
        Movie {
            stream_icon: Some("  ".into()),
            rating: 55,
            ..film(3, "Dune (2021) 720p")
        },
        // The default has a rating and no cover; the next has a padded one.
        Movie {
            rating: 91,
            ..film(10, "Heat (1995) 4K")
        },
        Movie {
            stream_icon: Some(" https://x/heat.jpg ".into()),
            rating: 60,
            ..film(11, "Heat (1995) 1080p")
        },
        // A blank cover is none.
        Movie {
            stream_icon: Some("   ".into()),
            ..film(20, "Blank (2000)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let of = |id: i64| {
        let t = index.title(index.find(film_id(id)).unwrap()).unwrap();
        (t.cover, t.rating)
    };
    assert_eq!(of(1), (Some("https://x/4k.jpg"), 77));
    assert_eq!(of(10), (Some("https://x/heat.jpg"), 91));
    assert_eq!(of(20), (None, 0));
}

#[test]
fn a_title_is_as_new_as_its_earliest_known_version() {
    let movies = [
        // 5 days old, 20 days old, and unknown (last): the earliest known day.
        Movie {
            added: OCT_15 - 5 * DAY,
            ..film(1, "Tron (1982) 4K")
        },
        Movie {
            added: OCT_15 - 20 * DAY,
            ..film(2, "Tron (1982) 1080p")
        },
        Movie {
            added: 0,
            ..film(3, "Tron (1982) 720p")
        },
        // An unknown default: the next version has the day.
        Movie {
            added: 0,
            ..film(4, "Zed (1990) 4K")
        },
        Movie {
            added: OCT_15 - 7 * DAY,
            ..film(5, "Zed (1990) 1080p")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let day = |id: i64| {
        index
            .title(index.find(film_id(id)).unwrap())
            .unwrap()
            .added_day
    };
    assert_eq!(day(1), 20_741 - 20);
    assert_eq!(day(4), 20_741 - 7);
}

#[test]
fn a_day_before_1970_or_after_the_last_a_u16_holds_is_clamped() {
    let movies = [
        Movie {
            added: -3 * DAY,
            ..film(1, "Before (2001)")
        },
        Movie {
            added: OCT_15,
            ..film(2, "Today (2001)")
        },
        Movie {
            added: i64::MAX,
            ..film(3, "Forever (2001)")
        },
        // Milliseconds where seconds are meant: the last day, not a wrapped one.
        Movie {
            added: OCT_15 * 1_000,
            ..film(4, "Millis (2001)")
        },
    ];
    let shows = [
        Series {
            last_modified: -DAY,
            ..series(1, "Old")
        },
        Series {
            last_modified: i64::MAX,
            ..series(2, "Never")
        },
    ];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let day = |id: TitleId| index.title(index.find(id).unwrap()).unwrap().added_day;
    assert_eq!(day(film_id(1)), 0);
    assert_eq!(day(film_id(2)), 20_741);
    assert_eq!(day(film_id(3)), u16::MAX);
    assert_eq!(day(film_id(4)), u16::MAX);
    assert_eq!(day(series_id(1)), 0);
    assert_eq!(day(series_id(2)), u16::MAX);
}

#[test]
fn a_series_year_is_its_release_date_before_its_name_and_releasedate_before_release_date() {
    let shows = [
        Series {
            release_date: Some("2006-02-02".into()),
            ..series(1, "Show (2005)")
        },
        Series {
            release_date: Some("n/a".into()),
            ..series(2, "Show Two (2005)")
        },
        Series {
            release_date_camel: Some("2010".into()),
            release_date: Some("2011".into()),
            ..series(3, "Show Three")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    let years: Vec<u16> = (0..3).map(|p| index.title(p).unwrap().year).collect();
    assert_eq!(years, [2006, 2005, 2010]);
}

#[test]
fn a_series_cover_is_trimmed_and_a_blank_one_is_none() {
    let shows = [
        Series {
            cover: Some("  https://x/s.jpg ".into()),
            ..series(1, "A")
        },
        Series {
            cover: Some("   ".into()),
            ..series(2, "B")
        },
        series(3, "C"),
    ];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    let covers: Vec<Option<&str>> = (0..3).map(|p| index.title(p).unwrap().cover).collect();
    assert_eq!(covers, [Some("https://x/s.jpg"), None, None]);
}

#[test]
fn find_answers_for_the_kind_and_the_id_it_was_asked() {
    // Film 2 and 9 only: series 2 is no film 2, and film 5 is between two.
    let movies = [film(2, "Two (2001)"), film(9, "Nine (2001)")];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.find(series_id(2)), None, "film 2 is no series 2");
    assert_eq!(index.find(film_id(5)), None, "film 5 is not film 9");
    assert_eq!(index.find(film_id(10)), None);
    assert!(index.find(film_id(2)).is_some() && index.find(film_id(9)).is_some());
    // And the other way round: series 3 only.
    let shows = [series(3, "Three")];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.find(film_id(3)), None, "series 3 is no film 3");
    assert_eq!(index.find(series_id(2)), None);
    assert_eq!(index.find(series_id(4)), None);
    assert_eq!(index.find(series_id(3)), Some(0));
}

#[test]
fn a_record_is_20_bytes_and_a_text_end_4_and_the_heap_is_counted_exactly() {
    // 1,000 series named "a": per title the record (20), the name and where it
    // ends (1 + 4), the search name and where it ends (1 + 4); the cover table
    // holds the ends only (4).
    let shows: Vec<Series> = (1..=1_000).map(|i| series(i, "a")).collect();
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1_000);
    assert_eq!(index.heap_bytes(), 1_000 * (20 + 5 + 5 + RANK));
    assert_eq!(index.cover_bytes(), 1_000 * 4);
}

#[test]
fn the_heap_counts_the_versions_the_containers_and_the_categories() {
    // 1,000 films of one version and one container each: 20 + 8 a title, and
    // the name and the search name with 4 bytes for where each ends. The names
    // x1 to x1000 are 18 + 270 + 3,600 + 5 = 3,893 bytes.
    let movies: Vec<Movie> = (1..=1_000).map(|i| film(i, &format!("x{i}"))).collect();
    let bare = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(bare.len(), 1_000);
    let counted = 1_000 * (20 + 8 + RANK) + 2 * (3_893 + 4 * 1_000);
    let heap = bare.heap_bytes();
    assert!(
        heap >= counted + 3,
        "{heap} bytes: the container is counted"
    );
    assert!(heap <= counted + 64, "{heap} bytes: nothing else is");
    // Five categories: each a record of its own, and its name byte for byte.
    let heap_with = |len: usize| {
        let cats = Categories {
            vod: (1..=5).map(|i| category(i, &"c".repeat(len))).collect(),
            series: Vec::new(),
        };
        TitleIndex::build(&movies, &[], &cats, MAX_YEAR).heap_bytes()
    };
    let (short, long) = (heap_with(1), heap_with(101));
    assert_eq!(long - short, 5 * 100, "a name is counted byte for byte");
    assert!(short >= heap + 5 * (1 + 16), "each category is a record");
    assert!(short <= heap + 5 * (1 + 64));
}

#[test]
fn the_versions_of_a_dropped_duplicate_leave_no_slack_behind() {
    // Two films of one id and two names are two titles of one id: the first
    // stays. The build had room for both versions; the index keeps one.
    let movies = [
        Movie {
            container_extension: None,
            ..film(5, "A (2001)")
        },
        Movie {
            container_extension: None,
            ..film(5, "B (2002)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.title(0).unwrap().name, "A");
    assert_eq!(index.stats().versions, 1);
    // The record (20), the version (8), the name and the search name (5 + 5).
    assert_eq!(index.heap_bytes(), 20 + 8 + 5 + 5 + RANK);
}

#[test]
fn of_many_titles_with_one_id_the_first_in_the_lists_stays() {
    // 400 series over 50 ids, in an order the sort has to move: for each id the
    // first series of that id stays, whatever the sort does with the others.
    let shows: Vec<Series> = (0..400)
        .map(|i| series((i * 37) % 50 + 1, &format!("Copy {i}")))
        .collect();
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 50);
    for id in 1..=50i64 {
        let first = (0..400i64).find(|i| (i * 37) % 50 + 1 == id).unwrap();
        let t = index.title(index.find(series_id(id)).unwrap()).unwrap();
        assert_eq!(t.name, format!("Copy {first}"), "series {id}");
    }
}

#[test]
fn two_film_titles_whose_default_has_one_id_keep_the_first() {
    let movies = [
        film(5, "Alpha (2001)"),
        film(6, "Gamma (2003)"),
        film(5, "Beta (2002)"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 2);
    let five = index.find(film_id(5)).unwrap();
    assert_eq!(index.title(five).unwrap().name, "Alpha");
    assert_eq!(index.title_of_version(5), Some(five));
}

#[test]
fn the_dashes_and_bars_of_every_spelling_make_one_title() {
    // Ruling 12 as the review of Tasks 2a and 2b changed it, through the index:
    // a hyphen, an en dash, an em dash, the box-drawing bars and the fullwidth
    // bar in front, labels after a bar at the end.
    let movies = [
        film(1, "Frozen II (2019)"),
        film(2, "NL - Frozen II (2019) 1080p"),
        film(3, "NL \u{2013} Frozen II (2019) 720p"),
        film(4, "NL \u{2014} Frozen II (2019) 4K"),
        film(5, "NL \u{2503} Frozen II (2019) HD"),
        film(6, "NL \u{2502} Frozen II (2019) SD"),
        film(7, "Frozen II (2019) 4K \u{ff5c} EN"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1, "seven spellings, one title");
    let frozen = index.title(0).unwrap();
    assert_eq!((frozen.name, frozen.year), ("Frozen II", 2019));
    let ids: Vec<i64> = frozen.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(ids, [4, 7, 2, 3, 5, 1, 6], "by quality, then by id");
    assert!(frozen.dutch && frozen.english);
}

#[test]
fn merge_and_the_index_agree_on_which_ids_fit() {
    // Versions of ids 2^32, 0 and -5 do not fit, 2^32 - 1, 1 and 2 do. merge
    // puts the unfit ones last whatever their quality; the index then finds a
    // default that fits and leaves the three out. Both ask `fit_id`: its line
    // stays at 1 and at u32::MAX, and merge and the index cannot disagree.
    let movies = [
        film(4_294_967_296, "Edge (2001) 4K"),
        film(0, "Edge (2001) 4K HDR"),
        film(-5, "Edge (2001) 2160p"),
        film(4_294_967_295, "Edge (2001) 1080p"),
        film(1, "Edge (2001) 720p"),
        film(2, "Edge (2001) SD"),
    ];
    let order: Vec<i64> = merge(&movies, MAX_YEAR)[0]
        .versions
        .iter()
        .map(|v| movies[v.index as usize].stream_id)
        .collect();
    assert_eq!(&order[..3], [4_294_967_295, 1, 2]);
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    let edge = index
        .title(index.find(film_id(4_294_967_295)).unwrap())
        .unwrap();
    let ids: Vec<i64> = edge.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(ids, [4_294_967_295, 1, 2]);
    assert_eq!(index.stats().skipped, 3);
}

#[test]
fn a_stream_listed_under_two_names_is_a_version_of_a_title_that_lists_it() {
    // Alpha holds the streams 5 and 6, Beta the stream 6 (a portal that lists
    // one stream twice): the answer is one of the two, never none.
    let movies = [
        film(5, "Alpha (2001) 4K"),
        film(6, "Alpha (2001) 1080p"),
        film(6, "Beta (2002)"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 2);
    let found = index.title_of_version(6).unwrap();
    let lists = |pos: usize| {
        let t = index.title(pos).unwrap();
        t.versions.iter().any(|v| v.stream_id == 6)
    };
    assert!(lists(found));
}

#[test]
fn a_huge_name_and_a_huge_cover_are_kept_whole() {
    let big = "Z".repeat(200_000);
    let cover = format!("https://x/{big}.jpg");
    let movies = [Movie {
        stream_icon: Some(cover.clone()),
        ..film(1, &format!("{big} (2001)"))
    }];
    let shows = [Series {
        cover: Some(big.clone()),
        ..series(1, &big)
    }];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 2);
    let s = index.title(0).unwrap();
    assert_eq!(
        (s.name.len(), s.cover.map(str::len)),
        (200_000, Some(200_000))
    );
    let f = index.title(1).unwrap();
    assert_eq!((f.name.len(), f.year), (200_000, 2001));
    assert_eq!(f.cover, Some(cover.as_str()));
    assert!(index.heap_bytes() >= 4 * 200_000);
    assert!(index.cover_bytes() >= 200_000 + cover.len());
}

#[test]
fn learning_races_readers_and_every_read_is_a_whole_value() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    // The index is shared between threads (it is an Arc in the catalog).
    fn shared<T: Send + Sync>() {}
    shared::<TitleIndex>();
    let movies: Vec<Movie> = (1..=500).map(|i| film(i, &format!("Film x{i}"))).collect();
    let index = Arc::new(TitleIndex::build(
        &movies,
        &[],
        &Categories::default(),
        MAX_YEAR,
    ));
    let (stop, passes) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let (index, stop, passes) = (index.clone(), stop.clone(), passes.clone());
            std::thread::spawn(move || {
                let mut first = true;
                while first || !stop.load(Ordering::Relaxed) {
                    for pos in 0..index.len() {
                        let t = index.title(pos).unwrap();
                        // Never half of a value: a year is 0 or the learned one.
                        assert!(t.year == 0 || t.year == 1994, "year {}", t.year);
                        assert!(t.genres == 0 || t.genres == 0x1234, "genres {}", t.genres);
                        assert_eq!(index.find(t.id), Some(pos));
                    }
                    let _ = index.stats();
                    if first {
                        first = false;
                        passes.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    // Learn only once both readers are in their loops.
    let t0 = Instant::now();
    while passes.load(Ordering::SeqCst) < 2 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::yield_now();
    }
    for round in 0..200 {
        for pos in 0..index.len() {
            index.learn(
                pos,
                Learned {
                    year: 1994,
                    genres: 0x1234,
                    trailer: round % 2 == 0,
                },
            );
        }
    }
    stop.store(true, Ordering::Relaxed);
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(passes.load(Ordering::SeqCst), 2, "both readers ran");
    for pos in 0..index.len() {
        let t = index.title(pos).unwrap();
        assert_eq!((t.year, t.genres, t.trailer), (1994, 0x1234, true));
    }
}

#[test]
fn a_portal_with_more_categories_than_a_u16_holds_names_the_first_65534() {
    // A place in the categories is a u16 plus one: the 65,534 first categories
    // can be named, the others are listed but name no title.
    let cats = Categories {
        vod: (1..=70_000).map(|i| category(i, "")).collect(),
        series: Vec::new(),
    };
    let movies: Vec<Movie> = [1, 65_534, 65_535, 65_536, 70_000]
        .iter()
        .map(|&c| Movie {
            category_id: c,
            ..film(c, &format!("Film x{c}"))
        })
        .collect();
    let index = TitleIndex::build(&movies, &[], &cats, MAX_YEAR);
    assert_eq!(index.categories().count(), 70_000);
    let named = |id: i64| {
        let t = index.title(index.find(film_id(id)).unwrap()).unwrap();
        t.category.map(|c| c.id)
    };
    assert_eq!(named(1), Some(1));
    assert_eq!(named(65_534), Some(65_534));
    assert_eq!(named(65_535), None);
    assert_eq!(named(65_536), None);
    assert_eq!(named(70_000), None);
}

#[test]
fn the_stats_tell_dutch_from_english_and_count_the_smallest_values() {
    // Two Dutch series and one English, so that the two counts cannot swap; a
    // rating of one tenth and the first genre alone are still given.
    let first = Genre::ALL[0];
    let shows = [
        Series {
            genre: Some(first.aliases()[0].to_string()),
            rating: 1,
            ..series(1, "A NL")
        },
        series(2, "B NL"),
        series(3, "C EN"),
    ];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    let s = index.stats();
    assert_eq!((s.dutch, s.english), (2, 1));
    assert_eq!((s.with_rating, s.with_genre), (1, 1));
    assert_eq!(index.title(0).unwrap().genres, first.bit());
}

#[test]
fn a_title_is_adult_when_a_version_that_is_not_the_default_is() {
    let movies = [
        film(1, "Late (2020) 4K"),
        Movie {
            is_adult: true,
            ..film(2, "Late (2020) 1080p")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let late = index.title(index.find(film_id(1)).unwrap()).unwrap();
    assert!(
        late.adult,
        "the 1080p copy is adult, the 4K one is the default"
    );
}

#[test]
fn a_series_in_an_english_category_is_english() {
    let cats = Categories {
        vod: Vec::new(),
        series: vec![category(1, "EN | Series"), category(2, "UK Drama")],
    };
    let shows = [
        Series {
            category_id: 1,
            ..series(1, "A")
        },
        Series {
            category_id: 2,
            ..series(2, "B")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &cats, MAX_YEAR);
    for pos in 0..2 {
        let t = index.title(pos).unwrap();
        assert_eq!((t.dutch, t.english), (false, true), "{}", t.name);
    }
}

#[test]
fn learning_no_trailer_sets_none_and_learning_nothing_changes_nothing() {
    let movies = [film(1, "Known (2001)"), film(2, "Open")];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let (known, open) = (
        index.find(film_id(1)).unwrap(),
        index.find(film_id(2)).unwrap(),
    );
    index.learn(open, Learned::default());
    index.learn(
        known,
        Learned {
            year: 1994,
            genres: Genre::Horror.bit(),
            trailer: false,
        },
    );
    let t = index.title(open).unwrap();
    assert_eq!((t.year, t.genres, t.trailer), (0, 0, false));
    let t = index.title(known).unwrap();
    assert_eq!(
        (t.year, t.genres, t.trailer),
        (2001, Genre::Horror.bit(), false)
    );
}

#[test]
fn a_search_name_shorter_than_its_name_leaves_no_slack() {
    // The apostrophe is in the name and not in the search name "oceans", for a
    // series and for a film: each has its record (20), its name and where it ends
    // (7 + 4) and its search name and where it ends (6 + 4); the film also has its
    // one version (8).
    let movies = [Movie {
        container_extension: None,
        ..film(1, "Ocean's (2001)")
    }];
    let shows = [series(1, "Ocean's")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.title(0).unwrap().name, "Ocean's");
    assert_eq!(index.title(1).unwrap().name, "Ocean's");
    assert_eq!(index.heap_bytes(), 2 * (20 + (7 + 4) + (6 + 4 + RANK)) + 8);
}

#[test]
fn a_title_left_out_because_an_earlier_one_has_its_id_is_counted() {
    let shows = [series(7, "First"), series(7, "Second"), series(8, "Third")];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 2);
    assert_eq!(index.stats().skipped, 1);
    let movies = [film(5, "Alpha (2001)"), film(5, "Beta (2002)")];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.stats().skipped, 1);
}

#[test]
fn what_details_learn_is_only_ever_a_genre_there_is() {
    let all = (1u32 << Genre::ALL.len()) - 1;
    let movies = [film(1, "A (2001)"), film(2, "B (2002)")];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    index.learn(
        0,
        Learned {
            genres: u32::MAX,
            ..Learned::default()
        },
    );
    assert_eq!(index.title(0).unwrap().genres, all);
    // A mask of bits past the genres names none.
    index.learn(
        1,
        Learned {
            genres: 1 << 30,
            ..Learned::default()
        },
    );
    assert_eq!(index.title(1).unwrap().genres, 0);
}

#[test]
fn a_stream_that_is_a_titles_default_belongs_to_that_title() {
    // Stream 6 is the second version of Alpha and the only one of Beta, whose
    // id it is (a portal that lists one stream under two names).
    let movies = [
        film(5, "Alpha (2001) 4K"),
        film(6, "Alpha (2001) 1080p"),
        film(6, "Beta (2002)"),
    ];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    let beta = index.find(film_id(6)).unwrap();
    assert_eq!(index.title(beta).unwrap().name, "Beta");
    assert_eq!(index.title_of_version(6), Some(beta));
    assert_eq!(index.title_of_version(5), index.find(film_id(5)));
}
