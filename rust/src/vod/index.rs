//! The title index (spec 2026-10-15 §4.2): every series and film title of a
//! portal in a small record, with its name, search name and cover in shared
//! text tables. Built once per catalog from its lists, off the screen; pure,
//! no I/O. The catalog hands it out behind its adult lock (`catalog::Titles`):
//! every query here takes `unlocked`, and nothing else filters.
//!
//! Records are kept in (kind, id) order, so a title is found by a binary
//! search. Ids are kept as `u32`: Xtream ids are positive (the portal's
//! unreadable one is 0) and far below 4·10⁹; a title, a version or a series
//! whose id is not 1 to `u32::MAX` is left out and counted.

use super::genres::{category_labels, genres_in, parse_name, Genre, Labels, Language};
use super::versions::merge;
use super::{fit_id, fold_into, Kind, TitleId};
use crate::portal::{Categories, Movie, Series};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

const FLAG_FILM: u8 = 1;
const FLAG_ADULT: u8 = 1 << 1;
const FLAG_TRAILER: u8 = 1 << 2;
const FLAG_DUTCH: u8 = 1 << 3;
const FLAG_ENGLISH: u8 = 1 << 4;

/// Texts one after the other, and where each ends.
#[derive(Default)]
pub(crate) struct TextTable {
    text: String,
    ends: Vec<u32>,
}

impl TextTable {
    fn with_capacity(texts: usize, bytes: usize) -> Self {
        Self {
            text: String::with_capacity(bytes),
            ends: Vec::with_capacity(texts),
        }
    }

    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.ends.push(self.text.len() as u32);
    }

    pub(crate) fn get(&self, i: usize) -> &str {
        let start = match i {
            0 => 0,
            _ => self.ends[i - 1] as usize,
        };
        &self.text[start..self.ends[i] as usize]
    }

    fn heap_bytes(&self) -> usize {
        self.text.capacity() + self.ends.capacity() * 4
    }

    fn shrink(&mut self) {
        self.text.shrink_to_fit();
        self.ends.shrink_to_fit();
    }
}

/// One title: 20 bytes. What the details may add later (a year, genres, the
/// trailer) is atomic, so the shared index learns it in place (`learn`).
struct Record {
    id: u32,
    genres: AtomicU32,
    /// Where its versions end in `versions` (they start where the record
    /// before's end); a series has none.
    versions_end: u32,
    year: AtomicU16,
    /// The day the portal added it (unix days); 0: unknown.
    added: u16,
    /// Its category: a position in `categories` plus one; 0: none.
    category: u16,
    rating: u8,
    flags: AtomicU8,
}

/// A film's version as kept: 8 bytes.
#[derive(Debug, Clone, Copy)]
struct VersionRec {
    stream_id: u32,
    labels: u16,
    /// Its container: a position in `exts` plus one; 0: none.
    ext: u8,
}

/// A portal category as kept, with what its name says.
pub(crate) struct CategoryRec {
    pub(crate) kind: Kind,
    pub(crate) id: i64,
    pub(crate) name: Box<str>,
    genres: u32,
    language: Language,
}

/// A version of a film, as a page offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version<'a> {
    pub stream_id: i64,
    pub labels: Labels,
    /// Its container ("mkv"); empty when the portal named none.
    pub ext: &'a str,
}

/// A film's versions, the default first.
#[derive(Debug, Clone, Copy)]
pub struct Versions<'a> {
    recs: &'a [VersionRec],
    exts: &'a [Box<str>],
}

impl<'a> Versions<'a> {
    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<Version<'a>> {
        let r = self.recs.get(i)?;
        Some(Version {
            stream_id: i64::from(r.stream_id),
            labels: Labels(r.labels),
            ext: match r.ext {
                0 => "",
                n => &self.exts[n as usize - 1],
            },
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Version<'a>> + 'a {
        let me = *self;
        (0..me.recs.len()).filter_map(move |i| me.get(i))
    }
}

/// A portal category, as a title names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryInfo<'a> {
    pub kind: Kind,
    pub id: i64,
    pub name: &'a str,
}

/// A title as the index knows it. `Debug` for the tests only: its cover's
/// address may hold the account.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
pub struct Title<'a> {
    /// Its position in the index.
    pub pos: usize,
    pub id: TitleId,
    pub name: &'a str,
    /// Its cover's address; none when the lists gave none.
    pub cover: Option<&'a str>,
    /// 0: unknown.
    pub year: u16,
    /// ★ in tenths; 0: none.
    pub rating: u8,
    /// A mask of `Genre::bit`s.
    pub genres: u32,
    pub category: Option<CategoryInfo<'a>>,
    /// The day the portal added it (unix days); 0: unknown.
    pub added_day: u16,
    /// A version, or the series, is spoken Dutch; English.
    pub dutch: bool,
    pub english: bool,
    pub adult: bool,
    /// Its details named a trailer.
    pub trailer: bool,
    /// A film's versions, the default first; a series has none.
    pub versions: Versions<'a>,
}

