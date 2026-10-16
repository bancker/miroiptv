//! The channel list of an M3U source (`g`; spec 2026-10-15, §3): its groups,
//! A–Z, and where the selection is. Worked out from the catalog's channels when
//! the list opens (`CatalogStore::channel_groups`): positions in them, never
//! copies. Draws nothing; channel_list_ui.rs does.

use crate::portal::LiveChannel;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A group of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKey {
    /// Every channel but the defect ones.
    All,
    /// The source's favorites that are in the list.
    Favorites,
    /// One of the list's own groups (`group-title`), spelled as the file has it.
    Named(String),
    /// The channels without a group.
    NoGroup,
    /// The channels marked defect, and only there.
    Defect,
}

/// One group: its channels A–Z, as positions in the catalog's channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub key: GroupKey,
    pub members: Vec<u32>,
}

/// The list's groups in their order (spec §3.2): All channels; Favorites when
/// the list holds one; the list's groups A–Z; No group when a channel has
/// none; Defect channels when a channel of the list is marked.
pub struct ChannelGroups {
    live: Arc<Vec<LiveChannel>>,
    groups: Vec<Group>,
}

impl ChannelGroups {
    /// The groups of `live`. `shows` says which channels the catalog shows at
    /// all (its adult filter); `defect` holds the marked ones, which are in
    /// Defect channels only (spec §4.3); `favorites` the source's favorites.
    /// One sort for every group: each takes its channels in that order.
    pub fn build(
        live: Arc<Vec<LiveChannel>>,
        shows: impl Fn(&LiveChannel) -> bool,
        defect: &HashMap<i64, i64>,
        favorites: &HashSet<i64>,
    ) -> Self {
        // Room for every channel at once: `collect` over a filter has no size
        // hint and would grow by doubling, to twice the room the keys need.
        let mut keyed: Vec<(String, u32)> = Vec::with_capacity(live.len());
        keyed.extend(
            live.iter()
                .enumerate()
                .filter(|(_, c)| shows(c))
                .map(|(i, c)| (sort_key(&c.name), i as u32)),
        );
        keyed.sort_by(|(a, i), (b, j)| compare_folded(a, b).then(i.cmp(j)));
        // All holds the shown channels but the marked ones, so never more than
        // `keyed`; the room it asks for is room it keeps while the list is open.
        let (mut all, mut favs, mut none, mut broken) = (
            Vec::with_capacity(keyed.len()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut named: HashMap<&str, Vec<u32>> = HashMap::new();
        for &(_, i) in &keyed {
            let channel = &live[i as usize];
            if defect.contains_key(&channel.stream_id) {
                broken.push(i);
                continue;
            }
            all.push(i);
            if favorites.contains(&channel.stream_id) {
                favs.push(i);
            }
            let mut grouped = false;
            for name in group_names(channel.category_id.as_deref()) {
                grouped = true;
                let members = named.entry(name).or_default();
                // "A;A" puts the channel in A once.
                if members.last() != Some(&i) {
                    members.push(i);
                }
            }
            if !grouped {
                none.push(i);
            }
        }
        drop(keyed);
        // A group name's key is worked out once, not at every comparison.
        let mut named: Vec<(String, &str, Vec<u32>)> = named
            .into_iter()
            .map(|(name, members)| (sort_key(name), name, members))
            .collect();
        named.sort_by(|(ka, a, _), (kb, b, _)| compare_folded(ka, kb).then(a.cmp(b)));
        let mut groups = vec![Group {
            key: GroupKey::All,
            members: all,
        }];
        if !favs.is_empty() {
            groups.push(Group {
                key: GroupKey::Favorites,
                members: favs,
            });
        }
        groups.extend(named.into_iter().map(|(_, name, members)| Group {
            key: GroupKey::Named(name.to_owned()),
            members,
        }));
        if !none.is_empty() {
            groups.push(Group {
                key: GroupKey::NoGroup,
                members: none,
            });
        }
        if !broken.is_empty() {
            groups.push(Group {
                key: GroupKey::Defect,
                members: broken,
            });
        }
        Self { live, groups }
    }

    /// The groups, in the list's order.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn key(&self, group: usize) -> Option<&GroupKey> {
        self.groups.get(group).map(|g| &g.key)
    }

    /// How many channels group `group` holds.
    pub fn rows(&self, group: usize) -> usize {
        self.groups.get(group).map_or(0, |g| g.members.len())
    }

    /// The channel at `row` of group `group`.
    pub fn channel(&self, group: usize, row: usize) -> Option<&LiveChannel> {
        let &index = self.groups.get(group)?.members.get(row)?;
        self.live.get(index as usize)
    }

    /// The row of channel `stream_id` in group `group`.
    pub fn row_of(&self, group: usize, stream_id: i64) -> Option<usize> {
        self.groups.get(group)?.members.iter().position(|&i| {
            self.live
                .get(i as usize)
                .is_some_and(|c| c.stream_id == stream_id)
        })
    }

    /// The group with this key.
    pub fn find(&self, key: &GroupKey) -> Option<usize> {
        self.groups.iter().position(|g| g.key == *key)
    }

    /// How many channels the list shows, the defect ones too (for the log).
    pub fn channel_count(&self) -> usize {
        self.groups
            .iter()
            .filter(|g| matches!(g.key, GroupKey::All | GroupKey::Defect))
            .map(|g| g.members.len())
            .sum()
    }

    /// About how many bytes the groups keep on the heap: their positions and
    /// names. The channels are the catalog's.
    pub fn heap_bytes(&self) -> usize {
        let groups: usize = self
            .groups
            .iter()
            .map(|g| {
                let name = match &g.key {
                    GroupKey::Named(name) => name.capacity(),
                    _ => 0,
                };
                g.members.capacity() * std::mem::size_of::<u32>() + name
            })
            .sum();
        self.groups.capacity() * std::mem::size_of::<Group>() + groups
    }
}

/// The groups of a channel's `group-title`: its `;`-separated parts, trimmed,
/// the empty ones left out.
pub fn group_names(category: Option<&str>) -> impl Iterator<Item = &str> {
    category
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|g| !g.is_empty())
}

