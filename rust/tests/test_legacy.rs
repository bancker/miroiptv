use bongo_player::args::ini_path;
use bongo_player::legacy::{
    legacy_config_dir, legacy_ini_path, migrate_config_dir, migrate_config_from, migrate_file,
    migrate_ini_from, LEGACY_APP_IDS,
};
use std::fs;
use std::path::{Path, PathBuf};

// The earlier locations are history: if they drift, migration silently finds
// nothing and users' favorites look lost.
#[test]
fn the_earlier_names_are_bongo_player_and_before_it_tvplayer() {
    assert_eq!(LEGACY_APP_IDS, ["BongoPlayer", "tvplayer"], "newest first");
    for id in LEGACY_APP_IDS {
        let dir = legacy_config_dir(id).unwrap();
        assert!(
            dir.ends_with(Path::new(id).join("config")),
            "{}",
            dir.display()
        );
    }
}

#[test]
fn an_earlier_ini_is_next_to_the_new_ini() {
    for id in LEGACY_APP_IDS {
        let legacy = legacy_ini_path(id);
        assert_eq!(legacy.file_name().unwrap(), format!("{id}.ini").as_str());
        assert_eq!(legacy.parent(), ini_path().parent());
    }
}

/// Config dir as an earlier name `id` left it: data files, a log/ subdir and a
/// cache/ subdir with a folder of its own. The favorites name the id.
fn legacy_config(root: &Path, id: &str) -> PathBuf {
    let dir = root.join(id).join("config");
    fs::create_dir_all(dir.join("log")).unwrap();
    fs::create_dir_all(dir.join("cache").join("logos")).unwrap();
    fs::write(dir.join("favorites.json"), format!(r#"[{{"of":"{id}"}}]"#)).unwrap();
    fs::write(dir.join("last_watched.json"), r#"{"stream_id":7}"#).unwrap();
    fs::write(dir.join("presets.json"), r#"{"1":"NPO 1"}"#).unwrap();
    fs::write(
        dir.join("log").join(format!("{id}.log.2026-10-10")),
        "old log",
    )
    .unwrap();
    fs::write(dir.join("cache").join("m3u-2.m3u"), "#EXTM3U\n").unwrap();
    fs::write(dir.join("cache").join("logos").join("1.png"), "png").unwrap();
    dir
}

fn new_config(root: &Path) -> PathBuf {
    root.join("BongoIPTVPlayer").join("config")
}

#[test]
fn first_start_copies_the_data_files_and_the_cache() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 5);
    assert_eq!(
        fs::read_to_string(new.join("favorites.json")).unwrap(),
        r#"[{"of":"BongoPlayer"}]"#
    );
    assert_eq!(
        fs::read_to_string(new.join("last_watched.json")).unwrap(),
        r#"{"stream_id":7}"#
    );
    assert_eq!(
        fs::read_to_string(new.join("presets.json")).unwrap(),
        r#"{"1":"NPO 1"}"#
    );
    // The saved list and the logos: the first start is as quick as before.
    assert_eq!(
        fs::read_to_string(new.join("cache").join("m3u-2.m3u")).unwrap(),
        "#EXTM3U\n"
    );
    assert_eq!(
        fs::read_to_string(new.join("cache").join("logos").join("1.png")).unwrap(),
        "png"
    );
}

#[test]
fn logs_are_not_migrated() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());

    migrate_config_dir(&legacy, &new).unwrap();
    assert!(!new.join("log").exists());
}

#[test]
fn legacy_data_stays_as_backup() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());

    migrate_config_dir(&legacy, &new).unwrap();
    assert_eq!(
        fs::read_to_string(legacy.join("favorites.json")).unwrap(),
        r#"[{"of":"BongoPlayer"}]"#
    );
    assert!(legacy.join("cache").join("m3u-2.m3u").exists());
}

#[test]
fn existing_config_is_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path(), "BongoPlayer");
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
    let legacy = root.path().join("BongoPlayer").join("config");
    let new = new_config(root.path());

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 0);
}

/// The earlier names' config dirs under `root`, newest first, as main.rs
/// hands them over.
fn candidates(root: &Path) -> Vec<(&'static str, PathBuf)> {
    LEGACY_APP_IDS
        .iter()
        .map(|id| (*id, root.join(id).join("config")))
        .collect()
}

#[test]
fn the_newest_earlier_name_wins() {
    let root = tempfile::tempdir().unwrap();
    legacy_config(root.path(), "tvplayer");
    legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());

    let (id, copied) = migrate_config_from(&candidates(root.path()), &new).unwrap();
    assert_eq!((id, copied.unwrap()), ("BongoPlayer", 5));
    assert_eq!(
        fs::read_to_string(new.join("favorites.json")).unwrap(),
        r#"[{"of":"BongoPlayer"}]"#
    );
    // Once there, a later start copies nothing more.
    let (_, copied) = migrate_config_from(&candidates(root.path()), &new).unwrap();
    assert_eq!(copied.unwrap(), 0);
}

