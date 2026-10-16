//! The channel list's groups and selection (channel_list.rs): the order of the
//! groups and of their channels, the defect and favorite channels, the steps.

use bongo_player::catalog::CatalogStore;
use bongo_player::channel_list::{natural_cmp, ChannelGroups, Cursor, GroupKey};
use bongo_player::m3u::stream_id;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::{Catalog, LiveChannel};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn ch(id: i64, name: &str, group: Option<&str>) -> LiveChannel {
    LiveChannel {
        stream_id: id,
        name: name.into(),
        category_id: group.map(Into::into),
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult: false,
        stream_icon: None,
    }
}

fn build(live: Vec<LiveChannel>, defect: &[i64], favorites: &[i64]) -> ChannelGroups {
    let defect: HashMap<i64, i64> = defect.iter().map(|&id| (id, 0)).collect();
    let favorites: HashSet<i64> = favorites.iter().copied().collect();
    ChannelGroups::build(Arc::new(live), |_| true, &defect, &favorites)
}

/// Each group as "<name> <count>"; the fixed ones by a word of their own.
fn summary(g: &ChannelGroups) -> Vec<String> {
    g.groups()
        .iter()
        .map(|group| {
            let name = match &group.key {
                GroupKey::All => "ALL",
                GroupKey::Favorites => "FAV",
                GroupKey::Named(name) => name.as_str(),
                GroupKey::NoGroup => "NONE",
                GroupKey::Defect => "DEFECT",
            };
            format!("{name} {}", group.members.len())
        })
        .collect()
}

fn names(g: &ChannelGroups, group: usize) -> Vec<String> {
    (0..g.rows(group))
        .map(|row| g.channel(group, row).unwrap().name.clone())
        .collect()
}

fn group_of(g: &ChannelGroups, name: &str) -> usize {
    g.find(&GroupKey::Named(name.into())).expect(name)
}

/// Sport and Nieuws, a channel in both, one in "nieuws", one without a group.
fn sample() -> Vec<LiveChannel> {
    vec![
        ch(1, "Sport 10", Some("Sport")),
        ch(2, "sport 2", Some("Sport")),
        ch(3, "Journaal", Some("Nieuws;Sport")),
        ch(4, "Zonder", None),
        ch(5, "Alpha", Some("nieuws")),
        ch(6, "Kids 1", Some("Kinderen")),
    ]
}

#[test]
fn the_groups_come_in_their_order_with_their_counts() {
    let g = build(sample(), &[], &[]);
    assert_eq!(
        summary(&g),
        [
            "ALL 6",
            "Kinderen 1",
            "Nieuws 1",
            "nieuws 1",
            "Sport 3",
            "NONE 1"
        ]
    );
}

#[test]
fn channels_go_a_to_z_with_numbers_by_their_value() {
    let g = build(sample(), &[], &[]);
    assert_eq!(
        names(&g, 0),
        ["Alpha", "Journaal", "Kids 1", "sport 2", "Sport 10", "Zonder"]
    );
    assert_eq!(
        names(&g, group_of(&g, "Sport")),
        ["Journaal", "sport 2", "Sport 10"],
        "a channel of two groups is in both"
    );
}

#[test]
fn a_defect_channel_is_in_defect_channels_only() {
    let g = build(sample(), &[2], &[]);
    assert_eq!(
        summary(&g),
        [
            "ALL 5",
            "Kinderen 1",
            "Nieuws 1",
            "nieuws 1",
            "Sport 2",
            "NONE 1",
            "DEFECT 1"
        ]
    );
    assert_eq!(names(&g, g.find(&GroupKey::Defect).unwrap()), ["sport 2"]);
    assert_eq!(g.channel_count(), 6, "the defect one too");
    // A mark of a channel the list does not have makes no group.
    assert!(build(sample(), &[99], &[])
        .find(&GroupKey::Defect)
        .is_none());
}