/// What a title's details tell the index (`TitleIndex::learn`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Learned {
    pub year: u16,
    pub genres: u32,
    pub trailer: bool,
}

/// How the lists' titles came out, for the catalog's log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub series: usize,
    pub films: usize,
    pub versions: usize,
    pub with_cover: usize,
    pub with_rating: usize,
    pub with_year: usize,
    pub with_genre: usize,
    pub dutch: usize,
    pub english: usize,
    /// Left out: a title (a film's default version, a series) or a version whose
    /// id is not 1 to `u32::MAX`, and a title whose id an earlier title of the
    /// same kind already has. A film whose default does not fit counts once, for
    /// the whole title.
    pub skipped: usize,
}

/// Every title of one catalog.
pub struct TitleIndex {
    records: Vec<Record>,
    names: TextTable,
    keys: TextTable,
    covers: TextTable,
    versions: Vec<VersionRec>,
    exts: Vec<Box<str>>,
    categories: Vec<CategoryRec>,
    skipped: usize,
}

/// A title on its way into the index (the lists' build and the cache's).
pub(crate) struct Entry<'a> {
    pub(crate) kind: Kind,
    pub(crate) id: u32,
    pub(crate) name: &'a str,
    /// Its folded name; built here when none is given.
    pub(crate) key: Option<String>,
    pub(crate) cover: &'a str,
    pub(crate) year: u16,
    pub(crate) rating: u8,
    pub(crate) added: u16,
    /// A position in the categories plus one; 0: none.
    pub(crate) category: u16,
    pub(crate) genres: u32,
    pub(crate) dutch: bool,
    pub(crate) english: bool,
    pub(crate) adult: bool,
    pub(crate) trailer: bool,
    /// Its versions: a range of the versions `assemble` is handed.
    pub(crate) versions: std::ops::Range<u32>,
}

/// A film's version on its way into the index: stream id, labels, container.
pub(crate) type VersionIn<'a> = (u32, Labels, &'a str);

/// A category on its way into the index.
pub(crate) struct CategoryIn<'a> {
    pub(crate) kind: Kind,
    pub(crate) id: i64,
    pub(crate) name: &'a str,
}

/// Unix seconds as a unix day; 0 for none.
fn day_of(secs: i64) -> u16 {
    (secs.max(0) / 86_400).min(u16::MAX as i64) as u16
}

/// The bits of the genres there are: what `learn` lets in.
const GENRE_BITS: u32 = (1 << Genre::ALL.len()) - 1;

