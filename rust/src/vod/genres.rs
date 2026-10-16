//! What the names of titles and categories say (spec 2026-10-15 §4.2): the
//! fixed list of 24 genres with the words portals use for them, the language
//! markers, the quality and language labels of a film's version, and the year
//! at the end of a name.
//!
//! Everything is compared [`fold`]ed (lower case, no accents) and as whole
//! words. Labels and language markers count only where labels stand: a prefix
//! before the last `|`, a prefix before ` - ` that holds nothing else, a group
//! in brackets, and the words at the end; in a category's name also the words
//! it starts with ("NL FILMS"). So the "en" of "Romeo en Julia" is Dutch for
//! "and", no English marker, and the "Dual" of "Dual (2022)" is its title.

use super::{fold, fold_into};
use crate::i18n::T;

/// One of the fixed genres. Its name is an i18n text ([`Genre::text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Genre {
    Action,
    Adventure,
    Animation,
    Anime,
    Biography,
    Christmas,
    Comedy,
    Crime,
    Documentary,
    Drama,
    Family,
    Fantasy,
    History,
    Horror,
    Kids,
    Music,
    Mystery,
    Reality,
    Romance,
    ScienceFiction,
    Sport,
    Thriller,
    War,
    Western,
}

impl Genre {
    /// Every genre, in the order of their bits.
    pub const ALL: [Genre; 24] = [
        Genre::Action,
        Genre::Adventure,
        Genre::Animation,
        Genre::Anime,
        Genre::Biography,
        Genre::Christmas,
        Genre::Comedy,
        Genre::Crime,
        Genre::Documentary,
        Genre::Drama,
        Genre::Family,
        Genre::Fantasy,
        Genre::History,
        Genre::Horror,
        Genre::Kids,
        Genre::Music,
        Genre::Mystery,
        Genre::Reality,
        Genre::Romance,
        Genre::ScienceFiction,
        Genre::Sport,
        Genre::Thriller,
        Genre::War,
        Genre::Western,
    ];

    /// Its bit in a genre mask.
    pub fn bit(self) -> u32 {
        1 << self as u32
    }

    /// Its name, in the user's language.
    pub fn text(self) -> T {
        match self {
            Genre::Action => T::GenreAction,
            Genre::Adventure => T::GenreAdventure,
            Genre::Animation => T::GenreAnimation,
            Genre::Anime => T::GenreAnime,
            Genre::Biography => T::GenreBiography,
            Genre::Christmas => T::GenreChristmas,
            Genre::Comedy => T::GenreComedy,
            Genre::Crime => T::GenreCrime,
            Genre::Documentary => T::GenreDocumentary,
            Genre::Drama => T::GenreDrama,
            Genre::Family => T::GenreFamily,
            Genre::Fantasy => T::GenreFantasy,
            Genre::History => T::GenreHistory,
            Genre::Horror => T::GenreHorror,
            Genre::Kids => T::GenreKids,
            Genre::Music => T::GenreMusic,
            Genre::Mystery => T::GenreMystery,
            Genre::Reality => T::GenreReality,
            Genre::Romance => T::GenreRomance,
            Genre::ScienceFiction => T::GenreScienceFiction,
            Genre::Sport => T::GenreSport,
            Genre::Thriller => T::GenreThriller,
            Genre::War => T::GenreWar,
            Genre::Western => T::GenreWestern,
        }
    }

