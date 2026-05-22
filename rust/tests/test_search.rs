use tvplayer::search::{rank, ItemKind, SearchItem};

fn item(name: &str, kind: ItemKind) -> SearchItem {
    SearchItem { id: 1, name: name.into(), kind }
}

#[test]
fn empty_query_returns_empty() {
    let items = vec![item("NPO 1", ItemKind::Live)];
    assert!(rank("", &items).is_empty());
}

#[test]
fn substring_match_case_insensitive() {
    let items = vec![item("NPO 1", ItemKind::Live), item("RTL 4", ItemKind::Live)];
    let r = rank("npo", &items);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "NPO 1");
}

#[test]
fn live_ranks_above_movie_above_series_for_same_match_length() {
    let items = vec![
        item("Nieuws (movie)",  ItemKind::Movie),
        item("Nieuws (live)",   ItemKind::Live),
        item("Nieuws (series)", ItemKind::Series),
    ];
    let r = rank("nieuws", &items);
    assert_eq!(r[0].kind, ItemKind::Live);
    assert_eq!(r[1].kind, ItemKind::Movie);
    assert_eq!(r[2].kind, ItemKind::Series);
}

#[test]
fn capped_at_12() {
    let items: Vec<SearchItem> = (0..30).map(|i| item(&format!("Foo {}", i), ItemKind::Movie)).collect();
    let r = rank("foo", &items);
    assert_eq!(r.len(), 12);
}

#[test]
fn whitespace_query_returns_empty() {
    let items = vec![item("X", ItemKind::Live)];
    assert!(rank("   ", &items).is_empty());
}