impl TitleIndex {
    /// The titles of these lists. `max_year` is the latest year a name may
    /// give: the year after this one.
    pub fn build(
        movies: &[Movie],
        series: &[Series],
        categories: &Categories,
        max_year: u16,
    ) -> Self {
        let cats: Vec<CategoryIn<'_>> = categories
            .vod
            .iter()
            .map(|c| (Kind::Film, c))
            .chain(categories.series.iter().map(|c| (Kind::Series, c)))
            .map(|(kind, c)| CategoryIn {
                kind,
                id: c.category_id,
                name: &c.category_name,
            })
            .collect();
        let cat_recs: Vec<CategoryRec> = cats.iter().map(category_rec).collect();
        // A category id, per kind, to its position plus one.
        let mut place: HashMap<(Kind, i64), u16> = HashMap::with_capacity(cats.len());
        for (i, c) in cats.iter().enumerate().take(u16::MAX as usize - 1) {
            place.entry((c.kind, c.id)).or_insert(i as u16 + 1);
        }
        let category_of = |kind: Kind, id: i64| match id {
            0 => 0,
            id => place.get(&(kind, id)).copied().unwrap_or(0),
        };
        let mut skipped = 0;
        let mut entries: Vec<Entry<'_>> = Vec::with_capacity(movies.len() + series.len());
        let mut pool: Vec<VersionIn<'_>> = Vec::with_capacity(movies.len());
        for title in merge(movies, max_year) {
            let film = |i: usize| &movies[title.versions[i].index as usize];
            // `merge` sorts a version whose id does not fit last, so the
            // default fits whenever any version does.
            let Some(id) = fit_id(film(0).stream_id) else {
                skipped += 1;
                continue;
            };
            let mut entry = Entry {
                kind: Kind::Film,
                id,
                name: title.title,
                key: Some(title.key),
                cover: "",
                year: title.year,
                rating: 0,
                added: 0,
                category: category_of(Kind::Film, film(0).category_id),
                genres: 0,
                dutch: false,
                english: false,
                adult: false,
                trailer: false,
                versions: 0..0,
            };
            let first = pool.len() as u32;
            for (i, v) in title.versions.iter().enumerate() {
                let m = film(i);
                let category = category_of(Kind::Film, m.category_id);
                let in_category = cat_recs.get(usize::from(category).wrapping_sub(1));
                entry.genres |= in_category.map_or(0, |c| c.genres);
                // A title's language is a set (Ruling 20): each of the
                // version's labels on its own, for `Labels::language()` names
                // one language and lets Dutch win; and its category's.
                let language = in_category.map_or(Language::Unknown, |c| c.language);
                entry.dutch |= v.labels.has(Labels::NL) || language == Language::Dutch;
                entry.english |= v.labels.has(Labels::EN) || language == Language::English;
                entry.adult |= m.is_adult;
                if entry.cover.is_empty() {
                    entry.cover = m.stream_icon.as_deref().map_or("", str::trim);
                }
                if entry.rating == 0 {
                    entry.rating = m.rating;
                }
                // A title is as new as its first version: a new 4K copy of an
                // old film is no new title.
                let added = day_of(m.added);
                if added > 0 && (entry.added == 0 || added < entry.added) {
                    entry.added = added;
                }
                match fit_id(m.stream_id) {
                    Some(sid) => pool.push((
                        sid,
                        v.labels,
                        m.container_extension.as_deref().unwrap_or(""),
                    )),
                    None => skipped += 1,
                }
            }
            entry.versions = first..pool.len() as u32;
            entries.push(entry);
        }
        // Series name the same few genre fields over and over.
        let mut genres_of_field: HashMap<&str, u32> = HashMap::new();
        for s in series {
            let Some(id) = fit_id(s.series_id) else {
                skipped += 1;
                continue;
            };
            let parts = parse_name(&s.name, max_year);
            let category = category_of(Kind::Series, s.category_id);
            let in_category = cat_recs.get(usize::from(category).wrapping_sub(1));
            // Its genre field; when that names none, its category's, as a film's.
            let field = s.genre.as_deref().unwrap_or("");
            let mut genres = *genres_of_field
                .entry(field)
                .or_insert_with(|| genres_in(field));
            if genres == 0 {
                genres = in_category.map_or(0, |c| c.genres);
            }
            let released = s.released().map_or(0, crate::portal::vod_info::year_of);
            let language = in_category.map_or(Language::Unknown, |c| c.language);
            entries.push(Entry {
                kind: Kind::Series,
                id,
                name: parts.title,
                key: None,
                cover: s.cover.as_deref().map_or("", str::trim),
                year: if released > 0 { released } else { parts.year },
                rating: s.rating,
                added: day_of(s.last_modified),
                category,
                genres,
                dutch: parts.labels.has(Labels::NL) || language == Language::Dutch,
                english: parts.labels.has(Labels::EN) || language == Language::English,
                adult: s.is_adult,
                trailer: false,
                versions: 0..0,
            });
        }
        let mut index = Self::assemble(entries, &pool, cat_recs);
        index.skipped += skipped;
        index
    }