    /// The words portals use for it, folded: Dutch, English and German, and
    /// the plurals they use.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Genre::Action => &[
                "action",
                "actie",
                "actiefilm",
                "actiefilms",
                "actionfilm",
                "actionfilme",
                "martial arts",
                "superhero",
                "superheroes",
                "superheld",
                "superhelden",
                "marvel",
            ],
            Genre::Adventure => &[
                "adventure",
                "adventures",
                "avontuur",
                "avonturen",
                "abenteuer",
            ],
            Genre::Animation => &[
                "animation",
                "animatie",
                "animated",
                "animatiefilm",
                "animatiefilms",
                "tekenfilm",
                "tekenfilms",
                "cartoon",
                "cartoons",
                "zeichentrick",
                "pixar",
            ],
            Genre::Anime => &["anime"],
            Genre::Biography => &["biography", "biografie", "biographie", "biopic", "biopics"],
            Genre::Christmas => &[
                "christmas",
                "kerst",
                "kerstmis",
                "kerstfilm",
                "kerstfilms",
                "weihnachten",
                "weihnachtsfilm",
                "xmas",
                "x mas",
            ],
            Genre::Comedy => &[
                "comedy", "comedies", "komedie", "komedies", "komodie", "komodien", "comedie",
                "humor", "stand up", "standup", "cabaret", "kabarett",
            ],
            Genre::Crime => &[
                "crime",
                "misdaad",
                "krimi",
                "gangster",
                "detective",
                "detectives",
            ],
            Genre::Documentary => &[
                "documentary",
                "documentaries",
                "documentaire",
                "documentaires",
                "docu",
                "docus",
                "dokumentation",
                "dokumentarfilm",
                "doku",
            ],
            Genre::Drama => &["drama", "dramas"],
            Genre::Family => &[
                "family",
                "familie",
                "familien",
                "familiefilm",
                "familiefilms",
                "familienfilm",
            ],
            Genre::Fantasy => &["fantasy"],
            Genre::History => &[
                "history",
                "historical",
                "historisch",
                "historie",
                "geschiedenis",
                "geschichte",
            ],
            Genre::Horror => &["horror", "horrorfilm", "horrorfilms", "horrorfilme"],
            Genre::Kids => &[
                "kids",
                "kinderen",
                "kinder",
                "kinderfilm",
                "kinderfilms",
                "kinderfilme",
                "kinderserie",
                "kinderseries",
                "jeugd",
                "children",
            ],
            Genre::Music => &[
                "music",
                "muziek",
                "musik",
                "musical",
                "musicals",
                "concert",
                "concerten",
                "konzert",
            ],
            Genre::Mystery => &["mystery", "mysterie", "mysteries"],
            Genre::Reality => &["reality", "realityshow", "reality tv"],
            Genre::Romance => &[
                "romance",
                "romantiek",
                "romantisch",
                "romantische",
                "romantic",
                "romantik",
                "romcom",
            ],
            Genre::ScienceFiction => {
                &["science fiction", "sciencefiction", "sci fi", "scifi", "sf"]
            }
            Genre::Sport => &["sport", "sports", "sporten"],
            Genre::Thriller => &["thriller", "thrillers", "suspense", "spanning"],
            Genre::War => &[
                "war",
                "oorlog",
                "oorlogsfilm",
                "oorlogsfilms",
                "krieg",
                "kriegsfilm",
            ],
            Genre::Western => &["western", "westerns"],
        }
    }
}

/// The genres named anywhere in `text` (a series' genre field, a category's
/// name) as a mask of [`Genre::bit`]s: every alias found as whole words.
pub fn genres_in(text: &str) -> u32 {
    let padded = format!(" {} ", fold(text));
    let mut mask = 0;
    for genre in Genre::ALL {
        let named = genre.aliases().iter().any(|alias| {
            padded.match_indices(alias).any(|(at, _)| {
                padded.as_bytes()[at - 1] == b' ' && padded.as_bytes()[at + alias.len()] == b' '
            })
        });
        if named {
            mask |= genre.bit();
        }
    }
    mask
}

/// The genres of a mask, in the fixed order.
pub fn genres_of(mask: u32) -> impl Iterator<Item = Genre> {
    Genre::ALL.into_iter().filter(move |g| mask & g.bit() != 0)
}

/// The quality and language labels of a name: a set of bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Labels(pub u16);