/// A–Z as people read it: case and accents aside, a run of digits by its value
/// ("Sport 2" before "Sport 10"), digits before letters. Texts that differ only
/// in case, in accents or in leading zeros compare equal.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    compare_folded(&sort_key(a), &sort_key(b))
}

/// A name as a sort key: lower case, and every letter of Latin-1 and of Latin
/// Extended-A as the plain letter or letters it is read as ("een" for "Één",
/// "o1" for "Ö1", "sok" for "Şok", "lodz" for "Łódź", "strasse" for "Straße"),
/// so it sorts where a Dutch, Turkish, Polish, Czech or Nordic reader looks for
/// it and not after the z. A ligature, or a letter the plain alphabet writes
/// with two, is spelled out (ß ss, æ ae, œ oe, ĳ ij, þ th); ð is a d. An accent
/// written as a mark of its own (an "e" and U+0301) goes. A name in plain ASCII,
/// nearly every name, is only lower-cased, with one allocation.
fn sort_key(name: &str) -> String {
    if name.is_ascii() {
        return name.to_ascii_lowercase();
    }
    let mut key = String::with_capacity(name.len());
    for c in name.chars() {
        // The ASCII letters of a name that has others need no table.
        if c.is_ascii() {
            key.push(c.to_ascii_lowercase());
            continue;
        }
        for lower in c.to_lowercase() {
            match plain_letters(lower) {
                Some(plain) => key.push_str(plain),
                None => key.push(lower),
            }
        }
    }
    key
}

/// The plain letters a lower-case character is read as, or `None` when it is
/// read as itself: a letter of Latin-1 or of Latin Extended-A without its
/// accent, spelled out where the plain alphabet writes it with two (ß ss, æ ae,
/// œ oe, ĳ ij, þ th), ð as d, and nothing for an accent written as a mark of its
/// own. The Latin Extended-A ranges take both cases of a letter at a time, so
/// that each can be read against the Unicode chart.
fn plain_letters(c: char) -> Option<&'static str> {
    Some(match c {
        // An accent written as a mark of its own.
        '\u{300}'..='\u{36f}' => "",
        // Latin-1.
        'à'..='å' => "a",
        'æ' => "ae",
        'ç' => "c",
        'è'..='ë' => "e",
        'ì'..='ï' => "i",
        'ð' => "d",
        'ñ' => "n",
        'ò'..='ö' | 'ø' => "o",
        'ù'..='ü' => "u",
        'ý' | 'ÿ' => "y",
        'þ' => "th",
        'ß' => "ss",
        // Latin Extended-A (U+0100 to U+017F), a letter in both cases at a
        // time: Ā ā Ă ă Ą ą are an a, Ć ć Ĉ ĉ Ċ ċ Č č a c, and so on.
        '\u{100}'..='\u{105}' => "a",
        '\u{106}'..='\u{10d}' => "c",
        '\u{10e}'..='\u{111}' => "d",
        '\u{112}'..='\u{11b}' => "e",
        '\u{11c}'..='\u{123}' => "g",
        '\u{124}'..='\u{127}' => "h",
        '\u{128}'..='\u{131}' => "i", // İ and the dotless ı too
        '\u{132}'..='\u{133}' => "ij",
        '\u{134}'..='\u{135}' => "j",
        '\u{136}'..='\u{138}' => "k", // the kra ĸ too
        '\u{139}'..='\u{142}' => "l",
        '\u{143}'..='\u{14b}' => "n", // ŉ and Ŋ ŋ too
        '\u{14c}'..='\u{151}' => "o",
        '\u{152}'..='\u{153}' => "oe",
        '\u{154}'..='\u{159}' => "r",
        '\u{15a}'..='\u{161}' | '\u{17f}' => "s", // the long ſ too
        '\u{162}'..='\u{167}' => "t",
        '\u{168}'..='\u{173}' => "u",
        '\u{174}'..='\u{175}' => "w",
        '\u{176}'..='\u{178}' => "y",
        '\u{179}'..='\u{17e}' => "z",
        _ => return None,
    })
}