    /// The index of `entries`, whose versions are ranges of `pool` and whose
    /// `category` is a position plus one in `categories`. Two titles of one
    /// kind and id keep the first; the others, a title whose id does not fit and
    /// a version whose stream id does not fit are left out and counted (the
    /// lists' build has left out its own already: this is the fence for what a
    /// cache file says).
    pub(crate) fn assemble(
        mut entries: Vec<Entry<'_>>,
        pool: &[VersionIn<'_>],
        categories: Vec<CategoryRec>,
    ) -> Self {
        let given = entries.len();
        entries.retain(|e| e.id != 0);
        entries.sort_by_key(|e| (e.kind, e.id));
        entries.dedup_by_key(|e| (e.kind, e.id));
        let n = entries.len();
        let bytes: usize = entries.iter().map(|e| e.name.len()).sum();
        let cover_bytes: usize = entries.iter().map(|e| e.cover.len()).sum();
        let mut index = TitleIndex {
            records: Vec::with_capacity(n),
            names: TextTable::with_capacity(n, bytes),
            keys: TextTable::with_capacity(n, bytes),
            covers: TextTable::with_capacity(n, cover_bytes),
            versions: Vec::with_capacity(pool.len()),
            exts: Vec::new(),
            categories,
            skipped: given - n,
        };
        let mut key = String::new();
        for e in entries {
            index.names.push(e.name);
            match &e.key {
                Some(k) => index.keys.push(k),
                None => {
                    key.clear();
                    fold_into(e.name, &mut key);
                    index.keys.push(&key);
                }
            }
            index.covers.push(e.cover);
            for &(sid, labels, ext) in &pool[e.versions.start as usize..e.versions.end as usize] {
                if sid == 0 {
                    index.skipped += 1;
                    continue;
                }
                let ext = index.ext_code(ext);
                index.versions.push(VersionRec {
                    stream_id: sid,
                    labels: labels.0,
                    ext,
                });
            }
            let flags = [
                (e.kind == Kind::Film, FLAG_FILM),
                (e.adult, FLAG_ADULT),
                (e.trailer, FLAG_TRAILER),
                (e.dutch, FLAG_DUTCH),
                (e.english, FLAG_ENGLISH),
            ]
            .iter()
            .filter(|(on, _)| *on)
            .fold(0, |all, (_, bit)| all | bit);
            index.records.push(Record {
                id: e.id,
                genres: AtomicU32::new(e.genres),
                versions_end: index.versions.len() as u32,
                year: AtomicU16::new(e.year),
                added: e.added,
                category: e.category,
                rating: e.rating,
                flags: AtomicU8::new(flags),
            });
        }
        index.names.shrink();
        index.keys.shrink();
        index.covers.shrink();
        index.versions.shrink_to_fit();
        index
    }

    /// The code of container `ext` (its place plus one); 0 for none, and when
    /// 255 others are known already.
    fn ext_code(&mut self, ext: &str) -> u8 {
        let ext = ext.trim();
        if ext.is_empty() {
            return 0;
        }
        if let Some(i) = self.exts.iter().position(|e| &**e == ext) {
            return i as u8 + 1;
        }
        if self.exts.len() >= u8::MAX as usize {
            return 0;
        }
        self.exts.push(ext.into());
        self.exts.len() as u8
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The title at `pos`, adult or not: the catalog's `Titles` asks with the
    /// lock in mind.
    pub fn title(&self, pos: usize) -> Option<Title<'_>> {
        let r = self.records.get(pos)?;
        let flags = r.flags.load(Ordering::Relaxed);
        let kind = if flags & FLAG_FILM != 0 {
            Kind::Film
        } else {
            Kind::Series
        };
        let start = match pos {
            0 => 0,
            _ => self.records[pos - 1].versions_end as usize,
        };
        let cover = self.covers.get(pos);
        Some(Title {
            pos,
            id: TitleId {
                kind,
                id: i64::from(r.id),
            },
            name: self.names.get(pos),
            cover: (!cover.is_empty()).then_some(cover),
            year: r.year.load(Ordering::Relaxed),
            rating: r.rating,
            genres: r.genres.load(Ordering::Relaxed),
            category: self.category_at(r.category),
            added_day: r.added,
            dutch: flags & FLAG_DUTCH != 0,
            english: flags & FLAG_ENGLISH != 0,
            adult: flags & FLAG_ADULT != 0,
            trailer: flags & FLAG_TRAILER != 0,
            versions: Versions {
                recs: &self.versions[start..r.versions_end as usize],
                exts: &self.exts,
            },
        })
    }

    fn category_at(&self, code: u16) -> Option<CategoryInfo<'_>> {
        let c = self.categories.get(usize::from(code).checked_sub(1)?)?;
        Some(CategoryInfo {
            kind: c.kind,
            id: c.id,
            name: &c.name,
        })
    }