impl Labels {
    /// 4K, UHD, 2160p.
    pub const UHD: Labels = Labels(1);
    /// 1080p, FHD, Full HD.
    pub const FHD: Labels = Labels(1 << 1);
    /// 720p, HD.
    pub const HD: Labels = Labels(1 << 2);
    pub const SD: Labels = Labels(1 << 3);
    pub const HDR: Labels = Labels(1 << 4);
    pub const THREE_D: Labels = Labels(1 << 5);
    /// Dutch: NL, Nederlands, Dutch, NL Gesproken.
    pub const NL: Labels = Labels(1 << 6);
    /// Dutch subtitles: no Dutch marker.
    pub const NL_SUBS: Labels = Labels(1 << 7);
    pub const MULTI: Labels = Labels(1 << 8);
    pub const DUAL: Labels = Labels(1 << 9);
    /// English: EN, English, UK.
    pub const EN: Labels = Labels(1 << 10);

    pub fn has(self, label: Labels) -> bool {
        self.0 & label.0 == label.0 && label.0 != 0
    }

    pub fn with(self, label: Labels) -> Labels {
        Labels(self.0 | label.0)
    }

    /// Its quality tier, as `catalog::quality_rank` ranks a channel's: UHD 4,
    /// FHD 3, HD 2, none 1, SD 0.
    pub fn rank(self) -> u8 {
        if self.has(Labels::UHD) {
            4
        } else if self.has(Labels::FHD) {
            3
        } else if self.has(Labels::HD) {
            2
        } else if self.has(Labels::SD) {
            0
        } else {
            1
        }
    }

    /// The language the labels say: Dutch over English (the spec names it
    /// first), else unknown.
    pub fn language(self) -> Language {
        if self.has(Labels::NL) {
            Language::Dutch
        } else if self.has(Labels::EN) {
            Language::English
        } else {
            Language::Unknown
        }
    }

    /// How a version's choice names it: "4K HDR", "1080p NL SUBS"; empty
    /// without labels. These tokens are the same in every language.
    pub fn text(self) -> String {
        let quality = match self.rank() {
            4 => Some("4K"),
            3 => Some("1080p"),
            2 => Some("720p"),
            0 => Some("SD"),
            _ => None,
        };
        let rest = [
            (Labels::HDR, "HDR"),
            (Labels::THREE_D, "3D"),
            (Labels::NL, "NL"),
            (Labels::NL_SUBS, "NL SUBS"),
            (Labels::EN, "EN"),
            (Labels::MULTI, "MULTI"),
            (Labels::DUAL, "DUAL"),
        ];
        quality
            .into_iter()
            .chain(rest.iter().filter(|(l, _)| self.has(*l)).map(|(_, t)| *t))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The spoken language of a title, when its labels say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    Unknown,
    Dutch,
    English,
}

/// The labels, folded, as phrases of one or two words: the two-word ones are
/// tried first.
const LABELS: &[(&str, Labels)] = &[
    ("nl gesproken", Labels::NL),
    ("nederlands gesproken", Labels::NL),
    ("nl subs", Labels::NL_SUBS),
    ("nl sub", Labels::NL_SUBS),
    ("full hd", Labels::FHD),
    ("ultra hd", Labels::UHD),
    ("dolby vision", Labels::HDR),
    ("multi audio", Labels::MULTI),
    ("multi sub", Labels::MULTI),
    ("multi subs", Labels::MULTI),
    ("dual audio", Labels::DUAL),
    ("4k", Labels::UHD),
    ("uhd", Labels::UHD),
    ("2160p", Labels::UHD),
    ("2160", Labels::UHD),
    ("1080p", Labels::FHD),
    ("1080", Labels::FHD),
    ("fhd", Labels::FHD),
    ("720p", Labels::HD),
    ("720", Labels::HD),
    ("hd", Labels::HD),
    ("sd", Labels::SD),
    ("480p", Labels::SD),
    ("576p", Labels::SD),
    ("hdr", Labels::HDR),
    ("hdr10", Labels::HDR),
    ("3d", Labels::THREE_D),
    ("nl", Labels::NL),
    ("nederlands", Labels::NL),
    ("dutch", Labels::NL),
    ("nlsubs", Labels::NL_SUBS),
    ("nlsub", Labels::NL_SUBS),
    ("multi", Labels::MULTI),
    ("multisub", Labels::MULTI),
    ("dual", Labels::DUAL),
    ("en", Labels::EN),
    ("english", Labels::EN),
    ("uk", Labels::EN),
];

/// The label a folded word or phrase is.
fn label_of(phrase: &str) -> Option<Labels> {
    LABELS.iter().find(|(w, _)| *w == phrase).map(|(_, l)| *l)
}

/// The label the folded words `a b` are together, without building the phrase.
fn pair_of(a: &str, b: &str) -> Option<Labels> {
    LABELS
        .iter()
        .find(|(w, _)| {
            w.len() == a.len() + 1 + b.len()
                && w.starts_with(a)
                && w.as_bytes()[a.len()] == b' '
                && w.ends_with(b)
        })
        .map(|(_, l)| *l)
}

/// The labels among the words of `folded` (a folded text), two-word labels
/// first. With `every`: none unless every word is one, and none for no words.
fn labels_of_words(folded: &str, every: bool) -> Option<Labels> {
    let mut labels = Labels::default();
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    let mut any = false;
    while let Some(w) = words.next() {
        any = true;
        if let Some(l) = words.peek().and_then(|next| pair_of(w, next)) {
            labels = labels.with(l);
            words.next();
        } else if let Some(l) = label_of(w) {
            labels = labels.with(l);
        } else if every {
            return None;
        }
    }
    (any || !every).then_some(labels)
}

/// A year from 1900 to `max_year` written as four digits; none for anything
/// else.
fn as_year(word: &str, max_year: u16) -> Option<u16> {
    if word.len() != 4 || !word.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: u16 = word.parse().ok()?;
    (1900..=max_year).contains(&year).then_some(year)
}

/// What a title's name says beside its title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameParts<'a> {
    /// The name without its prefix, and without the labels and the year at its
    /// end: what a tile shows. At least one word stays.
    pub title: &'a str,
    /// "(2024)" anywhere, else " 2024" at the end (before the labels); 0 when
    /// neither, or when it is later than `max_year`.
    pub year: u16,
    pub labels: Labels,
}

