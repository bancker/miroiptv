use bongo_player::storage::Storage;
use std::ffi::OsString;
use std::sync::Mutex;

/// The environment is the whole process's: the tests that change it take turns.
static ENV: Mutex<()> = Mutex::new(());

/// Sets (or, with `None`, removes) the test variables for one test, and puts
/// them back as they were when it is dropped.
struct TestEnv {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl TestEnv {
    fn set(vars: &[(&'static str, Option<OsString>)]) -> Self {
        let saved = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        TestEnv { saved }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[test]
fn config_dir_exists_after_ensure() {
    let dir = tempfile::tempdir().unwrap();
    let s = Storage::with_root(dir.path().to_path_buf());
    let p = s.ensure_config_dir().unwrap();
    assert!(p.exists());
}

#[test]
fn favorites_path_in_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let s = Storage::with_root(dir.path().to_path_buf());
    let p = s.favorites_path();
    assert!(p.to_string_lossy().ends_with("favorites.json"));
}

#[test]
fn config_folder_and_ini_can_be_redirected_for_tests() {
    let _turn = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let env = TestEnv::set(&[
        ("BONGO_CONFIG_DIR", Some(dir.path().into())),
        ("BONGO_INI", Some(dir.path().join("test.ini").into())),
    ]);
    let s = bongo_player::storage::Storage::standard().unwrap();
    let ini = bongo_player::args::ini_path();
    drop(env);
    assert_eq!(s.config_dir(), dir.path());
    assert_eq!(s.settings_path(), dir.path().join("settings.json"));
    assert_eq!(ini, dir.path().join("test.ini"));
}

#[test]
fn a_redirected_config_folder_without_an_ini_variable_keeps_the_ini_in_that_folder() {
    // A Save in the settings overwrites the ini. A harness that redirects the
    // config folder and forgets BONGO_INI must not reach the file next to the exe,
    // which may hold the developer's real portal.
    let _turn = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let env = TestEnv::set(&[
        ("BONGO_CONFIG_DIR", Some(dir.path().into())),
        ("BONGO_INI", None),
    ]);
    let ini = bongo_player::args::ini_path();
    drop(env);
    assert_eq!(ini, dir.path().join("BongoPlayer.ini"));
}

#[test]
fn the_ini_variable_wins_and_with_neither_the_ini_is_next_to_the_exe() {
    let _turn = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere.ini");
    let env = TestEnv::set(&[
        ("BONGO_CONFIG_DIR", Some(dir.path().into())),
        ("BONGO_INI", Some(elsewhere.clone().into())),
    ]);
    let explicit = bongo_player::args::ini_path();
    drop(env);
    assert_eq!(explicit, elsewhere);
    let env = TestEnv::set(&[("BONGO_CONFIG_DIR", None), ("BONGO_INI", None)]);
    let default = bongo_player::args::ini_path();
    drop(env);
    let exe_folder = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(default, exe_folder.join("BongoPlayer.ini"));
}

#[test]
fn source_1_keeps_the_0_6_1_file_names_and_the_others_carry_their_id() {
    let root = std::path::PathBuf::from("C:/cfg");
    let mut storage = Storage::with_root(root.clone());
    assert_eq!(storage.source(), 1);
    assert_eq!(storage.favorites_path(), root.join("favorites.json"));
    assert_eq!(storage.presets_path(), root.join("presets.json"));
    assert_eq!(storage.last_watched_path(), root.join("last_watched.json"));
    storage.set_source(2);
    assert_eq!(storage.favorites_path(), root.join("favorites-2.json"));
    assert_eq!(storage.presets_path(), root.join("presets-2.json"));
    assert_eq!(
        storage.last_watched_path(),
        root.join("last_watched-2.json")
    );
    // Shared by every source.
    assert_eq!(storage.settings_path(), root.join("settings.json"));
    assert_eq!(
        storage.m3u_cache_path(2),
        root.join("cache").join("m3u-2.m3u")
    );
}

#[test]
fn removing_a_sources_files_leaves_the_other_sources_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = Storage::with_root(dir.path().to_path_buf());
    std::fs::create_dir_all(storage.cache_dir()).unwrap();
    for id in [1, 2] {
        storage.set_source(id);
        for path in [
            storage.favorites_path(),
            storage.presets_path(),
            storage.last_watched_path(),
            storage.m3u_cache_path(id),
        ] {
            std::fs::write(path, "x").unwrap();
        }
    }
    assert_eq!(storage.remove_source_files(2), 4);
    assert!(!storage.favorites_path().exists(), "source 2's own");
    storage.set_source(1);
    assert!(storage.favorites_path().exists() && storage.m3u_cache_path(1).exists());
    assert_eq!(storage.remove_source_files(2), 0, "nothing left to remove");
}
