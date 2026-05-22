use crate::catalog::CatalogStore;

/// Resolve the next live channel index given current + direction (-1 / +1).
/// `current` is None if no channel is currently selected.
pub fn next_live_idx(current: Option<usize>, len: usize, delta: i32) -> Option<usize> {
    if len == 0 { return None; }
    let cur = current.unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(len as i32);
    Some(next as usize)
}

pub fn find_live_by_name(catalog: &CatalogStore, needle: &str) -> Option<i64> {
    let live = catalog.live_channels();
    let n = needle.to_lowercase();
    live.iter().find(|c| c.name.to_lowercase().contains(&n)).map(|c| c.stream_id)
}

pub fn npo_shortcut_id(catalog: &CatalogStore, ch_num: u8) -> Option<i64> {
    let needle = format!("NPO {}", ch_num);
    find_live_by_name(catalog, &needle)
}

pub fn news_npo(catalog: &CatalogStore) -> Option<i64> {
    for n in [1u8, 2, 3] {
        if let Some(id) = npo_shortcut_id(catalog, n) {
            return Some(id);
        }
    }
    None
}

pub fn news_rtl(catalog: &CatalogStore) -> Option<i64> {
    for needle in ["RTL Nieuws", "RTL 4", "RTL Z"] {
        if let Some(id) = find_live_by_name(catalog, needle) {
            return Some(id);
        }
    }
    None
}
