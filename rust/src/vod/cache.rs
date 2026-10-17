//! The title index in a file (spec 2026-10-15 §4.6), so the next start has
//! the titles before the fresh lists come: `config\cache\vod-<host>_<port>.json`.
//! No adult title goes in (as the EPG cache keeps no adult channel), and no
//! credential: a cover address that `redact` would change holds an account,
//! and one with a query may hold a token that `redact` does not know; both are
//! left out. The catalog writes the file after every build of its titles and
//! reads it once, after the first frame, both on the blocking pool. A file that
//! cannot be read is ignored: the next load writes a new one. What a file says
//! is checked as it is read: a genre bit that no genre has, or a category it
//! does not list, names nothing.

use super::genres::{Genre, Labels};
use super::index::{category_rec, CategoryIn, Entry, Title, TitleIndex, VersionIn};
use super::{write_atomic_with, Kind};
use crate::args::XtreamCreds;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

/// The file as it is read.
#[derive(Deserialize)]
struct CacheFile {
    version: u32,
    /// When it was written: unix seconds.
    saved_at: i64,
    /// `[film, id, name]`: films' categories and series' ones are two id spaces.
    categories: Vec<(bool, i64, String)>,
    titles: Vec<CachedTitle>,
}

/// A title as written: `[film, id, name, cover, year, rating, genres,
/// category, added, dutch, english, trailer, [[stream id, labels, container]]]`.
/// `category` is a place in `categories` plus one; 0 for none.
#[derive(Deserialize)]
struct CachedTitle(
    bool,
    u32,
    String,
    String,
    u16,
    u8,
    u32,
    u16,
    u16,
    bool,
    bool,
    bool,
    Vec<(u32, u16, String)>,
);

/// The file as it is written: borrowed from the index, so no copy of its text
/// is made on the way (`save`).
#[derive(Serialize)]
struct CacheFileRef<'a> {
    version: u32,
    saved_at: i64,
    categories: Vec<(bool, i64, &'a str)>,
    titles: TitlesRef<'a>,
}

/// A title as written ([`CachedTitle`]'s array), borrowed from the index.
#[derive(Serialize)]
struct CachedTitleRef<'a>(
    bool,
    u32,
    &'a str,
    &'a str,
    u16,
    u8,
    u32,
    u16,
    u16,
    bool,
    bool,
    bool,
    Vec<(u32, u16, &'a str)>,
);

/// The titles of an index that go into the file, written one by one: no list
/// of them is made.
struct TitlesRef<'a>(&'a TitleIndex);

impl Serialize for TitlesRef<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(kept(self.0).map(|t| cached(self.0, t)))
    }
}

/// The titles of `index` that go into the file: no adult one.
fn kept(index: &TitleIndex) -> impl Iterator<Item = Title<'_>> {
    (0..index.len())
        .filter_map(|pos| index.title(pos))
        .filter(|t| !t.adult)
}

/// Title `t` of `index` as written. A cover address that holds an account, or
/// a query (`redact` sees no token there), is left out: its tile shows the
/// placeholder until the fresh lists come.
fn cached<'a>(index: &TitleIndex, t: Title<'a>) -> CachedTitleRef<'a> {
    let cover = t
        .cover
        .filter(|c| !c.contains('?') && crate::redact::redact(c) == *c)
        .unwrap_or_default();
    CachedTitleRef(
        t.id.kind == Kind::Film,
        t.id.id as u32,
        t.name,
        cover,
        t.year,
        t.rating,
        t.genres,
        index.category_code(t.pos),
        t.added_day,
        t.dutch,
        t.english,
        t.trailer,
        t.versions
            .iter()
            .map(|v| (v.stream_id as u32, v.labels.0, v.ext))
            .collect(),
    )
}

/// The portal at `host`:`port` as a file or folder name: `<host>_<port>`,
/// every character but letters, digits, '.' and '-' turned into '_' (as the
/// EPG cache names its file). The user name and password never go into it.
pub fn portal_key(host: &str, port: u16) -> String {
    let host: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{host}_{port}")
}

/// The index cache of the portal with these credentials, in `cache_dir`; none
/// without a portal (an M3U list keeps no titles).
pub fn path_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf> {
    let c = creds?;
    Some(cache_dir.join(format!("vod-{}.json", portal_key(&c.host, c.port))))
}

/// Writes the titles of `index` to `path` at `saved_at` (unix seconds), as
/// they are serialized, through a temporary file renamed over it
/// (`write_atomic_with`), so a write that fails leaves the old file whole.
/// Adult titles stay out, and so does a cover address that holds an account
/// or a query. How many titles went in and the file's size.
pub fn save(path: &Path, index: &TitleIndex, saved_at: i64) -> std::io::Result<(usize, u64)> {
    let file = CacheFileRef {
        version: VERSION,
        saved_at,
        categories: index
            .categories()
            .map(|c| (c.kind == Kind::Film, c.id, c.name))
            .collect(),
        titles: TitlesRef(index),
    };
    write_atomic_with(path, |out| {
        serde_json::to_writer(out, &file).map_err(std::io::Error::other)
    })?;
    let bytes = std::fs::metadata(path)?.len();
    Ok((kept(index).count(), bytes))
}

/// The titles in the file at `path` and when it was written; none when there
/// is no file. A file that is no index cache of this version is an error (the
/// caller logs it and goes on without). Ids are read as `u32`, the file's own
/// type, and handed over as they are: `TitleIndex::assemble` is their fence (a
/// title or a version whose id is 0 goes out, and so does a film left with no
/// version to play; `Stats::skipped` counts them). The other values that index
/// or bound something are masked here (Ruling 51).
pub fn load(path: &Path) -> Result<Option<(TitleIndex, i64)>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let file: CacheFile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("version {}", file.version));
    }
    let categories = file
        .categories
        .iter()
        .map(|(film, id, name)| {
            category_rec(&CategoryIn {
                kind: if *film { Kind::Film } else { Kind::Series },
                id: *id,
                name,
            })
        })
        .collect::<Vec<_>>();
    let known = categories.len();
    let mut pool: Vec<VersionIn<'_>> = Vec::new();
    let mut entries = Vec::with_capacity(file.titles.len());
    for t in &file.titles {
        let first = pool.len() as u32;
        pool.extend(
            t.12.iter()
                .map(|(sid, labels, ext)| (*sid, Labels(*labels), ext.as_str())),
        );
        entries.push(Entry {
            kind: if t.0 { Kind::Film } else { Kind::Series },
            id: t.1,
            name: &t.2,
            key: None,
            cover: &t.3,
            year: t.4,
            rating: t.5.min(100),
            genres: t.6 & ((1 << Genre::ALL.len()) - 1), // stray bits of a file name no genre
            // A place past the file's categories names none.
            category: if usize::from(t.7) <= known { t.7 } else { 0 },
            added: t.8,
            dutch: t.9,
            english: t.10,
            adult: false,
            trailer: t.11,
            versions: first..pool.len() as u32,
        });
    }
    Ok(Some((
        TitleIndex::assemble(entries, &pool, categories),
        file.saved_at,
    )))
}