/// What a group in brackets holds.
#[derive(Clone, Copy)]
enum Group {
    Year(u16),
    Labels(Labels),
    Other,
}

/// A piece of a name: a word, or a group in brackets; where it starts and
/// ends in the name.
#[derive(Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
    group: Option<Group>,
}

/// What `parse_name` reuses from one name to the next: ~58,000 names are read
/// per catalog, and none of them allocates once these have grown.
#[derive(Default)]
struct Scratch {
    tokens: Vec<Token>,
    last: String,
    before: String,
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

/// Reads a film's, a series' or a category's name: the prefix before its last
/// `|` (whatever it holds), a prefix before ` - ` that is labels only, the
/// groups in brackets that hold a year or labels only, and the labels and the
/// year at its end. `max_year` is the latest year a name may give, the year
/// after this one: "Blade Runner 2049" is no film of 2049.
pub fn parse_name(name: &str, max_year: u16) -> NameParts<'_> {
    SCRATCH.with(|s| read_name(name, max_year, &mut s.borrow_mut()))
}

fn read_name<'a>(name: &'a str, max_year: u16, s: &mut Scratch) -> NameParts<'a> {
    let mut labels = Labels::default();
    let mut year = 0u16;
    let mut rest = name.trim();
    if let Some((before, after)) = rest.rsplit_once('|') {
        if !after.trim().is_empty() {
            s.before.clear();
            fold_into(before, &mut s.before);
            labels = labels.with(labels_of_words(&s.before, false).unwrap_or_default());
            rest = after.trim();
        }
    }
    if let Some((before, after)) = rest.split_once(" - ") {
        s.before.clear();
        fold_into(before, &mut s.before);
        if !after.trim().is_empty() {
            if let Some(found) = labels_of_words(&s.before, true) {
                labels = labels.with(found);
                rest = after.trim();
            }
        }
    }
    tokens(rest, max_year, s);
    // A group anywhere gives its year or its labels.
    for t in &s.tokens {
        match t.group {
            Some(Group::Year(y)) if year == 0 => year = y,
            Some(Group::Labels(l)) => labels = labels.with(l),
            _ => {}
        }
    }
    // From the end: groups with a year or labels, label words, punctuation and
    // one bare year go; the first word always stays.
    while s.tokens.len() > 1 {
        let last = s.tokens[s.tokens.len() - 1];
        match last.group {
            Some(Group::Year(_) | Group::Labels(_)) => {
                s.tokens.pop();
                continue;
            }
            Some(Group::Other) => break,
            None => {}
        }
        s.last.clear();
        fold_into(&rest[last.start..last.end], &mut s.last);
        if s.last.is_empty() {
            s.tokens.pop();
            continue;
        }
        let before = s.tokens[s.tokens.len() - 2];
        if s.tokens.len() > 2 && before.group.is_none() {
            s.before.clear();
            fold_into(&rest[before.start..before.end], &mut s.before);
            if let Some(l) = pair_of(&s.before, &s.last) {
                labels = labels.with(l);
                s.tokens.truncate(s.tokens.len() - 2);
                continue;
            }
        }
        if let Some(l) = label_of(&s.last) {
            labels = labels.with(l);
            s.tokens.pop();
            continue;
        }
        match as_year(&s.last, max_year) {
            Some(y) if year == 0 => {
                year = y;
                s.tokens.pop();
            }
            _ => break,
        }
    }
    let end = s.tokens.last().map_or(rest.len(), |t| t.end);
    let title = rest[..end].trim_end_matches([' ', '-', ':', '|', '.', ',', '_']);
    NameParts {
        title: if title.is_empty() { rest } else { title },
        year,
        labels,
    }
}