#[test]
fn favorites_show_when_the_list_has_one() {
    let g = build(sample(), &[], &[3, 99]);
    assert_eq!(g.find(&GroupKey::Favorites), Some(1), "second, after All");
    assert_eq!(names(&g, 1), ["Journaal"]);
    assert!(build(sample(), &[], &[99])
        .find(&GroupKey::Favorites)
        .is_none());
    // A defect favorite is in Defect channels only: here no Favorites, and its
    // only other group, Nieuws, holds nothing and goes.
    let g = build(sample(), &[3], &[3]);
    assert_eq!(
        summary(&g),
        [
            "ALL 5",
            "Kinderen 1",
            "nieuws 1",
            "Sport 2",
            "NONE 1",
            "DEFECT 1"
        ]
    );
}

#[test]
fn no_group_shows_only_when_a_channel_has_none() {
    let g = build(vec![ch(1, "Een", Some("X"))], &[], &[]);
    assert_eq!(summary(&g), ["ALL 1", "X 1"]);
}

#[test]
fn untidy_group_titles_are_read_as_people_mean_them() {
    let g = build(
        vec![
            ch(1, "Een", Some(" Kinderen ; ;Kinderen")),
            ch(2, "Twee", Some("  ")),
            ch(3, "Drie", Some(";Sport;")),
        ],
        &[],
        &[],
    );
    assert_eq!(summary(&g), ["ALL 3", "Kinderen 1", "Sport 1", "NONE 1"]);
}

#[test]
fn channels_of_one_name_keep_the_files_order() {
    let g = build(
        vec![ch(5, "Een", None), ch(2, "een", None), ch(9, "Een", None)],
        &[],
        &[],
    );
    let ids: Vec<i64> = (0..3).map(|r| g.channel(0, r).unwrap().stream_id).collect();
    assert_eq!(ids, [5, 2, 9]);
}

#[test]
fn what_the_catalog_hides_is_nowhere() {
    let g = ChannelGroups::build(
        Arc::new(sample()),
        |c| c.stream_id != 1,
        &HashMap::new(),
        &HashSet::new(),
    );
    assert_eq!(names(&g, group_of(&g, "Sport")), ["Journaal", "sport 2"]);
    assert_eq!(g.rows(0), 5);
}

#[test]
fn natural_order_reads_like_people_read() {
    assert_eq!(natural_cmp("Sport 2", "Sport 10"), Ordering::Less);
    assert_eq!(natural_cmp("Kanaal 9", "Kanaal 10"), Ordering::Less);
    assert_eq!(natural_cmp("a", "B"), Ordering::Less);
    assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
    assert_eq!(
        natural_cmp("M3U 01", "M3U DOOD"),
        Ordering::Less,
        "digits before letters"
    );
    assert_eq!(natural_cmp("x01", "x1"), Ordering::Equal);
    assert_eq!(natural_cmp("abc", "ab"), Ordering::Greater);
    assert_eq!(natural_cmp("ÉÉN", "één"), Ordering::Equal);
    assert_eq!(
        natural_cmp(
            "Zender 99999999999999999999",
            "Zender 100000000000000000000"
        ),
        Ordering::Less,
        "longer than any integer"
    );
}

#[test]
fn the_list_opens_on_all_channels_on_the_channel_that_plays() {
    let g = build(sample(), &[], &[]);
    assert_eq!(Cursor::open(&g, None, None), Cursor { group: 0, row: 0 });
    assert_eq!(
        Cursor::open(&g, None, Some(1)),
        Cursor { group: 0, row: 4 },
        "Sport 10"
    );
}

#[test]
fn it_opens_in_the_group_used_last_on_what_plays_or_on_the_first_row() {
    let g = build(sample(), &[], &[]);
    let sport = GroupKey::Named("Sport".into());
    let at = group_of(&g, "Sport");
    assert_eq!(
        Cursor::open(&g, Some(&sport), Some(1)),
        Cursor { group: at, row: 2 }
    );
    assert_eq!(
        Cursor::open(&g, Some(&sport), Some(4)),
        Cursor { group: at, row: 0 },
        "Zonder is not in Sport"
    );
    assert_eq!(
        Cursor::open(&g, Some(&GroupKey::Favorites), Some(1)).group,
        0,
        "a group that is gone: All"
    );
}

