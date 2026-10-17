//! The title index in a file (vod/cache.rs, spec 2026-10-15 §4.6): what goes
//! in and what never does, a file that is missing or broken, and what a file
//! says that nothing may trust.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::cache::{load, path_for, portal_key, save};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Query, TitleIndex};
use bongo_player::vod::{Kind, TitleId};
use std::collections::HashSet;
use std::path::Path;

const SAVED_AT: i64 = 1_792_022_400;

fn library() -> TitleIndex {
    let movies = [
        Movie {
            stream_id: 1,
            name: "NL | The Matrix (1999) [4K]".into(),
            container_extension: Some("mkv".into()),
            stream_icon: Some("https://img.example/m.jpg".into()),
            rating: 82,
            added: SAVED_AT,
            category_id: 12,
            ..Movie::default()
        },
        Movie {
            stream_id: 2,
            name: "The Matrix (1999) 1080p".into(),
            container_extension: Some("mp4".into()),
            ..Movie::default()
        },
        Movie {
            stream_id: 3,
            name: "Night Club (2020)".into(),
            is_adult: true,
            ..Movie::default()
        },
        Movie {
            stream_id: 4,
            name: "Secret (2021)".into(),
            stream_icon: Some("http://h:80/movie/u/p/4.jpg".into()),
            ..Movie::default()
        },
    ];
    let shows = [Series {
        series_id: 9,
        name: "Dark".into(),
        genre: Some("Drama".into()),
        release_date_camel: Some("2017-12-01".into()),
        ..Series::default()
    }];
    let categories = Categories {
        vod: vec![Category {
            category_id: 12,
            category_name: "NL | Komedie".into(),
        }],
        series: Vec::new(),
    };
    TitleIndex::build(&movies, &shows, &categories, 2027)
}

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

fn series(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

#[test]
fn what_is_written_is_read_back_without_adult_titles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache").join("vod-h_80.json");
    let index = library();
    let (n, bytes) = save(&path, &index, SAVED_AT).unwrap();
    assert_eq!(n, 3, "the adult title stays out");
    assert!(bytes > 0);
    assert!(!path.with_extension("json.tmp").exists());
    let (back, saved_at) = load(&path).unwrap().unwrap();
    assert_eq!(saved_at, SAVED_AT);
    assert_eq!(back.len(), 3);
    assert_eq!(back.find(film(3)), None);
    let matrix = back.title(back.find(film(1)).unwrap()).unwrap();
    let was = index.title(index.find(film(1)).unwrap()).unwrap();
    assert_eq!(
        (
            matrix.name,
            matrix.cover,
            matrix.year,
            matrix.rating,
            matrix.genres
        ),
        (was.name, was.cover, was.year, was.rating, was.genres)
    );
    assert_eq!(matrix.genres, Genre::Comedy.bit());
    assert_eq!(
        (matrix.added_day, matrix.dutch, matrix.english),
        (was.added_day, true, false)
    );
    assert_eq!(
        matrix.category.map(|c| (c.kind, c.id, c.name)),
        Some((Kind::Film, 12, "NL | Komedie"))
    );
    let versions: Vec<(i64, Labels, &str)> = matrix
        .versions
        .iter()
        .map(|v| (v.stream_id, v.labels, v.ext))
        .collect();
    assert_eq!(
        versions,
        [
            (1, Labels::UHD.with(Labels::NL), "mkv"),
            (2, Labels::FHD, "mp4")
        ]
    );
    let dark = back
        .title(
            back.find(TitleId {
                kind: Kind::Series,
                id: 9,
            })
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        (dark.name, dark.year, dark.genres),
        ("Dark", 2017, Genre::Drama.bit())
    );
    // The search works on what was read: its search names are built again.
    let found = back.search(
        &Query {
            text: "matrix".into(),
            ..Query::default()
        },
        false,
    );
    assert_eq!(found.titles.len(), 1);
}

#[test]
fn a_cover_with_an_account_in_it_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    save(&path, &library(), SAVED_AT).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("/movie/u/p/"), "{text}");
    let (back, _) = load(&path).unwrap().unwrap();
    let secret = back.title(back.find(film(4)).unwrap()).unwrap();
    assert_eq!(secret.cover, None, "its tile shows the placeholder");
}

