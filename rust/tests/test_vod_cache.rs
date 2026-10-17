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

/// Kills w11 (the writer writes a series' category as a film's) and r6 (the
/// reader reads every category as a film's): nothing committed has a category
/// of a series, so the `film` flag of a category is read back by no test.
#[test]
fn a_series_category_and_a_film_category_of_one_id_stay_two_categories_in_the_file() {
    // The file's [film, id, name]: films' categories and series' ones are two
    // id spaces, and id 5 names one of each.
    let movies = [Movie {
        stream_id: 1,
        name: "Funny (2001)".into(),
        category_id: 5,
        ..Movie::default()
    }];
    let shows = [Series {
        series_id: 9,
        name: "Dark".into(),
        category_id: 5,
        ..Series::default()
    }];
    let categories = Categories {
        vod: vec![Category {
            category_id: 5,
            category_name: "NL | Komedie".into(),
        }],
        series: vec![Category {
            category_id: 5,
            category_name: "Drama".into(),
        }],
    };
    let index = TitleIndex::build(&movies, &shows, &categories, 2027);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    save(&path, &index, SAVED_AT).unwrap();
    let (back, _) = load(&path).unwrap().unwrap();
    let category_of = |id: TitleId| {
        let t = back.title(back.find(id).unwrap()).unwrap();
        t.category.map(|c| (c.kind, c.id, c.name))
    };
    assert_eq!(category_of(film(1)), Some((Kind::Film, 5, "NL | Komedie")));
    assert_eq!(category_of(series(9)), Some((Kind::Series, 5, "Drama")));
    let listed: Vec<_> = back.categories().map(|c| (c.kind, c.id)).collect();
    assert_eq!(listed, [(Kind::Film, 5), (Kind::Series, 5)]);
}