#[test]
fn tvplayer_comes_along_when_bongo_player_left_nothing() {
    let root = tempfile::tempdir().unwrap();
    legacy_config(root.path(), "tvplayer");
    let new = new_config(root.path());

    let (id, copied) = migrate_config_from(&candidates(root.path()), &new).unwrap();
    assert_eq!((id, copied.unwrap()), ("tvplayer", 5));
    assert_eq!(
        fs::read_to_string(new.join("favorites.json")).unwrap(),
        r#"[{"of":"tvplayer"}]"#
    );
}

#[test]
fn with_no_earlier_name_there_is_nothing_to_migrate() {
    let root = tempfile::tempdir().unwrap();
    assert!(migrate_config_from(&candidates(root.path()), &new_config(root.path())).is_none());
    let inis: Vec<(&str, PathBuf)> = LEGACY_APP_IDS
        .iter()
        .map(|id| (*id, root.path().join(format!("{id}.ini"))))
        .collect();
    assert!(migrate_ini_from(&inis, &root.path().join("BongoIPTVPlayer.ini")).is_none());
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
    let legacy = legacy_config(root.path(), "BongoPlayer");
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
fn a_failed_copy_of_the_newer_name_is_never_passed_over_for_the_older() {
    let root = tempfile::tempdir().unwrap();
    legacy_config(root.path(), "tvplayer");
    let newer = legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());
    {
        let _lock = lock_exclusively(&newer.join("cache").join("m3u-2.m3u"));
        let (id, copied) = migrate_config_from(&candidates(root.path()), &new).unwrap();
        assert_eq!(id, "BongoPlayer");
        assert!(copied.is_err());
        assert!(
            !new.exists(),
            "tvplayer's older data did not take its place"
        );
    }
    // The next start tries the newer one again.
    let (id, copied) = migrate_config_from(&candidates(root.path()), &new).unwrap();
    assert_eq!((id, copied.unwrap()), ("BongoPlayer", 5));
}

#[cfg(windows)]
#[test]
fn retry_after_failed_copy_migrates_everything() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_config(root.path(), "BongoPlayer");
    let new = new_config(root.path());
    {
        let _lock = lock_exclusively(&legacy.join("presets.json"));
        let _ = migrate_config_dir(&legacy, &new);
    }

    assert_eq!(migrate_config_dir(&legacy, &new).unwrap(), 5);
    assert_eq!(
        fs::read_to_string(new.join("presets.json")).unwrap(),
        r#"{"1":"NPO 1"}"#
    );
}

#[test]
fn legacy_ini_is_renamed_on_first_start() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("BongoPlayer.ini");
    let new = dir.path().join("BongoIPTVPlayer.ini");
    fs::write(&legacy, "portal=u:p@h.example.com:8080").unwrap();

    assert!(migrate_file(&legacy, &new).unwrap());
    assert_eq!(
        fs::read_to_string(&new).unwrap(),
        "portal=u:p@h.example.com:8080"
    );
    assert!(
        !legacy.exists(),
        "two ini files would leave users guessing which one counts"
    );
}

#[test]
fn the_newest_earlier_ini_is_the_one_that_moves() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("tvplayer.ini"), "old:pass@h.example.com:1").unwrap();
    fs::write(
        dir.path().join("BongoPlayer.ini"),
        "new:pass@h.example.com:2",
    )
    .unwrap();
    let inis: Vec<(&str, PathBuf)> = LEGACY_APP_IDS
        .iter()
        .map(|id| (*id, dir.path().join(format!("{id}.ini"))))
        .collect();
    let new = dir.path().join("BongoIPTVPlayer.ini");

    let (id, moved) = migrate_ini_from(&inis, &new).unwrap();
    assert_eq!((id, moved.unwrap()), ("BongoPlayer", true));
    assert_eq!(
        fs::read_to_string(&new).unwrap(),
        "new:pass@h.example.com:2"
    );
    assert!(dir.path().join("tvplayer.ini").exists(), "left alone");
    // tvplayer.ini is the first one there now, and the new ini stays as it is.
    let (id, moved) = migrate_ini_from(&inis, &new).unwrap();
    assert_eq!((id, moved.unwrap()), ("tvplayer", false));
    assert_eq!(
        fs::read_to_string(&new).unwrap(),
        "new:pass@h.example.com:2"
    );
}

#[test]
fn existing_ini_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("BongoPlayer.ini");
    let new = dir.path().join("BongoIPTVPlayer.ini");
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
    let new = dir.path().join("BongoIPTVPlayer.ini");

    assert!(!migrate_file(&dir.path().join("BongoPlayer.ini"), &new).unwrap());
    assert!(!new.exists());
}