    /// Where title `id` is; none when the index does not hold it.
    pub fn find(&self, id: TitleId) -> Option<usize> {
        let id32 = fit_id(id.id)?;
        let film = id.kind == Kind::Film;
        let pos = self.records.partition_point(|r| {
            let r_film = r.flags.load(Ordering::Relaxed) & FLAG_FILM != 0;
            (r_film, r.id) < (film, id32)
        });
        let r = self.records.get(pos)?;
        let r_film = r.flags.load(Ordering::Relaxed) & FLAG_FILM != 0;
        (r_film == film && r.id == id32).then_some(pos)
    }

    /// The film title that `stream_id` is a version of: what plays tells which
    /// title it is (a scan of the versions; no map is kept for it).
    pub fn title_of_version(&self, stream_id: i64) -> Option<usize> {
        let sid = fit_id(stream_id)?;
        // The title whose default it is, if there is one: a stream a portal lists
        // under two names is that title's, as `find` has it.
        let id = TitleId {
            kind: Kind::Film,
            id: i64::from(sid),
        };
        if let Some(pos) = self.find(id) {
            return Some(pos);
        }
        let at = self.versions.iter().position(|v| v.stream_id == sid)? as u32;
        Some(self.records.partition_point(|r| r.versions_end <= at))
    }

    /// The portal's categories, films' first.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.categories.iter().map(|c| CategoryInfo {
            kind: c.kind,
            id: c.id,
            name: &c.name,
        })
    }

    /// What title `pos`'s details tell: a year where its name gave none (the
    /// name's year made the title), genres where its category and genre field
    /// named none, and whether it has a trailer.
    pub fn learn(&self, pos: usize, learned: Learned) {
        let Some(r) = self.records.get(pos) else {
            return;
        };
        if learned.year > 0 {
            let _ = r
                .year
                .compare_exchange(0, learned.year, Ordering::Relaxed, Ordering::Relaxed);
        }
        // Only the bits of the genres there are: a bit past them would index the
        // counts of a query.
        let genres = learned.genres & GENRE_BITS;
        if genres != 0 {
            let _ = r
                .genres
                .compare_exchange(0, genres, Ordering::Relaxed, Ordering::Relaxed);
        }
        if learned.trailer {
            r.flags.fetch_or(FLAG_TRAILER, Ordering::Relaxed);
        }
    }

    /// How the titles came out.
    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            versions: self.versions.len(),
            skipped: self.skipped,
            ..Stats::default()
        };
        for pos in 0..self.len() {
            let Some(t) = self.title(pos) else { continue };
            match t.id.kind {
                Kind::Film => s.films += 1,
                Kind::Series => s.series += 1,
            }
            s.with_cover += usize::from(t.cover.is_some());
            s.with_rating += usize::from(t.rating > 0);
            s.with_year += usize::from(t.year > 0);
            s.with_genre += usize::from(t.genres != 0);
            s.dutch += usize::from(t.dutch);
            s.english += usize::from(t.english);
        }
        s
    }

    /// The bytes the index holds, the covers aside (spec §4.2's 5 MB): the
    /// records, the names and search names, the versions and the categories.
    pub fn heap_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<Record>()
            + self.names.heap_bytes()
            + self.keys.heap_bytes()
            + self.versions.capacity() * std::mem::size_of::<VersionRec>()
            + self.exts.iter().map(|e| e.len() + 16).sum::<usize>()
            + self
                .categories
                .iter()
                .map(|c| c.name.len() + std::mem::size_of::<CategoryRec>())
                .sum::<usize>()
    }

    /// The bytes of the cover addresses.
    pub fn cover_bytes(&self) -> usize {
        self.covers.heap_bytes()
    }
}

/// A category as kept: its genres and language read from its name.
pub(crate) fn category_rec(c: &CategoryIn<'_>) -> CategoryRec {
    CategoryRec {
        kind: c.kind,
        id: c.id,
        name: c.name.into(),
        genres: genres_in(c.name),
        language: category_labels(c.name).language(),
    }
}