/// Kills r5 (the reader drops the trailer flag) and, with the next test, w8 (the
/// writer does) and any swap of the three flags: no committed test has a trailer.
#[test]
fn the_flags_of_a_title_are_read_in_the_order_they_are_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,1,"A","",0,0,0,0,0,true,false,false,[[1,0,""]]],
            [true,2,"B","",0,0,0,0,0,false,true,false,[[2,0,""]]],
            [true,3,"C","",0,0,0,0,0,false,false,true,[[3,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let flags = |id: i64| {
        let t = index.title(index.find(film(id)).unwrap()).unwrap();
        (t.dutch, t.english, t.trailer)
    };
    assert_eq!(flags(1), (true, false, false));
    assert_eq!(flags(2), (false, true, false));
    assert_eq!(flags(3), (false, false, true));
}

/// Kills w8 (the writer writes `false` for the trailer).
#[test]
fn what_the_details_taught_a_title_is_written_with_it() {
    use bongo_player::vod::index::Learned;
    let index = library();
    let secret = index.find(film(4)).unwrap();
    index.learn(
        secret,
        Learned {
            year: 0,
            genres: Genre::Horror.bit(),
            trailer: true,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    save(&path, &index, SAVED_AT).unwrap();
    let (back, _) = load(&path).unwrap().unwrap();
    let title = |id: i64| back.title(back.find(film(id)).unwrap()).unwrap();
    assert_eq!(
        (title(4).trailer, title(4).genres),
        (true, Genre::Horror.bit())
    );
    assert!(!title(1).trailer, "only the title that learned it");
}

/// Kills r1 (a rating past 100 is kept as it is): `Title::rating` is "in tenths,
/// 0 to 100".
#[test]
fn a_rating_past_100_reads_as_100() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,1,"A","",0,99,0,0,0,false,false,false,[[1,0,""]]],
            [true,2,"B","",0,100,0,0,0,false,false,false,[[2,0,""]]],
            [true,3,"C","",0,101,0,0,0,false,false,false,[[3,0,""]]],
            [true,4,"D","",0,255,0,0,0,false,false,false,[[4,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let rating = |id: i64| index.title(index.find(film(id)).unwrap()).unwrap().rating;
    assert_eq!(
        (rating(1), rating(2), rating(3), rating(4)),
        (99, 100, 100, 100)
    );
}

/// Kills r2 (the place one past the list is kept). The committed test names place
/// 7 in a list of one, which a bound one too high lets pass.
#[test]
fn a_category_place_one_past_the_list_names_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    // Two categories: places 1 and 2 are theirs, 3 is one past them.
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[[true,12,"A"],[true,13,"B"]],"titles":[
            [true,1,"One","",0,0,0,2,0,false,false,false,[[1,0,""]]],
            [true,2,"Two","",0,0,0,3,0,false,false,false,[[2,0,""]]],
            [true,3,"Three","",0,0,0,3,0,false,false,false,[[3,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let category = |id: i64| {
        let t = index.title(index.find(film(id)).unwrap()).unwrap();
        t.category.map(|c| c.id)
    };
    assert_eq!(
        (category(1), category(2), category(3)),
        (Some(13), None, None)
    );
    let none = HashSet::new();
    let two = index.find(film(2)).unwrap();
    assert!(
        index.more_like(two, 6, false, &none, &none).is_empty(),
        "no category to share"
    );
}

/// Kills p1 (a dash becomes `_`) and p2 (a letter that is no ASCII letter stays):
/// the rule is the EPG cache's, and the committed test has neither.
#[test]
fn portal_key_keeps_dots_and_dashes_and_turns_the_rest_into_underscores() {
    assert_eq!(portal_key("my-host.example", 8080), "my-host.example_8080");
    assert_eq!(portal_key("b\u{fc}cher.example", 443), "b_cher.example_443");
    assert_eq!(portal_key("a b@c", 1), "a_b_c_1");
}

/// Kills i7 (the film check after the dedup): the report's "a later title of the
/// same kind and id is no longer taken for a copy of a film that was left out" is
/// pinned by no committed test.
#[test]
fn a_dead_film_does_not_shadow_a_live_one_of_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [true,7,"Dead","",0,0,0,0,0,false,false,false,[[0,0,""]]],
            [true,7,"Alive","",0,0,0,0,0,false,false,false,[[7,0,"mkv"]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    assert_eq!(index.len(), 1);
    let alive = index.title(index.find(film(7)).unwrap()).unwrap();
    assert_eq!(alive.name, "Alive");
    assert_eq!(index.stats().skipped, 1, "the dead one, once");
}

/// Kills r13 (an unreadable file answers "no cache"): a folder where the file
/// should be is an error the caller logs, not a missing file.
#[test]
fn a_file_that_cannot_be_read_is_an_error_and_not_no_cache() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load(dir.path()).is_err());
}

/// Kills r12 (only a higher version is refused).
#[test]
fn a_file_of_a_version_before_this_one_is_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":0,"saved_at":0,"categories":[],"titles":[]}"#,
    )
    .unwrap();
    let Err(e) = load(&path) else {
        panic!("version 0 was read");
    };
    assert!(e.contains('0'), "{e}");
}

/// One assertion for every field the reader or the writer drops or changes: what
/// a file holds, written again, is the same file.
#[test]
fn a_file_read_and_written_again_is_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let (first, again) = (dir.path().join("first.json"), dir.path().join("again.json"));
    save(&first, &library(), SAVED_AT).unwrap();
    let (back, saved_at) = load(&first).unwrap().unwrap();
    save(&again, &back, saved_at).unwrap();
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        std::fs::read_to_string(&again).unwrap()
    );
}

/// The cover filter is `redact`'s: every form of an account it knows stays out,
/// whatever the file's reader would do with it. (Forms it has never known, such
/// as `//user:pass@host/x.jpg` or a `#` fragment, are no address a portal's
/// cover takes; see the review.)
#[test]
fn a_cover_that_holds_an_account_is_left_out_in_every_form_redact_knows() {
    let stays = [
        "https://image.tmdb.org/t/p/w600_and_h900_bestv2/abcdef0123456789.jpg",
        "http://203.0.113.5:8080/images/m.jpg",
        "https://cdn.example/posters/12/34/56.jpg",
    ];
    let goes = [
        "http://u:p@h/x.jpg",
        "http://user@h/x.jpg",
        "http://h:80/movie/u/p/4.jpg",
        "http://h:80/series/u/p/4.jpg",
        "http://h:80/live/u/p/4.jpg",
        "http://h:80/timeshift/u/p/4.jpg",
        "http://h/u/p/4.jpg",
        "http://h/u/p/4",
        "http://h/x.jpg?username=u&password=p",
        "http://h/x.jpg?token=abc",
        "http://h/x.jpg?",
    ];
    let movies: Vec<Movie> = stays
        .iter()
        .chain(goes.iter())
        .enumerate()
        .map(|(i, cover)| Movie {
            stream_id: i as i64 + 1,
            name: format!("Film {i} (2001)"),
            stream_icon: Some((*cover).to_string()),
            ..Movie::default()
        })
        .collect();
    let index = TitleIndex::build(&movies, &[], &Categories::default(), 2027);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    save(&path, &index, SAVED_AT).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for cover in goes {
        assert!(!text.contains(cover), "{cover} was written");
    }
    let (back, _) = load(&path).unwrap().unwrap();
    for (i, cover) in stays.iter().chain(goes.iter()).enumerate() {
        let t = back.title(back.find(film(i as i64 + 1)).unwrap()).unwrap();
        assert_eq!(t.cover, (i < stays.len()).then_some(*cover), "{cover}");
    }
}

/// A file cut anywhere is refused, and one with a damaged byte is refused or read:
/// the release build aborts on a panic, and a half file is what a crash leaves.
/// Every fifth cut and every eleventh byte, two ways each (a file costs 4 ms in a
/// debug build; the review ran every cut and every bit of this file, and 290,000
/// more, and met no panic).
#[test]
fn a_damaged_file_is_refused_or_read_and_never_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("valid.json");
    save(&valid, &library(), SAVED_AT).unwrap();
    let bytes = std::fs::read(&valid).unwrap();
    let path = dir.path().join("damaged.json");
    for cut in (0..bytes.len()).step_by(5) {
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(load(&path).is_err(), "a file cut at byte {cut} was read");
    }
    for at in (0..bytes.len()).step_by(11) {
        for flip in [0x01u8, 0x20] {
            let mut damaged = bytes.clone();
            damaged[at] ^= flip;
            std::fs::write(&path, &damaged).unwrap();
            let _ = load(&path);
        }
    }
}

/// RED on e74cd62 (the implementer's concern 1): a series that carries versions
/// in a file keeps them, `title_of_version` answers its position, and the
/// versions count in `Stats`. GREEN with `assemble` giving a series none.
#[test]
fn a_series_that_carries_versions_in_a_file_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[],"titles":[
            [false,9,"A series","",0,0,0,0,0,false,false,false,[[77,0,"mkv"],[78,0,""]]],
            [true,5,"A film","",0,0,0,0,0,false,false,false,[[5,0,"mp4"],[79,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let dark = index.title(index.find(series(9)).unwrap()).unwrap();
    assert!(dark.versions.is_empty(), "a series has no versions");
    assert_eq!(
        index.title_of_version(77),
        None,
        "no film plays as stream 77"
    );
    assert_eq!(index.title_of_version(79), index.find(film(5)));
    let s = index.stats();
    assert_eq!((s.series, s.films, s.versions, s.skipped), (1, 1, 2, 0));
}

/// RED on e74cd62: two stores of one portal write one file, and their saves share
/// its temporary file. Measured on e74cd62 (release): 13 to 24 of 100 saves fail
/// with "The system cannot find the file specified", readers meet files that are
/// halfway, and the file at the end can be a valid mix of both. GREEN with one
/// save at a time in the process.
#[test]
fn two_saves_of_one_file_at_once_leave_it_whole() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let films = |n: i64, tag: &str| -> Arc<TitleIndex> {
        let movies: Vec<Movie> = (1..=n)
            .map(|i| Movie {
                stream_id: i,
                name: format!("{tag} film {i} (2001)"),
                container_extension: Some("mkv".into()),
                stream_icon: Some(format!("https://img.example/{tag}/{i}.jpg")),
                ..Movie::default()
            })
            .collect();
        Arc::new(TitleIndex::build(
            &movies,
            &[],
            &Categories::default(),
            2027,
        ))
    };
    let (small, big) = (films(400, "small"), films(1200, "big"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache").join("vod-h_80.json");
    save(&path, &small, 1).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let path = path.clone();
            let index = if w % 2 == 0 {
                small.clone()
            } else {
                big.clone()
            };
            std::thread::spawn(move || {
                (0..20)
                    .filter_map(|_| save(&path, &index, 2).err())
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let reader = {
        let (path, done) = (path.clone(), done.clone());
        std::thread::spawn(move || {
            let mut halfway = Vec::new();
            while !done.load(Ordering::Relaxed) {
                if let Err(e) = load(&path) {
                    halfway.push(e);
                }
            }
            halfway
        })
    };
    let failed: Vec<String> = writers
        .into_iter()
        .flat_map(|w| w.join().unwrap())
        .collect();
    done.store(true, Ordering::Relaxed);
    let halfway = reader.join().unwrap();
    assert!(
        failed.is_empty(),
        "{} saves failed: {failed:?}",
        failed.len()
    );
    assert!(halfway.is_empty(), "reads of a file halfway: {halfway:?}");
    let (back, _) = load(&path).unwrap().unwrap();
    assert!(
        back.len() == small.len() || back.len() == big.len(),
        "a mix of both: {} titles",
        back.len()
    );
}
