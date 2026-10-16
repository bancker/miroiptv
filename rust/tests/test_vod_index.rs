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