#[test]
fn rows_stop_at_both_ends() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, None);
    assert!(!c.step(&g, -1));
    assert!(c.step(&g, 1));
    assert_eq!(c.row, 1);
    assert!(c.step(&g, 10));
    assert_eq!(c.row, 5, "a page past the end stops on the last");
    assert!(!c.step(&g, 1));
    assert!(c.to_end(&g, false));
    assert_eq!(c.row, 0);
    assert!(!c.to_end(&g, false));
    assert!(c.to_end(&g, true));
    assert_eq!(c.row, 5);
    assert!(c.step(&g, -3));
    assert_eq!(c.row, 2);
}

#[test]
fn groups_step_and_stop_at_both_ends() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, Some(3));
    assert_eq!(c.row, 1, "Journaal");
    assert!(!c.step_group(&g, -1, Some(3)));
    assert!(c.step_group(&g, 1, Some(3)));
    assert_eq!((c.group, c.row), (1, 0), "Kinderen: Journaal is not in it");
    assert!(c.step_group(&g, 1, Some(3)));
    assert_eq!((c.group, c.row), (2, 0), "Nieuws, on Journaal");
    for _ in 0..10 {
        c.step_group(&g, 1, Some(3));
    }
    assert_eq!(c.group, 5);
    assert!(!c.step_group(&g, 1, Some(3)));
    let sport = group_of(&g, "Sport");
    assert!(c.select_group(&g, sport, Some(3)));
    assert_eq!((c.group, c.row), (sport, 0), "Sport, on Journaal");
    assert!(!c.select_group(&g, sport, Some(3)), "the group it is in");
    assert!(!c.select_group(&g, 99, None));
}

#[test]
fn a_click_selects_a_row_of_the_group() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, None);
    assert!(c.select_row(&g, 3));
    assert_eq!(c.row, 3);
    assert!(!c.select_row(&g, 3));
    assert!(!c.select_row(&g, 6), "past the last row");
    assert_eq!(c.channel(&g).map(|ch| ch.name.as_str()), Some("sport 2"));
}

#[test]
fn the_selection_follows_groups_made_anew() {
    let old = build(sample(), &[], &[]);
    let sport = GroupKey::Named("Sport".into());
    let mut c = Cursor::open(&old, Some(&sport), Some(2));
    assert_eq!(c.channel(&old).map(|ch| ch.name.as_str()), Some("sport 2"));
    // Another copy of the list, with a channel before it: the same channel.
    let mut more = sample();
    more.push(ch(7, "Aaa", Some("Sport")));
    let new = build(more.clone(), &[], &[]);
    c.follow(&old, &new, None);
    assert_eq!(c.channel(&new).map(|ch| ch.name.as_str()), Some("sport 2"));
    assert_eq!(c.row, 2);
    // It is marked: the same row, its neighbour now.
    let marked = build(more, &[2], &[]);
    c.follow(&new, &marked, None);
    assert_eq!(c.group, group_of(&marked, "Sport"));
    assert_eq!(
        c.channel(&marked).map(|ch| ch.name.as_str()),
        Some("Sport 10")
    );
    // Its group is gone: All, on the channel.
    let gone = build(vec![ch(1, "Sport 10", None)], &[], &[]);
    c.follow(&marked, &gone, None);
    assert_eq!(c, Cursor { group: 0, row: 0 });
}

/// Kanaal A to D, without a group, and the selection on Kanaal C in Defect
/// channels, where B, C and D are.
fn on_c_in_defect_channels() -> (ChannelGroups, Cursor) {
    let marked = build(four(), &[2, 3, 4], &[]);
    assert_eq!(summary(&marked), ["ALL 1", "NONE 1", "DEFECT 3"]);
    let mut c = Cursor::open(&marked, Some(&GroupKey::Defect), None);
    assert!(c.step(&marked, 1));
    let name = c.channel(&marked).map(|ch| ch.name.as_str());
    assert_eq!(name, Some("Kanaal C"));
    (marked, c)
}

