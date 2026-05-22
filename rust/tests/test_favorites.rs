use tvplayer::favorites::Favorites;

#[test]
fn toggle_adds_then_removes() {
    let mut f = Favorites::default();
    assert!(!f.contains(101));
    f.toggle(101, "Een");
    assert!(f.contains(101));
    f.toggle(101, "Een");
    assert!(!f.contains(101));
}

#[test]
fn order_preserves_insertion() {
    let mut f = Favorites::default();
    f.toggle(1, "A");
    f.toggle(2, "B");
    f.toggle(3, "C");
    let names: Vec<&str> = f.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["A", "B", "C"]);
}

#[test]
fn roundtrip_through_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fav.json");
    let mut f = Favorites::default();
    f.toggle(7, "NPO 1");
    f.toggle(11, "RTL 4");
    f.save(&path).unwrap();

    let loaded = Favorites::load(&path).unwrap();
    assert_eq!(loaded.iter().count(), 2);
    assert!(loaded.contains(7));
    assert!(loaded.contains(11));
}

#[test]
fn load_missing_file_returns_empty() {
    let f = Favorites::load(std::path::Path::new("nonexistent_file_xyz.json")).unwrap();
    assert_eq!(f.iter().count(), 0);
}

#[test]
fn remove_idempotent_on_absent_id() {
    let mut f = Favorites::default();
    f.toggle(1, "A");
    f.remove(1);
    f.remove(1); // no panic
    assert_eq!(f.iter().count(), 0);
}
