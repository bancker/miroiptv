//! Films merged into titles with versions (spec 2026-10-15 §4.2). A portal
//! lists one film many times: in 4K and in 1080p, spoken Dutch or with Dutch
//! subtitles. They are one title when their names, without prefix, labels and
//! year, fold to the same words and they give the same year. The highest
//! quality is the default version, as `catalog::quality_rank` picks a
//! channel's variant; a film page offers the others. Series are not merged.

use super::fold_into;
use super::genres::{parse_name, Labels};
use crate::portal::Movie;
use std::collections::HashMap;
use std::fmt::Write as _;

/// A version of a film title: the film's position in the list, and its labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmVersion {
    pub index: u32,
    pub labels: Labels,
}

/// A film title: the films of one name and year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmTitle<'a> {
    /// What the tile shows: the default version's name without prefix, labels
    /// and year.
    pub title: &'a str,
    /// The folded title (`vod::fold`): what the versions share, and what the
    /// search compares.
    pub key: String,
    /// The year the names give; 0: none.
    pub year: u16,
    /// Its versions, the default one first: the highest quality tier, HDR
    /// before none, then the lowest stream id (the same default on every load).
    /// A version whose stream id fits no title (not 1 to `u32::MAX`; a portal's
    /// unreadable id is 0) comes after all the others, whatever its quality:
    /// the title's id is the default version's stream id.
    pub versions: Vec<FilmVersion>,
}

/// `movies` merged into titles, in the order of each title's first film.
/// `max_year` is the latest year a name may give (`genres::parse_name`).
pub fn merge(movies: &[Movie], max_year: u16) -> Vec<FilmTitle<'_>> {
    let mut titles: Vec<FilmTitle<'_>> = Vec::with_capacity(movies.len());
    // The folded title, a tab and the year: one buffer for every lookup, so
    // only a new title allocates its key.
    let mut by_key: HashMap<String, usize> = HashMap::with_capacity(movies.len());
    let mut lookup = String::new();
    for (i, movie) in movies.iter().enumerate() {
        let parts = parse_name(&movie.name, max_year);
        lookup.clear();
        fold_into(parts.title, &mut lookup);
        let _ = write!(lookup, "\t{}", parts.year);
        let version = FilmVersion {
            index: i as u32,
            labels: parts.labels,
        };
        match by_key.get(lookup.as_str()) {
            Some(&t) => titles[t].versions.push(version),
            None => {
                by_key.insert(lookup.clone(), titles.len());
                titles.push(FilmTitle {
                    title: parts.title,
                    key: String::new(),
                    year: parts.year,
                    versions: vec![version],
                });
            }
        }
    }
    // Each title takes its key from the map, without a copy: the folded title
    // before the tab.
    for (mut key, t) in by_key {
        key.truncate(key.rfind('\t').unwrap_or(key.len()));
        titles[t].key = key;
    }
    for title in &mut titles {
        if title.versions.len() > 1 {
            title.versions.sort_by_key(|v| {
                let id = movies[v.index as usize].stream_id;
                (
                    !(1..=i64::from(u32::MAX)).contains(&id),
                    std::cmp::Reverse(v.labels.rank()),
                    !v.labels.has(Labels::HDR),
                    id,
                )
            });
            let first = &movies[title.versions[0].index as usize];
            title.title = parse_name(&first.name, max_year).title;
        }
    }
    titles
}
