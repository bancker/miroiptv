//! One-time migration from the pre-rename `tvplayer` locations, so favorites,
//! resume points, presets and the portal ini survive the switch to Bongo Player.
//! Runs at startup; once migrated it costs one `exists()` per location.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Technical name the app had before it became Bongo Player.
pub const LEGACY_APP_ID: &str = "tvplayer";

/// Config dir of the pre-rename app (`%APPDATA%\tvplayer\config` on Windows).
pub fn legacy_config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", LEGACY_APP_ID).map(|p| p.config_dir().to_path_buf())
}

/// `tvplayer.ini`, the pre-rename portal ini, next to the executable.
pub fn legacy_ini_path() -> PathBuf {
    crate::args::ini_path().with_file_name(format!("{LEGACY_APP_ID}.ini"))
}

/// On first start (no `new` dir yet) copy the files of `legacy` into `new`.
/// Subdirs such as `log/` are skipped and `legacy` stays as a backup. The copy
/// goes through a sibling temp dir that is renamed into place, so a failure
/// never leaves a half-filled `new` that would block the retry next start.
/// Returns the number of files copied.
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

fn copy_files(from: &Path, to: &Path) -> io::Result<usize> {
    let mut copied = 0;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
            copied += 1;
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
