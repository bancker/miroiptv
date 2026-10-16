//! The thuisbioscoop's data (spec 2026-10-15 §7): titles, genres, versions,
//! details, covers and what was watched. It draws nothing.

pub mod genres;
pub mod index;
pub mod versions;

/// What a title is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Series,
    Film,
}

/// A title of the thuisbioscoop: a series by its `series_id`, a film by the
/// `stream_id` of its default version (`versions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TitleId {
    pub kind: Kind,
    pub id: i64,
}

/// A film's, a version's or a series' id as the index keeps it: 1 to `u32::MAX`.
/// Xtream ids are positive and the portal's unreadable id is 0, so 0 fits no
/// more than a negative id does (`u32::try_from` alone takes it). One rule for
/// a title, a version and a series, for the order of a film's versions
/// (`versions::merge` puts an id that does not fit last) and for asking after
/// one; what reads an id from a file applies it too.
pub(crate) fn fit_id(id: i64) -> Option<u32> {
    u32::try_from(id).ok().filter(|&id| id != 0)
}

/// `text` as the search compares it: lower case, accents off ("Pokémon" is
/// "pokemon", "ß" is "ss"), an apostrophe dropped ("Ocean's" is "oceans",
/// "Don’t" is "dont"), every run of other characters one space, no space at
/// either end. Letters of other scripts stay, in lower case.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// [`fold`], appended to `out`.
pub fn fold_into(text: &str, out: &mut String) {
    let start = out.len();
    let mut space = true;
    for c in text.chars() {
        if c.is_ascii() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
                space = false;
            } else if !space && !is_apostrophe(c) {
                out.push(' ');
                space = true;
            }
            continue;
        }
        for lower in c.to_lowercase() {
            match plain(lower) {
                Some(ascii) => {
                    out.push_str(ascii);
                    space = false;
                }
                // An apostrophe joins its word ("Ocean's" is "oceans"): it is
                // no letter and no space. ʼ (U+02BC) is a letter to Unicode, so
                // this comes before the letters.
                None if is_apostrophe(lower) => {}
                None if lower.is_alphanumeric() => {
                    out.push(lower);
                    space = false;
                }
                // A combining accent (a decomposed é) belongs to the letter
                // before it: it adds nothing.
                None if is_combining(lower) => {}
                None if !space => {
                    out.push(' ');
                    space = true;
                }
                None => {}
            }
        }
    }
    if out.len() > start && out.ends_with(' ') {
        out.pop();
    }
}

/// The apostrophes: `'` and `` ` `` (ASCII), the curly `’` (U+2019) and `‘`
/// (U+2018), `ʼ` (U+02BC) and the accent `´` (U+00B4), which keyboards and
/// portals use for one.
fn is_apostrophe(c: char) -> bool {
    matches!(
        c,
        '\'' | '`' | '\u{2019}' | '\u{2018}' | '\u{2bc}' | '\u{b4}'
    )
}

/// The plain letters of a lower-case letter with an accent; none for any other.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Unicode's combining diacritical marks (U+0300 to U+036F).
fn is_combining(c: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&c)
}