/// [`natural_cmp`] of texts as [`sort_key`] made them. Bytewise, which for UTF-8
/// is the order of the characters; an ASCII digit is never part of another one.
fn compare_folded(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (from_i, from_j) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (x, y) = (without_zeros(&a[from_i..i]), without_zeros(&b[from_j..j]));
            let by_value = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if by_value != Ordering::Equal {
                return by_value;
            }
        } else if a[i] != b[j] {
            return a[i].cmp(&b[j]);
        } else {
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn without_zeros(digits: &[u8]) -> &[u8] {
    let first = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[first..]
}

/// Where the selection is: a group, and a row in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub group: usize,
    pub row: usize,
}

impl Cursor {
    /// Where the list opens (spec §3.3): in group `last` (the one open last this
    /// session; All when there is none, or it is gone), on channel `playing` when
    /// that group holds it, else on its first row.
    pub fn open(groups: &ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self {
        let group = last.and_then(|key| groups.find(key)).unwrap_or(0);
        Self {
            group,
            row: Self::landing(groups, group, playing),
        }
    }

    /// The row a group is entered on: `playing`'s when it is in it, else the first.
    fn landing(groups: &ChannelGroups, group: usize, playing: Option<i64>) -> usize {
        playing
            .and_then(|sid| groups.row_of(group, sid))
            .unwrap_or(0)
    }

    /// `delta` rows on (↑ ↓ PgUp PgDn), stopping at the first and the last row:
    /// the list does not wrap. Whether the selection moved.
    pub fn step(&mut self, groups: &ChannelGroups, delta: isize) -> bool {
        let last = groups.rows(self.group).saturating_sub(1);
        let to = self.row.saturating_add_signed(delta).min(last);
        self.move_to(to)
    }

    /// The first row (Home) or the last (End). Whether the selection moved.
    pub fn to_end(&mut self, groups: &ChannelGroups, last: bool) -> bool {
        let to = if last {
            groups.rows(self.group).saturating_sub(1)
        } else {
            0
        };
        self.move_to(to)
    }

    /// Row `to` of the group the selection is in. Whether it moved.
    fn move_to(&mut self, to: usize) -> bool {
        let moved = to != self.row;
        self.row = to;
        moved
    }

    /// The group before (`delta` -1) or after (+1), stopping at the first and
    /// the last (←/→), entered on `playing` when it holds it, else on its first
    /// row. Whether the group changed.
    pub fn step_group(
        &mut self,
        groups: &ChannelGroups,
        delta: isize,
        playing: Option<i64>,
    ) -> bool {
        let last = groups.group_count().saturating_sub(1);
        let to = self.group.saturating_add_signed(delta).min(last);
        self.select_group(groups, to, playing)
    }

    /// Group `group` (a click), entered as `step_group` enters one. Whether the
    /// group changed.
    pub fn select_group(
        &mut self,
        groups: &ChannelGroups,
        group: usize,
        playing: Option<i64>,
    ) -> bool {
        if group == self.group || group >= groups.group_count() {
            return false;
        }
        self.group = group;
        self.row = Self::landing(groups, group, playing);
        true
    }

    /// Row `row` of the group (a click). Whether the selection moved.
    pub fn select_row(&mut self, groups: &ChannelGroups, row: usize) -> bool {
        if row >= groups.rows(self.group) || row == self.row {
            return false;
        }
        self.row = row;
        true
    }

    /// The same place in groups made anew while the list is open (another copy
    /// of the list, a channel marked or cleared): the same group by its key,
    /// else All; the same channel when the group still holds it, else the same
    /// row, as far as the group goes.
    pub fn follow(&mut self, old: &ChannelGroups, new: &ChannelGroups) {
        let selected = old.channel(self.group, self.row).map(|c| c.stream_id);
        let group = old
            .key(self.group)
            .and_then(|key| new.find(key))
            .unwrap_or(0);
        let row = selected
            .and_then(|sid| new.row_of(group, sid))
            .unwrap_or(self.row);
        self.group = group;
        self.row = row.min(new.rows(group).saturating_sub(1));
    }

    /// The selected channel; none in an empty group.
    pub fn channel<'a>(&self, groups: &'a ChannelGroups) -> Option<&'a LiveChannel> {
        groups.channel(self.group, self.row)
    }
}
