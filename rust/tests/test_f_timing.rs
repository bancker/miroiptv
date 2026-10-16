//! `f` before the thuisbioscoop and after it (spec 2026-10-15 §2: "ik wil
//! 100% zeker zijn dat het even snel is als optie f nu"): the search of `f` per
//! keystroke on a made-up catalog of Pandora's size, 13,000 live channels,
//! 47,188 films and 10,589 series, read from JSON in 0.6.1's fields only, so
//! this test is the same before phase 1 and after it. The names are those of
//! `test_vod_timing.rs`. It asserts no speed (a busy machine would fail it):
//! the numbers go to the user.
//!
//! `cargo test --release --test test_f_timing -- --ignored --nocapture`

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::CatalogStore;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, Portal};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

const FILMS: usize = 47_188;
const SERIES: usize = 10_589;
const LIVE: usize = 13_000;

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

/// The catalog, read from the lists as JSON with 0.6.1's fields: what `f`
/// searched before phase 1 kept more of them.
fn catalog() -> Catalog {
    let prefixes = ["", "NL | ", "EN | ", "4K | "];
    let labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"];
    let flag = |adult: bool| if adult { "1" } else { "0" };
    let films: Vec<serde_json::Value> = (0..FILMS)
        .map(|i| {
            let base = if i % 10 == 9 { i - 1 } else { i };
            let name = format!(
                "{}{} ({}){}",
                prefixes[mix(base) % 4],
                title_of(base),
                1950 + mix(base) % 77,
                labels[mix(i + 3) % labels.len()]
            );
            let ext = ["mkv", "mp4"][i % 2];
            serde_json::json!({
                "stream_id": 100_000 + i,
                "name": name,
                "container_extension": ext,
                "is_adult": flag(i % 97 == 0)
            })
        })
        .collect();
    let series: Vec<serde_json::Value> = (0..SERIES)
        .map(|i| {
            serde_json::json!({
                "series_id": 1 + i,
                "name": title_of(FILMS + i),
                "is_adult": flag(i % 89 == 0)
            })
        })
        .collect();
    let live: Vec<serde_json::Value> = (0..LIVE)
        .map(|i| {
            serde_json::json!({
                "stream_id": i,
                "name": format!("NL | Kanaal {i} HD"),
                "tv_archive": 0,
                "is_adult": "0"
            })
        })
        .collect();
    Catalog {
        live: serde_json::from_value(serde_json::Value::Array(live)).unwrap(),
        movies: serde_json::from_value(serde_json::Value::Array(films)).unwrap(),
        series: serde_json::from_value(serde_json::Value::Array(series)).unwrap(),
    }
}

#[test]
#[ignore]
fn f_on_a_catalog_of_pandoras_size_timed() {
    let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }));
    let store = CatalogStore::from_catalog(portal, catalog());
    println!("keystroke      | f (best of 5)");
    let typed = "the matrix pokemon";
    let mut worst = 0.0f64;
    for end in 1..=typed.len() {
        let text = &typed[..end];
        let mut found = 0;
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            found = black_box(store.search(text)).len();
            best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        }
        worst = worst.max(best);
        println!("{:<14} | {best:6.2} ms {found:>3}", format!("{text:?}"));
    }
    println!("worst keystroke: f {worst:.2} ms");
}