#[test]
fn a_cover_with_a_token_in_its_query_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    let movies = [Movie {
        stream_id: 1,
        name: "Tokened (2020)".into(),
        stream_icon: Some("https://cdn.example/p.jpg?token=secret".into()),
        ..Movie::default()
    }];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), 2027);
    save(&path, &index, SAVED_AT).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("token="),
        "redact sees no token there: {text}"
    );
    let (back, _) = load(&path).unwrap().unwrap();
    assert_eq!(
        back.title(0).unwrap().cover,
        None,
        "its tile shows the placeholder"
    );
}

#[test]
fn a_missing_file_is_no_cache_and_a_broken_one_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load(&dir.path().join("none.json")).unwrap().is_none());
    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, b"{\"version\":1,\"saved_at\":").unwrap();
    assert!(load(&broken).is_err());
    let other = dir.path().join("other.json");
    std::fs::write(
        &other,
        br#"{"version":99,"saved_at":0,"categories":[],"titles":[]}"#,
    )
    .unwrap();
    let Err(e) = load(&other) else {
        panic!("another version was read");
    };
    assert!(e.contains("99"), "{e}");
}

#[test]
fn a_write_that_fails_leaves_no_temporary_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::create_dir(&path).unwrap();
    assert!(save(&path, &library(), SAVED_AT).is_err());
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn the_file_is_named_after_the_portal_and_nothing_else() {
    assert_eq!(portal_key("192.168.1.5", 8080), "192.168.1.5_8080");
    let odd = portal_key("evil/../host:x\\y", 80);
    assert!(
        !odd.contains('/') && !odd.contains('\\') && !odd.contains(':'),
        "{odd}"
    );
    let creds = XtreamCreds {
        username: "user".into(),
        password: "secret".into(),
        host: "pandora.example".into(),
        port: 8080,
    };
    let path = path_for(Path::new("C:/cfg/cache"), Some(&creds)).unwrap();
    assert_eq!(
        path,
        Path::new("C:/cfg/cache").join("vod-pandora.example_8080.json")
    );
    assert!(
        path_for(Path::new("C:/cfg/cache"), None).is_none(),
        "an M3U list keeps none"
    );
}

#[test]
fn a_file_with_stray_genre_bits_counts_the_24_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(&path, br#"{"version":1,"saved_at":0,"categories":[],"titles":[[true,1,"X","",0,0,4294967295,0,0,false,false,false,[[1,0,""]]]]}"#).unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let counts = index.search(&Query::default(), false).counts;
    assert_eq!(counts.genres.iter().sum::<usize>(), 24);
}

