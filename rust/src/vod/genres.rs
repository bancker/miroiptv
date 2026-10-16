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

use super::fold;
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