fn four() -> Vec<LiveChannel> {
    ["Kanaal A", "Kanaal B", "Kanaal C", "Kanaal D"]
        .into_iter()
        .zip(1..)
        .map(|(name, id)| ch(id, name, None))
        .collect()
}

/// A retry in Defect channels that works: the channel's first picture clears
/// its mark, so it leaves Defect channels, which B and D keep, for All
/// channels. The selection is on it and it plays: it follows it there.
#[test]
fn the_selected_channel_that_plays_follows_into_all_channels_when_its_mark_goes() {
    let (marked, mut c) = on_c_in_defect_channels();
    let cleared = build(four(), &[2, 4], &[]);
    c.follow(&marked, &cleared, Some(3));
    assert_eq!(c, Cursor { group: 0, row: 1 }, "All channels: A, C");
    assert_eq!(
        c.channel(&cleared).map(|ch| ch.name.as_str()),
        Some("Kanaal C")
    );
}

/// Ruling 21 for every other channel that leaves its group: the same row, as
/// far as the group goes.
#[test]
fn a_selected_channel_that_leaves_its_group_and_does_not_play_keeps_the_row() {
    let cleared = build(four(), &[2, 4], &[]);
    // C's mark goes while D plays, or while nothing does: the row of C in
    // Defect channels, D's now.
    for playing in [Some(4), None] {
        let (marked, mut c) = on_c_in_defect_channels();
        c.follow(&marked, &cleared, playing);
        let defect = cleared.find(&GroupKey::Defect).unwrap();
        assert_eq!((c.group, c.row), (defect, 1), "{playing:?}");
        let name = c.channel(&cleared).map(|ch| ch.name.as_str());
        assert_eq!(name, Some("Kanaal D"), "{playing:?}");
    }
    // The channel that plays, marked: All channels no longer shows it, so the
    // selection is not followed anywhere either. The same row of All channels.
    let all = build(four(), &[], &[]);
    let mut c = Cursor::open(&all, None, Some(3));
    assert_eq!(c, Cursor { group: 0, row: 2 });
    let c_marked = build(four(), &[3], &[]);
    c.follow(&all, &c_marked, Some(3));
    assert_eq!(c, Cursor { group: 0, row: 2 });
    let name = c.channel(&c_marked).map(|ch| ch.name.as_str());
    assert_eq!(name, Some("Kanaal D"));
}

#[test]
fn the_catalog_builds_the_groups_with_its_own_marks() {
    let list = "#EXTM3U\n\
#EXTINF:-1 group-title=\"Nieuws\",Een\nhttps://a.example/1.m3u8\n\
#EXTINF:-1 group-title=\"Nieuws\",Twee\nhttps://a.example/2.m3u8\n";
    let (portal, catalog) = M3uPortal::from_text(list).unwrap();
    let store = CatalogStore::from_catalog(Arc::new(portal), catalog);
    store.set_defect([(stream_id("Twee"), 0)].into());
    let g = store
        .channel_groups(&HashSet::from([stream_id("Een")]))
        .unwrap();
    assert_eq!(summary(&g), ["ALL 1", "FAV 1", "Nieuws 1", "DEFECT 1"]);
    let (portal, _) = M3uPortal::from_text(list).unwrap();
    let unloaded = CatalogStore::new(Arc::new(portal));
    assert!(unloaded.channel_groups(&HashSet::new()).is_none());
}

