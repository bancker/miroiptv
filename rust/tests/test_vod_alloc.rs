//! Ruling 17: `parse_name` allocates nothing once it is warm (it reuses a
//! thread-local scratch), and `fold_into` into a `String` with room allocates
//! nothing either. A catalog has ~58,000 names, so one `to_string()` in
//! `parse_name` would cost 58,000 allocations a build and pass every other test.
//!
//! This binary holds this one test only, so that no other test thread allocates
//! while the measured pass runs, and the counter is a global one (a
//! `thread_local!` can allocate on its first access and recurse into the
//! allocator).

use bongo_player::vod::fold_into;
use bongo_player::vod::genres::parse_name;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The system allocator, counting the requests made while `ARMED`.
struct Counting;

static ARMED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);

fn note() {
    if ARMED.load(Ordering::SeqCst) {
        COUNT.fetch_add(1, Ordering::SeqCst);
    }
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

/// How often `f` asks the allocator for memory.
fn allocations(f: impl FnOnce()) -> usize {
    COUNT.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    f();
    ARMED.store(false, Ordering::SeqCst);
    COUNT.load(Ordering::SeqCst)
}

/// The shapes a portal names a film in, `{b}` its title and `{y}` its year: the
/// review's matrix of what a name can hold (prefixes, groups in front, labels
/// and bars at the end), and the names that look like them and are not.
const SHAPES: &[&str] = &[
    "{b}",
    "{b} ({y})",
    "{b} {y}",
    "NL | {b} ({y})",
    "|NL| {b} ({y})",
    "NL - {b} ({y})",
    "4K - {b} ({y})",
    "NL - 4K - {b} ({y})",
    "[NL] {b} ({y})",
    "[NL][4K] {b} ({y})",
    "[NL] {b} ({y}) [4K]",
    "(NL) {b} ({y})",
    "[NL] - {b} ({y})",
    "[NL] {b}",
    "{b} ({y}) [4K]",
    "{b} ({y}) [NL]",
    "{b} ({y}) NL SUBS",
    "{b} ({y}) 1080p NL Gesproken",
    "{b} ({y}) - NL",
    "{b} ({y}) | NL",
    "{b} ({y}) 4K | NL",
    "{b} ({y}) | 4K | NL",
    "{b} | NL | 4K",
    "NL | {b} ({y}) | 4K",
    "NL | 4K | {b} ({y})",
    "NL | {b}",
    "{b} [4K | NL]",
    "NL | Dual (2022)",
    "[REC] (2007)",
    "Mission: Impossible - Fallout (2018)",
    "Ocean's Eleven (2001)",
    "Don\u{2019}t Look Up (2021) [NL]",
    "Amélie\u{a0}(2001)",
];

/// The names the test reads: every shape, with every spelling of its bars and
/// dashes, some odd ones, and names of 10 KB that make every loop run long.
fn all_names() -> Vec<String> {
    let mut names = Vec::new();
    for shape in SHAPES {
        let name = shape.replace("{b}", "Frozen II").replace("{y}", "2019");
        for bar in ["\u{ff5c}", "\u{2503}", "\u{2502}"] {
            if name.contains('|') {
                names.push(name.replace('|', bar));
            }
        }
        for dash in ["\u{2013}", "\u{2014}"] {
            if name.contains(" - ") {
                names.push(name.replace(" - ", &format!(" {dash} ")));
            }
        }
        names.push(name);
    }
    for odd in [
        "",
        "  ",
        "|",
        "NL |",
        "(2001)",
        "((()))",
        "[4K Film",
        "Title \u{2013}",
        "🎬 Film 🎬 (2020)",
        "A\u{301}",
    ] {
        names.push(odd.to_string());
    }
    names.push("word ".repeat(2_000) + "(2001) [4K]");
    names.push("(".repeat(10_000));
    names.push("NL | ".repeat(2_000) + "Frozen II");
    names.push("Frozen II (2019)".to_string() + &" | NL".repeat(2_000));
    names.push("[NL]".repeat(2_500) + " Frozen II");
    names.push("NL - ".repeat(2_000) + "Frozen II");
    names
}

#[test]
fn parse_name_and_fold_into_allocate_nothing_once_warm() {
    let names = all_names();
    // The first pass makes the scratch as big as these names need.
    for name in &names {
        black_box(parse_name(name, 2027));
    }
    let parsing = allocations(|| {
        for name in &names {
            black_box(parse_name(name, 2027));
        }
    });

    let mut out = String::new();
    for name in &names {
        out.clear();
        fold_into(name, &mut out);
    }
    let folding = allocations(|| {
        for name in &names {
            out.clear();
            fold_into(name, &mut out);
            black_box(&out);
        }
    });

    assert_eq!(
        parsing,
        0,
        "parse_name asked the allocator {parsing} times for {} warm names",
        names.len()
    );
    assert_eq!(
        folding,
        0,
        "fold_into asked the allocator {folding} times for {} names",
        names.len()
    );
}
