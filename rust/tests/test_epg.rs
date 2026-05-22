use chrono::{TimeZone, Utc};
use tvplayer::epg::{Epg, EpgEntry};

fn e(title: &str, start_min: i64, end_min: i64) -> EpgEntry {
    EpgEntry {
        title: title.into(),
        start: Utc.timestamp_opt(start_min * 60, 0).unwrap(),
        end:   Utc.timestamp_opt(end_min   * 60, 0).unwrap(),
    }
}

#[test]
fn current_returns_active_entry() {
    let epg = Epg::new(vec![e("A", 0, 30), e("B", 30, 60), e("C", 60, 90)]);
    let t = Utc.timestamp_opt(45 * 60, 0).unwrap();
    assert_eq!(epg.current_at(t).unwrap().title, "B");
}

#[test]
fn current_returns_none_outside_schedule() {
    let epg = Epg::new(vec![e("A", 0, 30)]);
    let t = Utc.timestamp_opt(60 * 60, 0).unwrap();
    assert!(epg.current_at(t).is_none());
}

#[test]
fn boundary_inclusive_start_exclusive_end() {
    let epg = Epg::new(vec![e("A", 0, 30), e("B", 30, 60)]);
    let t = Utc.timestamp_opt(30 * 60, 0).unwrap();
    assert_eq!(epg.current_at(t).unwrap().title, "B");
}

#[test]
fn next_after_current() {
    let epg = Epg::new(vec![e("A", 0, 30), e("B", 30, 60)]);
    let t = Utc.timestamp_opt(15 * 60, 0).unwrap();
    assert_eq!(epg.next_at(t).unwrap().title, "B");
}

#[test]
fn next_none_at_end_of_schedule() {
    let epg = Epg::new(vec![e("A", 0, 30)]);
    let t = Utc.timestamp_opt(15 * 60, 0).unwrap();
    assert!(epg.next_at(t).is_none());
}

#[test]
fn new_sorts_entries_by_start() {
    let epg = Epg::new(vec![e("B", 30, 60), e("A", 0, 30)]);
    assert_eq!(epg.entries()[0].title, "A");
}