/// The spec's budget (§5): 20 ms for 11,000 channels in a release build, under
/// 1 MB kept. Prints the time; asserts the memory, which does not depend on the
/// machine. Task 10 runs it with `--release -- --nocapture`.
#[test]
fn eleven_thousand_channels_are_grouped_within_the_budget() {
    // iptv-org's shape: 29 groups, every seventh channel in two, numbers in names.
    let live: Vec<LiveChannel> = (0..11_000_i64)
        .map(|i| {
            let group = if i % 7 == 0 {
                format!("Groep {};Groep {}", i % 29, (i + 3) % 29)
            } else {
                format!("Groep {}", i % 29)
            };
            ch(
                i,
                &format!("Zender {} HD", (i * 7919) % 11_000),
                Some(&group),
            )
        })
        .collect();
    let defect: HashMap<i64, i64> = (0..11_000_i64).step_by(37).map(|i| (i, 0)).collect();
    let favorites: HashSet<i64> = (0..11_000_i64).step_by(500).collect();
    let live = Arc::new(live);
    let t0 = std::time::Instant::now();
    let g = ChannelGroups::build(live, |_| true, &defect, &favorites);
    let took = t0.elapsed();
    let kb = g.heap_bytes() / 1024;
    println!(
        "grouping: 11000 channels, {} groups, {:.1} ms, {} KB kept",
        g.group_count(),
        took.as_secs_f64() * 1000.0,
        kb
    );
    assert!(g.heap_bytes() < 1024 * 1024, "{kb} KB");
    assert_eq!(g.rows(0) + defect.len(), 11_000);
    assert_eq!(g.channel_count(), 11_000);
}

// ---- beyond the brief's tests: what they leave unpinned ----

/// The brief's catalog test pins the marks; the other filter the catalog owns is
/// the adult lock, and the list takes it from the same predicate as the zap ring.
#[test]
fn the_catalog_leaves_adult_channels_out_of_the_groups_while_locked() {
    let adult = LiveChannel {
        is_adult: true,
        ..ch(2, "Nacht", Some("Volwassen"))
    };
    let catalog = Catalog {
        live: vec![ch(1, "Een", Some("Nieuws")), adult],
        ..Catalog::default()
    };
    let (portal, _) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttps://a.example/1.m3u8\n").unwrap();
    let store = CatalogStore::from_catalog(Arc::new(portal), catalog);
    let favorites = HashSet::from([2]);
    // Locked: nowhere, not in a group of its own and not in the favorites.
    let g = store.channel_groups(&favorites).unwrap();
    assert_eq!(summary(&g), ["ALL 1", "Nieuws 1"]);
    store.set_adult_unlocked(true);
    let g = store.channel_groups(&favorites).unwrap();
    assert_eq!(summary(&g), ["ALL 2", "FAV 1", "Nieuws 1", "Volwassen 1"]);
}

/// Ruling 19: All channels stays when every channel is marked, empty. The
/// selection rests in it: nothing to select, nothing to step to.
#[test]
fn all_channels_stays_when_every_channel_is_marked() {
    let g = build(sample(), &[1, 2, 3, 4, 5, 6], &[]);
    assert_eq!(summary(&g), ["ALL 0", "DEFECT 6"]);
    let mut c = Cursor::open(&g, None, Some(1));
    assert_eq!(c, Cursor { group: 0, row: 0 });
    assert!(c.channel(&g).is_none(), "an empty group selects nothing");
    assert!(!c.step(&g, 1));
    assert!(!c.step(&g, -1));
    assert!(!c.to_end(&g, true));
    assert!(!c.select_row(&g, 0));
    // Defect channels is next to it and holds them all: it is entered on the one that plays.
    assert!(c.step_group(&g, 1, Some(1)));
    assert_eq!(c.channel(&g).map(|ch| ch.stream_id), Some(1));
}

// ---- fix round 1: a Dutch reader's A-Z, the room All keeps, a list without channels ----

