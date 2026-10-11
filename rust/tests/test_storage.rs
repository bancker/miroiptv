use bongo_player::storage::Storage;

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
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("BONGO_CONFIG_DIR", dir.path());
    std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
    let s = bongo_player::storage::Storage::standard().unwrap();
    let ini = bongo_player::args::ini_path();
    std::env::remove_var("BONGO_CONFIG_DIR");
    std::env::remove_var("BONGO_INI");
    assert_eq!(s.config_dir(), dir.path());
    assert_eq!(s.settings_path(), dir.path().join("settings.json"));
    assert_eq!(ini, dir.path().join("test.ini"));
}
