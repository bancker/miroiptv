use bongo_player::args::ini_path;
use bongo_player::legacy::{legacy_config_dir, legacy_ini_path, migrate_config_dir, migrate_file};
use std::fs;
use std::path::{Path, PathBuf};

// The pre-rename locations are history: if they drift, migration silently
// finds nothing and users' favorites look lost.
#[test]
fn legacy_config_dir_is_where_tvplayer_kept_its_data() {
    let dir = legacy_config_dir().unwrap();
    assert!(
        dir.ends_with(Path::new("tvplayer").join("config")),
        "{}",
        dir.display()
    );
}

#[test]
fn legacy_ini_is_tvplayer_ini_next_to_the_new_ini() {
    let legacy = legacy_ini_path();
    assert_eq!(legacy.file_name().unwrap(), "tvplayer.ini");
    assert_eq!(legacy.parent(), ini_path().parent());
}

/// Config dir as the pre-rename app left it: data files plus a log/ subdir.
fn legacy_config(root: &Path) -> PathBuf {
    let dir = root.join("tvplayer").join("config");
    fs::create_dir_all(dir.join("log")).unwrap();
    fs::write(dir.join("favorites.json"), r#"[{"stream_id":101}]"#).unwrap();
    fs::write(dir.join("last_watched.json"), r#"{"stream_id":7}"#).unwrap();
    fs::write(dir.join("presets.json"), r#"{"1":"NPO 1"}"#).unwrap();
    fs::write(dir.join("log").join("tvplayer.log.2026-10-10"), "old log").unwrap();
    dir
}

fn new_config(root: &Path) -> PathBuf {
    root.join("BongoPlayer").join("config")
}

#[test]
fn first_start_copies_legacy_data_files() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 3);
    assert_eq!(
        fs::read_to_string(new.join("favorites.json")).unwrap(),
        r#"[{"stream_id":101}]"#
    );
    assert_eq!(
        fs::read_to_string(new.join("last_watched.json")).unwrap(),
        r#"{"stream_id":7}"#
    );
    assert_eq!(
        fs::read_to_string(new.join("presets.json")).unwrap(),
        r#"{"1":"NPO 1"}"#
    );
}

#[test]
fn logs_are_not_migrated() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());

    migrate_config_dir(&legacy, &new).unwrap();
    assert!(!new.join("log").exists());
}

#[test]
fn legacy_data_stays_as_backup() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());

    migrate_config_dir(&legacy, &new).unwrap();
    assert_eq!(
        fs::read_to_string(legacy.join("favorites.json")).unwrap(),
        r#"[{"stream_id":101}]"#
    );
}

#[test]
fn existing_config_is_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("favorites.json"), "[]").unwrap();

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 0);
    assert_eq!(
        fs::read_to_string(new.join("favorites.json")).unwrap(),
        "[]"
    );
}

#[test]
fn fresh_install_without_legacy_data_is_not_an_error() {
    let root = tempfile::tempdir().unwrap();
    let legacy = root.path().join("tvplayer").join("config");
    let new = new_config(root.path());

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 0);
}

/// Hold `path` open with no sharing, so any copy of it fails (Windows only).
#[cfg(windows)]
fn lock_exclusively(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn failed_copy_leaves_no_half_filled_config() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());
    let _lock = lock_exclusively(&legacy.join("presets.json"));

    assert!(migrate_config_dir(&legacy, &new).is_err());
    assert!(
        !new.exists(),
        "a half-filled config dir would block the retry on the next start"
    );
}

#[cfg(windows)]
#[test]
fn retry_after_failed_copy_migrates_everything() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path());
    let new = new_config(root.path());
    {
        let _lock = lock_exclusively(&legacy.join("presets.json"));
        let _ = migrate_config_dir(&legacy, &new);
    }

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 3);
    assert_eq!(
        fs::read_to_string(new.join("presets.json")).unwrap(),
        r#"{"1":"NPO 1"}"#
    );
}

#[test]
fn legacy_ini_is_renamed_on_first_start() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("tvplayer.ini");
    let new = dir.path().join("BongoPlayer.ini");
    fs::write(&legacy, "xtream=u:p@h.example.com:8080").unwrap();

    assert!(migrate_file(&legacy, &new).unwrap());
    assert_eq!(
        fs::read_to_string(&new).unwrap(),
        "xtream=u:p@h.example.com:8080"
    );
    assert!(
        !legacy.exists(),
        "two ini files would leave users guessing which one counts"
    );
}

#[test]
fn existing_ini_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("tvplayer.ini");
    let new = dir.path().join("BongoPlayer.ini");
    fs::write(&legacy, "old:pass@h.example.com:1").unwrap();
    fs::write(&new, "new:pass@h.example.com:2").unwrap();

    assert!(!migrate_file(&legacy, &new).unwrap());
    assert_eq!(
        fs::read_to_string(&new).unwrap(),
        "new:pass@h.example.com:2"
    );
    assert_eq!(
        fs::read_to_string(&legacy).unwrap(),
        "old:pass@h.example.com:1"
    );
}

#[test]
fn no_legacy_ini_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("BongoPlayer.ini");

    assert!(!migrate_file(&dir.path().join("tvplayer.ini"), &new).unwrap());
    assert!(!new.exists());
}