/// "Één" is listed with the e's, not after the z's: a letter with an accent sorts
/// as its plain letter (Latin-1, and an accent written as a mark of its own).
/// Numbers still go by their value.
#[test]
fn accented_letters_sort_with_their_plain_letter() {
    assert_eq!(natural_cmp("één", "zender"), Ordering::Less);
    assert_eq!(natural_cmp("Eén", "een"), Ordering::Equal);
    assert_eq!(natural_cmp("Ö1", "Oz"), Ordering::Less);
    assert_eq!(natural_cmp("Çanakkale TV", "Canvas"), Ordering::Less);
    assert_eq!(
        natural_cmp("e\u{301}en", "een"),
        Ordering::Equal,
        "an accent written as a mark of its own"
    );
    assert_eq!(
        natural_cmp("Één 2", "Een 10"),
        Ordering::Less,
        "numbers by value"
    );
    // Every accented letter of Latin-1, in lower and in upper case.
    for (plain, accented) in [
        ('a', "àáâãäå"),
        ('c', "ç"),
        ('e', "èéêë"),
        ('i', "ìíîï"),
        ('n', "ñ"),
        ('o', "òóôõöø"),
        ('u', "ùúûü"),
        ('y', "ýÿ"),
    ] {
        for lower in accented.chars() {
            for c in [lower, lower.to_uppercase().next().unwrap()] {
                assert_eq!(
                    natural_cmp(&format!("{c}!"), &format!("{plain}!")),
                    Ordering::Equal,
                    "{c}"
                );
            }
        }
    }
}

/// A list in the order its reader looks for a channel in: every name with an
/// accent among the plain ones, and Kanaal 2 before Kanaal 10.
#[test]
fn a_list_is_in_the_order_a_dutch_reader_looks_for_a_channel() {
    let unsorted = [
        "Zender 1",
        "Ö1",
        "Tele 2",
        "Télé Québec",
        "Sat.1",
        "Südwest",
        "Oz",
        "Österreich 1",
        "NPO 1",
        "Ketnet",
        "Kanaal 10",
        "Kanaal 2",
        "Eerste",
        "Eén HD",
        "Één",
        "Cartoon Network",
        "Canvas",
        "Çanakkale TV",
        "24 Kitchen",
    ];
    let live = unsorted
        .iter()
        .zip(1..)
        .map(|(name, id)| ch(id, name, None))
        .collect();
    let g = build(live, &[], &[]);
    assert_eq!(
        names(&g, 0),
        [
            "24 Kitchen",
            "Çanakkale TV",
            "Canvas",
            "Cartoon Network",
            "Één",
            "Eén HD",
            "Eerste",
            "Kanaal 2",
            "Kanaal 10",
            "Ketnet",
            "NPO 1",
            "Ö1",
            "Österreich 1",
            "Oz",
            "Sat.1",
            "Südwest",
            "Tele 2",
            "Télé Québec",
            "Zender 1"
        ]
    );
}

/// The list's own groups go in the same order.
#[test]
fn groups_sort_with_their_plain_letter_too() {
    let g = build(
        vec![
            ch(1, "a", Some("Zender")),
            ch(2, "b", Some("Één")),
            ch(3, "c", Some("Eerste")),
            ch(4, "d", Some("Ö1")),
            ch(5, "e", Some("Oz")),
        ],
        &[],
        &[],
    );
    assert_eq!(
        summary(&g),
        ["ALL 5", "Één 1", "Eerste 1", "Ö1 1", "Oz 1", "Zender 1"]
    );
}

/// All channels asks for the room of its channels at once and keeps no more:
/// positions pushed one by one would sit in room for 1,024.
#[test]
fn the_all_group_keeps_no_more_room_than_its_channels() {
    let live = (0..1000)
        .map(|i| ch(i, &format!("Zender {i}"), None))
        .collect();
    let g = build(live, &[], &[]);
    let all = &g.groups()[0].members;
    assert_eq!(all.len(), 1000);
    assert!(all.capacity() <= 1000, "room for {}", all.capacity());
}

