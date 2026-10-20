//! The thuisbioscoop's data (spec 2026-10-15 §7): titles, genres, versions,
//! details, covers and what was watched. It draws nothing.

pub mod cache;
pub mod covers;
pub mod details;
pub mod genres;
pub mod index;
pub mod versions;
pub mod watching;

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
/// a title, a version and a series of the lists, for the order of a film's
/// versions (`versions::merge` puts an id that does not fit last) and for
/// asking after one. A file's ids are `u32` by the file's own type and do not
/// come through here: the fence for them is `TitleIndex::assemble`'s, which
/// leaves out a title whose id is 0 and a version whose stream id is 0.
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

/// Writes a file of the thuisbioscoop through a temporary one beside it
/// (`<name>.json.tmp`) that is renamed over it, so a write that fails leaves
/// the old file whole and no temporary file behind (spec 2026-10-15 §6).
/// `write` fills the temporary file, buffered.
pub(crate) fn write_atomic_with(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::File::create(&tmp)
        .and_then(|file| {
            // 64 KiB: serde writes in small pieces, and the cache is 9 MB.
            let mut out = std::io::BufWriter::with_capacity(64 * 1024, file);
            write(&mut out)?;
            out.flush()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::write_atomic_with;
    use std::io::Write as _;

    /// The new text is written beside the old file, which stays whole until
    /// the rename: a crash or a full disk halfway never leaves half a file.
    #[test]
    fn a_file_is_written_beside_the_old_one_and_renamed_over_it() {
        let dir = tempfile::tempdir().unwrap();
        // The folder is made when it is missing.
        let path = dir.path().join("cache").join("vod.json");
        let tmp = path.with_extension("json.tmp");
        write_atomic_with(&path, |out| out.write_all(b"old")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(!tmp.exists());
        let mut seen = None;
        write_atomic_with(&path, |out| {
            out.write_all(b"the new one, a good deal longer")?;
            out.flush()?;
            seen = Some((std::fs::read(&path)?, std::fs::read(&tmp)?));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            seen,
            Some((b"old".to_vec(), b"the new one, a good deal longer".to_vec())),
            "while it is written: the old file whole, the new text beside it"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"the new one, a good deal longer"
        );
        assert!(!tmp.exists());
    }

    #[test]
    fn a_write_that_fails_halfway_keeps_the_old_file_whole_and_leaves_no_temporary_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod.json");
        let tmp = path.with_extension("json.tmp");
        write_atomic_with(&path, |out| out.write_all(b"old")).unwrap();
        let failed = write_atomic_with(&path, |out| {
            out.write_all(b"half of the new one")?;
            out.flush()?;
            Err(std::io::Error::other("disk full"))
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(!tmp.exists());
    }

    /// Kills a5 (the temporary file is made with `create_new`): what a crash
    /// leaves behind is written over by the next write, not a reason to fail.
    #[test]
    fn a_temporary_file_left_by_a_crash_does_not_stop_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            b"half of a write that never ended, longer than the new text",
        )
        .unwrap();
        write_atomic_with(&path, |out| out.write_all(b"new")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!tmp.exists());
    }

    /// 6b reads the file on the blocking pool while a build may be writing it.
    /// std opens a file with all three share flags, so a reader that holds the
    /// old file open does not stop the rename, and keeps reading the old file.
    #[test]
    fn a_reader_that_holds_the_file_open_does_not_stop_the_write() {
        use std::io::Read as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod.json");
        write_atomic_with(&path, |out| out.write_all(b"old old old")).unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        write_atomic_with(&path, |out| out.write_all(b"new")).unwrap();
        let mut seen = String::new();
        reader.read_to_string(&mut seen).unwrap();
        assert_eq!(seen, "old old old");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
