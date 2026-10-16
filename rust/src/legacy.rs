//! One-time migration from the names the app had before, so favorites, resume
//! points, presets, the saved lists and the portal ini survive a rename: Bongo
//! Player (`BongoPlayer`, 0.4 to 0.7) and, before it, `tvplayer`. Runs at
//! startup; once migrated it costs one `exists()` per location.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The technical names the app had before it became Bongo IPTV Player, newest
/// first: a user who had both gets the newer one's data.
pub const LEGACY_APP_IDS: [&str; 2] = ["BongoPlayer", "tvplayer"];

/// The one subfolder that comes along: the saved lists, the guide's cache and
/// the logos, so that the first start after a rename is as quick as the one
/// before it. `log/` and anything else stay behind.
const CACHE_DIR: &str = "cache";

/// The config dir an earlier name `id` kept (`%APPDATA%\<id>\config` on
/// Windows).
pub fn legacy_config_dir(id: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", id).map(|p| p.config_dir().to_path_buf())
}

/// `<id>.ini`, the portal ini of an earlier name `id`, next to the new ini.
pub fn legacy_ini_path(id: &str) -> PathBuf {
    crate::args::ini_path().with_file_name(format!("{id}.ini"))
}

/// Copies the config dir of the first of `candidates` (newest first) that is
/// there into `new`, and of that one only: one whose copy fails is tried again
/// at the next start, never passed over for an older one. Which name it was,
/// and how it went; `None` when no earlier name left a config dir.
pub fn migrate_config_from<'a>(
    candidates: &[(&'a str, PathBuf)],
    new: &Path,
) -> Option<(&'a str, io::Result<usize>)> {
    let (id, old) = candidates.iter().find(|(_, old)| old.is_dir())?;
    Some((id, migrate_config_dir(old, new)))
}

/// Moves the ini of the first of `candidates` (newest first) that is there to
/// `new`, the same way. Which name it was, and how it went.
pub fn migrate_ini_from<'a>(
    candidates: &[(&'a str, PathBuf)],
    new: &Path,
) -> Option<(&'a str, io::Result<bool>)> {
    let (id, old) = candidates.iter().find(|(_, old)| old.is_file())?;
    Some((id, migrate_file(old, new)))
}

/// On first start (no `new` dir yet) copy the files of `legacy` into `new`,
/// and its `cache/` with everything in it. Other subdirs such as `log/` are
/// skipped and `legacy` stays as a backup. The copy goes through a sibling
/// temp dir that is renamed into place, so a failure never leaves a
/// half-filled `new` that would block the retry next start. Returns the number
/// of files copied.
pub fn migrate_config_dir(legacy: &Path, new: &Path) -> io::Result<usize> {
    if new.exists() || !legacy.is_dir() {
        return Ok(0);
    }
    let tmp = new.with_extension("migrating");
    let _ = fs::remove_dir_all(&tmp); // leftover of an earlier failed attempt
    fs::create_dir_all(&tmp)?;
    let result = copy_files(legacy, &tmp).and_then(|n| fs::rename(&tmp, new).map(|()| n));
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

/// The files of `from` and its `cache/` subdir into `to`.
fn copy_files(from: &Path, to: &Path) -> io::Result<usize> {
    let mut copied = 0;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
            copied += 1;
        } else if kind.is_dir() && entry.file_name() == CACHE_DIR {
            copied += copy_tree(&entry.path(), &to.join(CACHE_DIR))?;
        }
    }
    Ok(copied)
}

/// Every file under `from` into `to`, subdirs and all. A link is no file and
/// no dir here: it is left out, so a link to a parent cannot loop.
fn copy_tree(from: &Path, to: &Path) -> io::Result<usize> {
    fs::create_dir_all(to)?;
    let mut copied = 0;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
            copied += 1;
        } else if kind.is_dir() {
            copied += copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(copied)
}

/// Rename `legacy` to `new` unless `new` already exists, so there is only ever
/// one ini. Returns whether the file was moved.
pub fn migrate_file(legacy: &Path, new: &Path) -> io::Result<bool> {
    if new.exists() || !legacy.is_file() {
        return Ok(false);
    }
    fs::rename(legacy, new)?;
    Ok(true)
}