/// A list without a channel: All channels, empty, and no other group; the
/// selection rests in it and nothing moves, whatever the step.
#[test]
fn a_list_without_channels_is_one_empty_all_group_and_nothing_moves() {
    let g = build(Vec::new(), &[], &[]);
    assert_eq!(summary(&g), ["ALL 0"]);
    assert!(g.find(&GroupKey::Defect).is_none());
    assert_eq!((g.group_count(), g.rows(0), g.channel_count()), (1, 0, 0));
    assert!(g.channel(0, 0).is_none());
    let mut c = Cursor::open(&g, Some(&GroupKey::Favorites), Some(5));
    assert_eq!(c, Cursor { group: 0, row: 0 });
    assert!(c.channel(&g).is_none(), "nothing to select");
    for delta in [1, -1, isize::MAX, isize::MIN] {
        assert!(!c.step(&g, delta));
        assert!(!c.step_group(&g, delta, Some(5)));
    }
    assert!(!c.to_end(&g, true));
    assert!(!c.to_end(&g, false));
    assert!(!c.select_row(&g, 0));
    assert!(!c.select_group(&g, 0, Some(5)), "the group it is in");
    assert!(!c.select_group(&g, 1, None), "there is no other");
    c.follow(&g, &g, None);
    assert_eq!(c, Cursor { group: 0, row: 0 });
    // Followed into a list that has channels: its first.
    let full = build(sample(), &[], &[]);
    c.follow(&g, &full, None);
    assert_eq!(c, Cursor { group: 0, row: 0 });
    assert_eq!(c.channel(&full).map(|ch| ch.name.as_str()), Some("Alpha"));
    // A catalog that hides every channel gives the same list.
    let hidden = ChannelGroups::build(
        Arc::new(sample()),
        |_| false,
        &HashMap::new(),
        &HashSet::new(),
    );
    assert_eq!(summary(&hidden), ["ALL 0"]);
}

// ---- fix round 1b: the letters of Latin Extended-A, and the ligatures ----

/// Every letter of Latin Extended-A (U+0100 to U+017F), in both cases, sorts as
/// the plain letter or letters it is read as: Turkish, Polish, Czech, Hungarian,
/// Nordic and Baltic names find their place among the plain ones.
#[test]
fn every_letter_of_latin_extended_a_sorts_as_its_plain_letters() {
    const LETTERS: [(&str, &str); 21] = [
        ("a", "ĀāĂăĄą"),
        ("c", "ĆćĈĉĊċČč"),
        ("d", "ĎďĐđ"),
        ("e", "ĒēĔĕĖėĘęĚě"),
        ("g", "ĜĝĞğĠġĢģ"),
        ("h", "ĤĥĦħ"),
        ("i", "ĨĩĪīĬĭĮįİı"),
        ("ij", "Ĳĳ"),
        ("j", "Ĵĵ"),
        ("k", "Ķķĸ"),
        ("l", "ĹĺĻļĽľĿŀŁł"),
        ("n", "ŃńŅņŇňŉŊŋ"),
        ("o", "ŌōŎŏŐő"),
        ("oe", "Œœ"),
        ("r", "ŔŕŖŗŘř"),
        ("s", "ŚśŜŝŞşŠšſ"),
        ("t", "ŢţŤťŦŧ"),
        ("u", "ŨũŪūŬŭŮůŰűŲų"),
        ("w", "Ŵŵ"),
        ("y", "ŶŷŸ"),
        ("z", "ŹźŻżŽž"),
    ];
    // The table holds the whole block, each letter once.
    let mut block: Vec<char> = LETTERS
        .iter()
        .flat_map(|(_, letters)| letters.chars())
        .collect();
    block.sort_unstable();
    assert_eq!(block, ('\u{100}'..='\u{17f}').collect::<Vec<char>>());
    for (plain, letters) in LETTERS {
        for c in letters.chars() {
            assert_eq!(
                natural_cmp(&format!("{c}!"), &format!("{plain}!")),
                Ordering::Equal,
                "{c} (U+{:04X})",
                c as u32
            );
        }
    }
}

