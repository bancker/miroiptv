//! The thuisbioscoop's timing test (spec 2026-10-15 §5, "Een tijdtest in
//! release"): a made-up catalog of Pandora's size, 47,188 films and 10,589
//! series beside 13,000 live channels. It times the title index's build as the
//! catalog makes it, from the lists it keeps and what they gave up beside them
//! (`Extras`), prints what the build holds on the heap while it runs and what
//! it keeps, the index's size, the covers' and the list records', times the
//! parse of the film and series lists as Pandora sends them, and per keystroke
//! the index's search with filters and counts next to the search of `f` on the
//! same catalog. It asserts no speed (a busy machine would fail it): the
//! numbers go to the user. `test_f_timing.rs` times `f` alone, before phase 1
//! and after.
//!
//! `cargo test --release --test test_vod_timing -- --ignored --nocapture`

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::CatalogStore;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, Categories, Category, LiveChannel, Movie, Portal, Series};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::{Extras, Query, Sort, TitleIndex, Years};
use bongo_player::vod::Kind;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;

/// This test's heap, counted while `COUNTING`: what the build of the index
/// holds while it runs, and what it keeps.
struct Counted;

static COUNTING: AtomicBool = AtomicBool::new(false);
static HELD: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

fn count(bytes: isize) {
    if COUNTING.load(Relaxed) {
        let held = HELD.fetch_add(bytes, Relaxed) + bytes;
        PEAK.fetch_max(held, Relaxed);
    }
}

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            count(layout.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        count(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = System.realloc(p, layout, size);
        if !moved.is_null() {
            count(size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static HEAP: Counted = Counted;

/// What `work` made, the most it held on the heap beyond what was there before
/// it ran, and what it left there, in bytes.
fn heap_of<T>(work: impl FnOnce() -> T) -> (T, isize, isize) {
    HELD.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    let made = work();
    COUNTING.store(false, Relaxed);
    (made, PEAK.load(Relaxed), HELD.load(Relaxed))
}

const FILMS: usize = 47_188;
const SERIES: usize = 10_589;
const LIVE: usize = 13_000;
const MAX_YEAR: u16 = 2027;

/// Words for made-up titles: with the numbers, about as long as a portal's.
const WORDS: [&str; 24] = [
    "the", "last", "night", "of", "dark", "matrix", "love", "story", "dead", "city", "secret",
    "man", "woman", "king", "lost", "house", "blood", "river", "star", "war", "pokémon", "zomer",
    "liefde", "nacht",
];

/// A number from `i`, the same on every run.
fn mix(i: usize) -> usize {
    let mut x = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 29;
    (x.wrapping_mul(0xbf58_476d_1ce4_e5b9) >> 32) as usize
}

fn title_of(i: usize) -> String {
    let n = 2 + mix(i) % 3;
    let words: Vec<&str> = (0..n)
        .map(|k| WORDS[mix(i * 7 + k) % WORDS.len()])
        .collect();
    let mut t = words.join(" ");
    if let Some(first) = t.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!("{t} {}", i % 997)
}

fn cover(i: usize) -> Option<String> {
    (!mix(i).is_multiple_of(10)).then(|| {
        format!(
            "https://image.tmdb.org/t/p/w600_and_h900_bestv2/{:027x}.jpg",
            mix(i)
        )
    })
}

/// Pandora-like films: a prefix, a title, a year and a label; one in ten is
/// another version of the film before it.
fn films() -> Vec<Movie> {
    let prefixes = ["", "NL | ", "EN | ", "4K | "];
    let labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"];
    (0..FILMS)
        .map(|i| {
            let base = if i % 10 == 9 { i - 1 } else { i };
            Movie {
                stream_id: 100_000 + i as i64,
                name: format!(
                    "{}{} ({}){}",
                    prefixes[mix(base) % 4],
                    title_of(base),
                    1950 + mix(base) % 77,
                    labels[mix(i + 3) % labels.len()]
                ),
                container_extension: Some(["mkv", "mp4"][i % 2].into()),
                is_adult: i % 97 == 0,
                stream_icon: cover(i),
                rating: (mix(i) % 101) as u8,
                added: 1_700_000_000 + (mix(i) % 90_000_000) as i64,
                category_id: 1 + (mix(i) % 200) as i64,
            }
        })
        .collect()
}

fn shows() -> Vec<Series> {
    let genres = [
        "Drama",
        "Comedy",
        "Crime, Drama",
        "Sci-Fi & Fantasy",
        "Kids",
        "Reality",
        "",
    ];
    (0..SERIES)
        .map(|i| Series {
            series_id: 1 + i as i64,
            name: title_of(FILMS + i),
            is_adult: i % 89 == 0,
            cover: cover(FILMS + i),
            rating: (mix(i + 11) % 101) as u8,
            release_date_camel: Some(format!("{}-05-01", 1990 + mix(i) % 36)),
            genre: Some(genres[mix(i) % genres.len()].into()),
            category_id: 1 + (mix(i) % 100) as i64,
            last_modified: 1_700_000_000 + (mix(i) % 90_000_000) as i64,
            ..Series::default()
        })
        .collect()
}

fn categories() -> Categories {
    let names = [
        "NL | Komedie",
        "EN | Action",
        "NL | Kids",
        "4K Films",
        "Nieuw",
        "EN | Drama",
        "Horror",
        "Documentaire",
    ];
    let cat = |i: usize| Category {
        category_id: 1 + i as i64,
        category_name: format!("{} {}", names[i % names.len()], i),
    };
    Categories {
        vod: (0..200).map(cat).collect(),
        series: (0..100).map(cat).collect(),
    }
}

fn live() -> Vec<LiveChannel> {
    (0..LIVE)
        .map(|i| LiveChannel {
            stream_id: i as i64,
            name: format!("NL | Kanaal {i} HD"),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        })
        .collect()
}

/// The film list as Pandora sends it (`get_vod_streams`): every value a
/// string, and the fields the app does not keep beside those it does.
fn film_list_body(movies: &[Movie]) -> String {
    let list: Vec<serde_json::Value> = movies
        .iter()
        .enumerate()
        .map(|(i, m)| {
            serde_json::json!({
                "num": i + 1,
                "name": m.name,
                "stream_type": "movie",
                "stream_id": m.stream_id.to_string(),
                "stream_icon": m.stream_icon.clone().unwrap_or_default(),
                "rating": format!("{:.1}", f64::from(m.rating) / 10.0),
                "rating_5based": f64::from(m.rating) / 20.0,
                "added": m.added.to_string(),
                "is_adult": if m.is_adult { "1" } else { "0" },
                "category_id": m.category_id.to_string(),
                "container_extension": m.container_extension.clone().unwrap_or_default(),
                "custom_sid": "",
                "direct_source": ""
            })
        })
        .collect();
    serde_json::to_string(&list).unwrap()
}

/// The series list as Pandora sends it (`get_series`), with the plot, cast and
/// backdrops the app does not keep.
fn series_list_body(series: &[Series]) -> String {
    let list: Vec<serde_json::Value> = series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cover = s.cover.clone().unwrap_or_default();
            serde_json::json!({
                "num": i + 1,
                "name": s.name,
                "series_id": s.series_id,
                "cover": cover,
                "plot": "A family's secrets come out when a child goes missing in a small town.",
                "cast": "An Actor, Another Actor, A Third Actor",
                "director": "A Director",
                "genre": s.genre.clone().unwrap_or_default(),
                "releaseDate": s.release_date_camel.clone().unwrap_or_default(),
                "last_modified": s.last_modified.to_string(),
                "rating": format!("{:.1}", f64::from(s.rating) / 10.0),
                "rating_5based": f64::from(s.rating) / 20.0,
                "backdrop_path": [cover],
                "youtube_trailer": "",
                "episode_run_time": "45",
                "category_id": s.category_id.to_string()
            })
        })
        .collect();
    serde_json::to_string(&list).unwrap()
}

/// `work` timed `n` times: the first, best, mean and worst, in ms.
fn time(n: usize, mut work: impl FnMut()) -> (f64, f64, f64, f64) {
    let ms: Vec<f64> = (0..n)
        .map(|_| {
            let t0 = Instant::now();
            work();
            t0.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let best = ms.iter().copied().fold(f64::MAX, f64::min);
    let worst = ms.iter().copied().fold(0.0, f64::max);
    (ms[0], best, ms.iter().sum::<f64>() / n as f64, worst)
}

#[test]
#[ignore]
fn a_catalog_of_pandoras_size_timed() {
    let (movies, series, categories) = (films(), shows(), categories());
    // The lists as the catalog keeps them, and what they give up for the index
    // beside them, as `catalog::StoredCatalog::new` takes it out.
    let (mut kept_movies, mut kept_series) = (movies.clone(), series.clone());
    let (extras, _, given_up) = heap_of(|| Extras::take(&mut kept_movies, &mut kept_series));
    let build = || {
        TitleIndex::build_with(
            black_box(&kept_movies),
            black_box(&kept_series),
            &extras,
            &categories,
            MAX_YEAR,
        )
    };
    let (first, best, mean, worst) = time(5, || {
        black_box(build());
    });
    let (index, peak, kept) = heap_of(build);
    let s = index.stats();
    println!(
        "index build: {} titles ({} series, {} films from {} versions): first {first:.1} ms, best {best:.1} ms, mean {mean:.1} ms, worst {worst:.1} ms",
        index.len(),
        s.series,
        s.films,
        s.versions
    );
    println!(
        "index build memory: {:.2} MB at its peak, {:.2} MB kept (the index), {:.2} MB passing (its own working lists); no list copied",
        peak as f64 / 1e6,
        kept as f64 / 1e6,
        (peak - kept) as f64 / 1e6
    );
    println!(
        "what the lists give up for it: {:.2} MB of handles (the strings move), held from the catalog's put until its titles are built",
        given_up as f64 / 1e6
    );
    println!(
        "index size: {:.2} MB (budget 5 MB), covers beside it {:.2} MB",
        index.heap_bytes() as f64 / 1e6,
        index.cover_bytes() as f64 / 1e6
    );
    println!(
        "list records: Movie {} B x {FILMS}, Series {} B x {SERIES} = {:.2} MB",
        std::mem::size_of::<Movie>(),
        std::mem::size_of::<Series>(),
        (std::mem::size_of::<Movie>() * FILMS + std::mem::size_of::<Series>() * SERIES) as f64
            / 1e6
    );
    let (film_body, series_body) = (film_list_body(&movies), series_list_body(&series));
    let (_, film_ms, _, _) = time(5, || {
        let parsed: Vec<Movie> = serde_json::from_slice(black_box(film_body.as_bytes())).unwrap();
        black_box(parsed);
    });
    let (_, series_ms, _, _) = time(5, || {
        let parsed: Vec<Series> =
            serde_json::from_slice(black_box(series_body.as_bytes())).unwrap();
        black_box(parsed);
    });
    println!(
        "film list parse: {film_ms:.1} ms, series list parse: {series_ms:.1} ms (on the fetch's worker, as before)"
    );

    let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }));
    let store = CatalogStore::from_catalog(
        portal,
        Catalog {
            live: live(),
            movies: movies.clone(),
            series: series.clone(),
        },
    );
    let filters = Query {
        kind: Some(Kind::Film),
        genres: Genre::Comedy.bit() | Genre::Action.bit(),
        years: Years::Since2020.bit() | Years::The2010s.bit(),
        sort: Sort::Rating,
        ..Query::default()
    };
    println!("keystroke      | index: text      | index: + filters | f today");
    let typed = "the matrix pokemon";
    let (mut worst_index, mut worst_f) = (0.0f64, 0.0f64);
    for end in 1..=typed.len() {
        let text = &typed[..end];
        let plain = Query {
            text: text.into(),
            ..Query::default()
        };
        let filtered = Query {
            text: text.into(),
            ..filters.clone()
        };
        let mut n_plain = 0;
        let (_, plain_ms, _, _) = time(5, || {
            n_plain = black_box(index.search(&plain, false)).titles.len()
        });
        let mut n_filtered = 0;
        let (_, filtered_ms, _, _) = time(5, || {
            n_filtered = black_box(index.search(&filtered, false)).titles.len()
        });
        let mut n_f = 0;
        let (_, f_ms, _, _) = time(5, || n_f = black_box(store.search(text)).len());
        worst_index = worst_index.max(plain_ms).max(filtered_ms);
        worst_f = worst_f.max(f_ms);
        println!(
            "{:<14} | {plain_ms:6.2} ms {n_plain:>6} | {filtered_ms:6.2} ms {n_filtered:>6} | {f_ms:6.2} ms {n_f:>3}",
            format!("{text:?}")
        );
    }
    println!("worst keystroke: index {worst_index:.2} ms (budget 16 ms, and no slower than f), f {worst_f:.2} ms");
}
