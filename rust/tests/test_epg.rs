use bongo_player::epg::{Epg, EpgEntry};
use chrono::{TimeZone, Utc};

fn e(title: &str, start_min: i64, end_min: i64) -> EpgEntry {
    EpgEntry {
        title: title.into(),
        start: Utc.timestamp_opt(start_min * 60, 0).unwrap(),
        end: Utc.timestamp_opt(end_min * 60, 0).unwrap(),
        has_archive: None,
    }
}

/// (title, start minute, end minute) of each entry.
fn spans(epg: &Epg) -> Vec<(String, i64, i64)> {
    epg.entries()
        .iter()
        .map(|x| {
            (
                x.title.clone(),
                x.start.timestamp() / 60,
                x.end.timestamp() / 60,
            )
        })
        .collect()
}

fn span(title: &str, start: i64, end: i64) -> (String, i64, i64) {
    (title.to_string(), start, end)
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

#[test]
fn the_same_programme_twice_becomes_one() {
    // 09:30 and 09:35 "Margreet Dolman Begrijpt 'T": two LIVE rows in the 0.5 guide.
    let epg = Epg::new(vec![
        e("Margreet Dolman Begrijpt 'T", 570, 600),
        e(" margreet dolman begrijpt 't", 575, 605),
        e("Hour Of Power", 605, 660),
    ]);
    assert_eq!(
        spans(&epg),
        [
            span("Margreet Dolman Begrijpt 'T", 570, 605),
            span("Hour Of Power", 605, 660)
        ]
    );
}

#[test]
fn a_programme_that_starts_while_another_runs_ends_that_one() {
    let epg = Epg::new(vec![
        e("LIVE: Hart van Nederland", 585, 600),
        e("De Grote Tuinverbouwing", 597, 640),
    ]);
    assert_eq!(
        spans(&epg),
        [
            span("LIVE: Hart van Nederland", 585, 597),
            span("De Grote Tuinverbouwing", 597, 640)
        ]
    );
}

#[test]
fn of_two_programmes_that_start_together_the_first_stays() {
    let epg = Epg::new(vec![e("Vis TV", 570, 600), e("Bluey", 570, 590)]);
    assert_eq!(spans(&epg), [span("Vis TV", 570, 600)]);
}

#[test]
fn a_programme_inside_another_is_dropped() {
    let epg = Epg::new(vec![
        e("Film", 600, 720),
        e("Flits", 630, 635),
        e("Na de film", 720, 750),
    ]);
    assert_eq!(
        spans(&epg),
        [span("Film", 600, 720), span("Na de film", 720, 750)]
    );
}

#[test]
fn entries_that_end_before_they_start_go() {
    let epg = Epg::new(vec![
        e("Leeg", 600, 600),
        e("Achterstevoren", 610, 605),
        e("Goed", 620, 650),
    ]);
    assert_eq!(spans(&epg), [span("Goed", 620, 650)]);
}

#[test]
fn merged_entries_are_watchable_when_either_says_so() {
    let mut a = e("Nieuws", 600, 630);
    a.has_archive = Some(false);
    let mut b = e("Nieuws", 605, 635);
    b.has_archive = Some(true);
    assert_eq!(Epg::new(vec![a, b]).entries()[0].has_archive, Some(true));
    let unknown = Epg::new(vec![e("Nieuws", 600, 630), e("Nieuws", 605, 635)]);
    assert_eq!(unknown.entries()[0].has_archive, None);
}

#[test]
fn no_two_programmes_ever_run_at_the_same_moment() {
    // A messy listing: 300 entries of 5 titles at pseudo-random times, some empty.
    let mut seed: u64 = 42;
    let mut next = |m: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % m
    };
    let titles = ["A", "B", "C", "D", "E"];
    let raw: Vec<EpgEntry> = (0..300)
        .map(|_| {
            let start = next(10_000) as i64;
            let len = next(120) as i64;
            e(titles[next(5) as usize], start, start + len)
        })
        .collect();
    let epg = Epg::new(raw);
    let xs = epg.entries();
    assert!(xs.len() > 10);
    for x in xs {
        assert!(x.end > x.start, "{x:?} does not end after it starts");
    }
    for w in xs.windows(2) {
        assert!(w[0].end <= w[1].start, "{:?} overlaps {:?}", w[0], w[1]);
    }
}