/// A letter that is two letters, or another one, in the plain alphabet sorts as
/// those: ß as ss, æ as ae, œ as oe, ĳ as ij, þ as th, ð as d. Numbers still go
/// by their value.
#[test]
fn ligatures_and_nordic_letters_sort_as_the_letters_they_are_written_with() {
    for (written, plain) in [
        ("Straße 1", "Strasse 1"),
        ("STRAẞE 1", "strasse 1"),
        ("Ægir TV", "Aegir TV"),
        ("Þáttur", "Thattur"),
        ("Ðorgi", "Dorgi"),
        ("Øresund TV", "Oresund TV"),
        ("Œuvre", "Oeuvre"),
        ("Ĳssel TV", "IJssel TV"),
    ] {
        assert_eq!(natural_cmp(written, plain), Ordering::Equal, "{written}");
    }
    assert_eq!(
        natural_cmp("Straße 2", "Strasse 10"),
        Ordering::Less,
        "numbers by value"
    );
    // Sorted with their plain letter, so all of them before "Zender 1".
    for (written, plain) in [
        ("Şok TV", "Sok TV"),
        ("Łódź TV", "Lodz TV"),
        ("Šport", "Sport"),
        ("Straße 1", "Strasse 1"),
    ] {
        assert_eq!(natural_cmp(written, plain), Ordering::Equal, "{written}");
        assert_eq!(
            natural_cmp(written, "Zender 1"),
            Ordering::Less,
            "{written}"
        );
    }
}

/// Romanian writes s and t with a comma below (Ș ș Ț ț, U+0218 to U+021B, in
/// Latin Extended-B): they sort as s and t, so "Știrile TV" is among the S's
/// and "Țara" among the T's, not after the z.
#[test]
fn romanian_comma_below_letters_sort_as_s_and_t() {
    for (written, plain) in [
        ("Știrile TV", "Stirile TV"),
        ("ȘTIRILE TV", "stirile tv"),
        ("Țara", "Tara"),
        ("ȚARA", "tara"),
    ] {
        assert_eq!(natural_cmp(written, plain), Ordering::Equal, "{written}");
        assert_eq!(
            natural_cmp(written, "Zender 1"),
            Ordering::Less,
            "{written}"
        );
    }
    let unsorted = ["Zender 1", "Țara", "Tele 2", "Știrile TV", "Sport 1", "Uno"];
    let live = unsorted
        .iter()
        .zip(1..)
        .map(|(name, id)| ch(id, name, None))
        .collect();
    let g = build(live, &[], &[]);
    assert_eq!(
        names(&g, 0),
        ["Sport 1", "Știrile TV", "Țara", "Tele 2", "Uno", "Zender 1"]
    );
}

/// A name in another script is only lower-cased: its letters go by their code
/// points, after the Latin ones.
#[test]
fn other_scripts_are_only_lower_cased() {
    assert_eq!(natural_cmp("Первый Канал", "первый канал"), Ordering::Equal);
    assert_eq!(natural_cmp("ΑΛΦΑ", "αλφα"), Ordering::Equal);
    assert_eq!(natural_cmp("Аист", "Бобёр"), Ordering::Less);
    assert_eq!(natural_cmp("Zender 1", "Первый"), Ordering::Less);
}

/// A list with Turkish, Polish, Czech, Hungarian, German, Icelandic and Nordic
/// names, in the order its reader looks for a channel in.
#[test]
fn a_list_of_many_languages_is_in_the_order_its_reader_looks_for_a_channel() {
    let unsorted = [
        "Žižkov TV",
        "Zender 1",
        "Ülke TV",
        "Þáttur",
        "Straße 1",
        "Sport 1",
        "Šport",
        "Şok TV",
        "Őrség TV",
        "Øresund TV",
        "Łódź TV",
        "Işık TV",
        "Česká TV",
        "Canvas",
        "Ålesund TV",
        "Ægir TV",
    ];
    let live = unsorted
        .iter()
        .zip(1..)
        .map(|(name, id)| ch(id, name, None))
        .collect();
    let g = build(live, &[], &[]);
    assert_eq!(
        names(&g, 0),
        [
            "Ægir TV",
            "Ålesund TV",
            "Canvas",
            "Česká TV",
            "Işık TV",
            "Łódź TV",
            "Øresund TV",
            "Őrség TV",
            "Şok TV",
            "Šport",
            "Sport 1",
            "Straße 1",
            "Þáttur",
            "Ülke TV",
            "Zender 1",
            "Žižkov TV"
        ]
    );
}
