use tvplayer::storage::Storage;

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