/// The words and bracket groups of `text`, in order, into `s.tokens`.
fn tokens(text: &str, max_year: u16, s: &mut Scratch) {
    s.tokens.clear();
    let bytes = text.as_bytes();
    let mut i = 0;
    // A bracket that finds no closer in the rest of the name tells that no
    // later bracket of its kind will: each kind is searched for until then,
    // not once per bracket, so a name of n unclosed ones costs n steps, not
    // n squared.
    let (mut round_closes, mut square_closes) = (true, true);
    while i < bytes.len() {
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let close = match bytes[i] {
            b'(' if round_closes => Some(b')'),
            b'[' if square_closes => Some(b']'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(len) = text[i + 1..].bytes().position(|b| b == close) {
                let end = i + len + 2;
                s.last.clear();
                fold_into(&text[i + 1..i + 1 + len], &mut s.last);
                let group = group_of(&s.last, max_year);
                s.tokens.push(Token {
                    start: i,
                    end,
                    group: Some(group),
                });
                i = end;
                continue;
            }
            if close == b')' {
                round_closes = false;
            } else {
                square_closes = false;
            }
        }
        // A word runs to the next space or bracket; an unclosed bracket is a
        // word of its own.
        let len = match text[i..].find(|c: char| c.is_whitespace() || c == '(' || c == '[') {
            Some(0) => c.len_utf8(),
            Some(n) => n,
            None => text.len() - i,
        };
        s.tokens.push(Token {
            start: i,
            end: i + len,
            group: None,
        });
        i += len;
    }
}

/// What a group in brackets holds (`folded`: what is inside, folded): a year
/// alone, labels only, or else.
fn group_of(folded: &str, max_year: u16) -> Group {
    if let Some(y) = as_year(folded, max_year) {
        return Group::Year(y);
    }
    match labels_of_words(folded, true) {
        Some(l) => Group::Labels(l),
        None => Group::Other,
    }
}

/// The labels of a category's name: those [`parse_name`] reads, and the label
/// words the name starts with ("Nederlands", "NL FILMS", "NL Gesproken Kids"):
/// a portal names a section after its language, with or without a `|`. A
/// title's name of one label keeps it as its title. A category's year means
/// nothing.
pub fn category_labels(name: &str) -> Labels {
    let mut labels = parse_name(name, u16::MAX).labels;
    let folded = fold(name);
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    while let Some(w) = words.next() {
        if let Some(l) = words.peek().and_then(|next| pair_of(w, next)) {
            labels = labels.with(l);
            words.next();
        } else if let Some(l) = label_of(w) {
            labels = labels.with(l);
        } else {
            break;
        }
    }
    labels
}