#[test]
fn a_category_the_file_does_not_list_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    // One category: title 1 is in it, titles 2 and 3 name a place past it.
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[[true,12,"NL | Komedie"]],"titles":[
            [true,1,"A","",0,0,0,1,0,false,false,false,[[1,0,""]]],
            [true,2,"B","",0,0,0,7,0,false,false,false,[[2,0,""]]],
            [true,3,"C","",0,0,0,7,0,false,false,false,[[3,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let category = |id: i64| {
        let t = index.title(index.find(film(id)).unwrap()).unwrap();
        t.category.map(|c| c.id)
    };
    assert_eq!(
        (category(1), category(2), category(3)),
        (Some(12), None, None)
    );
    let none = HashSet::new();
    let b = index.find(film(2)).unwrap();
    assert!(
        index.more_like(b, 6, false, &none, &none).is_empty(),
        "no category to share"
    );
}

#[test]
fn odd_labels_and_more_containers_than_codes_are_read_safely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    // Labels with every bit set, and more containers than the 255 codes.
    let versions: Vec<String> = (0..300)
        .map(|i| format!(r#"[{},65535,"c{i}"]"#, 100 + i))
        .collect();
    let body = format!(
        r#"{{"version":1,"saved_at":0,"categories":[],"titles":[[true,100,"X","",0,0,0,0,0,false,false,false,[{}]]]}}"#,
        versions.join(",")
    );
    std::fs::write(&path, body).unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let x = index.title(0).unwrap();
    assert_eq!(x.versions.len(), 300);
    let first = x.versions.get(0).unwrap();
    assert_eq!((first.ext, first.labels.rank()), ("c0", 4));
    assert!(!first.labels.text().is_empty());
    assert_eq!(x.versions.get(254).unwrap().ext, "c254", "the last code");
    assert!(
        x.versions.iter().skip(255).all(|v| v.ext.is_empty()),
        "past the codes: no container"
    );
}

#[test]
fn a_film_left_with_no_version_to_play_is_left_out_and_counted() {
    // A film with no version cannot play: one whose versions the fence drops
    // all (a stream id of 0), or that has none, goes out whole, once, and is
    // counted. A series has no versions and stays.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,1,"Plays","",0,0,0,0,0,false,false,false,[[1,0,"mkv"]]],
            [true,2,"Only a zero","",0,0,0,0,0,false,false,false,[[0,0,""]]],
            [true,3,"All zeros","",0,0,0,0,0,false,false,false,[[0,0,""],[0,65535,"x"]]],
            [true,4,"No version","",0,0,0,0,0,false,false,false,[]],
            [true,5,"One of two","",0,0,0,0,0,false,false,false,[[0,0,""],[5,0,"mp4"]]],
            [false,9,"Dark","",0,0,0,0,0,false,false,false,[]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    for id in [2, 3, 4] {
        assert_eq!(index.find(film(id)), None, "film {id}");
    }
    let name_of = |id: TitleId| index.title(index.find(id).unwrap()).unwrap().name;
    assert_eq!(
        (name_of(film(1)), name_of(film(5)), name_of(series(9))),
        ("Plays", "One of two", "Dark")
    );
    assert_eq!(index.len(), 3);
    // Three films went out, each once for the whole title; the zero version of
    // "One of two" is one more.
    let s = index.stats();
    assert_eq!((s.films, s.series, s.versions, s.skipped), (2, 1, 2, 4));
    let five = index.title(index.find(film(5)).unwrap()).unwrap();
    let streams: Vec<i64> = five.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(streams, [5]);
    assert_eq!(index.title_of_version(5), index.find(film(5)));
    assert_eq!(index.title_of_version(0), None);
}

#[test]
fn a_film_left_out_leaves_the_index_as_if_the_file_never_had_it() {
    // The records, the text tables, their capacities and the A-Z order are all
    // made from what is kept: the index is the one a file without the left-out
    // film makes, down to its size. The film left out comes first in A-Z and
    // has a name and a cover of its own, so a count or an order taken before
    // it went out would show.
    let dir = tempfile::tempdir().unwrap();
    let read = |name: &str, left_out: &str| {
        let body = format!(
            r#"{{"version":1,"saved_at":0,"categories":[],"titles":[{left_out}
            [true,1,"Zebra","https://img.example/z.jpg",1999,0,0,0,0,false,false,false,[[1,0,"mkv"]]],
            [false,9,"Dark","",2017,0,0,0,0,false,false,false,[]]]}}"#
        );
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        load(&path).unwrap().unwrap().0
    };
    let with = read(
        "with.json",
        r#"[true,2,"Aardvark","https://img.example/a.jpg",0,0,0,0,0,false,false,false,[[0,0,""]]],"#,
    );
    let without = read("without.json", "");
    assert_eq!((with.stats().skipped, without.stats().skipped), (1, 0));
    assert_eq!(
        (with.len(), with.heap_bytes(), with.cover_bytes()),
        (without.len(), without.heap_bytes(), without.cover_bytes())
    );
    let order = |index: &TitleIndex| -> Vec<String> {
        let found = index.search(&Query::default(), false);
        let name_at = |pos: &u32| index.title(*pos as usize).unwrap().name.to_string();
        found.titles.iter().map(name_at).collect()
    };
    assert_eq!(order(&with), ["Dark", "Zebra"]);
    assert_eq!(order(&without), ["Dark", "Zebra"]);
}

#[test]
fn a_genre_bit_no_genre_has_is_dropped_where_the_file_is_read() {
    // The queries mask a stray bit as well, as a last line. What the index
    // keeps is the 24 genres and no more, so a filter on a bit past them finds
    // nothing (Ruling 51).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,1,"Every bit","",0,0,4294967295,0,0,false,false,false,[[1,0,""]]],
            [true,2,"A stray one","",0,0,1073741824,0,0,false,false,false,[[2,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let genres_of = |id: i64| index.title(index.find(film(id)).unwrap()).unwrap().genres;
    let all = Genre::ALL.iter().fold(0, |mask, g| mask | g.bit());
    assert_eq!(genres_of(1), all, "the 24 and no more");
    assert_eq!(genres_of(2), 0, "a bit past them names nothing");
    let stray = Query {
        genres: 1 << 30,
        ..Query::default()
    };
    assert!(index.search(&stray, false).titles.is_empty());
}

#[test]
fn a_title_or_version_with_id_0_in_a_file_is_left_out_and_counted() {
    // `assemble` is the fence for ids: the reader reads them as the file's own
    // u32 and hands them over as they are.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,0,"Zero film","",0,0,0,0,0,false,false,false,[[1,0,""]]],
            [false,0,"Zero series","",0,0,0,0,0,false,false,false,[]],
            [true,5,"Five","",0,0,0,0,0,false,false,false,[[0,0,""],[5,0,"mkv"]]],
            [true,5,"Five again","",0,0,0,0,0,false,false,false,[[6,0,""]]],
            [false,5,"Series five","",0,0,0,0,0,false,false,false,[]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    assert_eq!(index.len(), 2, "film 5 and series 5");
    assert_eq!(index.find(film(0)), None);
    assert_eq!(index.find(series(0)), None);
    let five = index.title(index.find(film(5)).unwrap()).unwrap();
    assert_eq!(five.name, "Five", "of two titles of one id the first stays");
    let streams: Vec<i64> = five.versions.iter().map(|v| v.stream_id).collect();
    assert_eq!(streams, [5], "its version of stream id 0 is left out");
    assert_eq!(index.title_of_version(6), None);
    // The two titles of id 0, the second film 5, and the version of stream id 0.
    assert_eq!(index.stats().skipped, 4);
}

#[test]
fn an_id_that_is_no_u32_makes_the_file_unreadable_and_never_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    for (id, stream_id) in [
        ("4294967296", "1"),
        ("-1", "1"),
        ("1", "4294967296"),
        ("1", "-1"),
    ] {
        let body = format!(
            r#"{{"version":1,"saved_at":0,"categories":[],"titles":[[true,{id},"X","",0,0,0,0,0,false,false,false,[[{stream_id},0,""]]]]}}"#
        );
        std::fs::write(&path, body).unwrap();
        assert!(load(&path).is_err(), "title {id}, stream {stream_id}");
    }
}

#[test]
fn what_the_build_writes_is_read_back_whole_odd_ids_in_the_lists_included() {
    // The build leaves out what does not fit (an id of 0, a negative one, one
    // past u32::MAX) before the index holds it, and gives no film without a
    // version that plays: so the file holds nothing the reader's fence drops,
    // and nothing is left out or counted on the way back.
    let film_of = |stream_id: i64, name: &str| Movie {
        stream_id,
        name: name.into(),
        container_extension: Some("mkv".into()),
        ..Movie::default()
    };
    let movies = [
        film_of(0, "Edge (2001) 4K"),
        film_of(7, "Edge (2001) 1080p"),
        film_of(-3, "Negative (2002)"),
        film_of(5_000_000_000, "Wide (2003)"),
        film_of(4_294_967_295, "Last (2004)"),
        Movie {
            is_adult: true,
            ..film_of(3, "Night (2005)")
        },
    ];
    let shows = [
        Series {
            series_id: 0,
            name: "Zero".into(),
            ..Series::default()
        },
        Series {
            series_id: 4,
            name: "Four".into(),
            ..Series::default()
        },
    ];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), 2027);
    assert_eq!(index.len(), 4, "Edge, Last, Night and Four");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    let (written, _) = save(&path, &index, SAVED_AT).unwrap();
    assert_eq!(written, 3, "all but Night");
    let (back, _) = load(&path).unwrap().unwrap();
    assert_eq!(back.len(), 3);
    assert_eq!(back.stats().skipped, 0, "nothing for the fence to drop");
    let streams_of = |i: &TitleIndex, pos: usize| -> Vec<i64> {
        let t = i.title(pos).unwrap();
        t.versions.iter().map(|v| v.stream_id).collect()
    };
    for pos in 0..back.len() {
        let t = back.title(pos).unwrap();
        let was = index.find(t.id).unwrap();
        assert_eq!(
            streams_of(&back, pos),
            streams_of(&index, was),
            "{}",
            t.name
        );
        assert_eq!(t.name, index.title(was).unwrap().name);
        assert_eq!(
            t.id.kind == Kind::Film,
            !t.versions.is_empty(),
            "a film has a version to play, a series none: {}",
            t.name
        );
    }
    assert_eq!(
        back.title_of_version(4_294_967_295),
        back.find(film(4_294_967_295))
    );
    assert_eq!(back.title_of_version(0), None);
}
