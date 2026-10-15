use bongo_player::args::read_ini_creds;
use bongo_player::i18n::Lang;
use bongo_player::sources::{is_web_address, Editing, Source, SourceKind, Sources};

const IPTV_ORG: &str = "https://iptv-org.github.io/iptv/index.m3u";

fn xtream(id: u32, name: &str, address: &str) -> Source {
    Source {
        id,
        kind: SourceKind::Xtream,
        name: name.into(),
        address: address.into(),
    }
}

fn m3u(id: u32, name: &str, address: &str) -> Source {
    Source {
        id,
        kind: SourceKind::M3u,
        name: name.into(),
        address: address.into(),
    }
}

/// A source as a tuple, to compare and to print: outside the crate's own tests
/// `Source` has no `Debug`, which would print its address.
type Row<'a> = (u32, SourceKind, &'a str, &'a str);

fn row(s: &Source) -> Row<'_> {
    (s.id, s.kind, s.name.as_str(), s.address.as_str())
}

fn rows(list: &[Source]) -> Vec<Row<'_>> {
    list.iter().map(row).collect()
}

/// What `Sources` holds: its sources and the one in use.
fn content(s: &Sources) -> (Vec<Row<'_>>, Option<u32>) {
    (rows(s.list()), s.active_id())
}

#[test]
fn a_0_6_1_ini_is_one_xtream_source_in_use() {
    let s = Sources::parse("portal=u:p@h.example.com:80\n");
    assert_eq!(
        rows(s.list()),
        rows(&[xtream(1, "", "u:p@h.example.com:80")])
    );
    assert_eq!(s.active_id(), Some(1));
}

#[test]
fn a_bare_credentials_line_a_comment_and_a_bom_still_read() {
    let s = Sources::parse("\u{feff}# my portal\r\n; old\r\nu:pa=ss@h.example.com:80\r\n");
    assert_eq!(
        rows(s.list()),
        rows(&[xtream(1, "", "u:pa=ss@h.example.com:80")])
    );
}

#[test]
fn one_default_portal_is_written_as_0_6_1_wrote_it() {
    let s = Sources::parse("portal=u:p@h.example.com:80\n");
    assert_eq!(s.to_text(), "portal=u:p@h.example.com:80\n");
}

#[test]
fn sources_and_the_one_in_use_go_round() {
    let mut s = Sources::default();
    assert_eq!(s.keep(Editing::New, SourceKind::Xtream, "", "u:p@h:80"), 1);
    assert_eq!(s.keep(Editing::New, SourceKind::M3u, "Gratis", IPTV_ORG), 2);
    assert!(s.set_active(2));
    let text = s.to_text();
    assert_eq!(
        text,
        format!(
            "portal=u:p@h:80\nsource=1;xtream;;u:p@h:80\nsource=2;m3u;Gratis;{IPTV_ORG}\nactive=2\n"
        )
    );
    assert_eq!(content(&Sources::parse(&text)), content(&s));
    assert_eq!(s.active().map(|a| a.name.as_str()), Some("Gratis"));
}

#[test]
fn with_source_lines_the_portal_line_is_only_a_copy() {
    let s = Sources::parse("portal=old:p@h:80\nsource=1;xtream;;new:p@h:80\n");
    assert_eq!(rows(s.list()), rows(&[xtream(1, "", "new:p@h:80")]));
}

#[test]
fn the_portal_line_is_the_first_xtream_source_so_0_6_1_starts_on_it() {
    let mut s = Sources::default();
    let list = s.keep(
        Editing::New,
        SourceKind::M3u,
        "Gratis",
        "https://bob:pw@lists.example/tv.m3u",
    );
    s.keep(
        Editing::New,
        SourceKind::Xtream,
        "Mijn portal",
        "u:p@portal.example:8080",
    );
    // The list is in use.
    assert!(s.set_active(list));
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    s.save(&ini).unwrap();
    // What 0.6.1 reads: the first line that holds credentials.
    assert_eq!(
        read_ini_creds(&ini).as_deref(),
        Some("u:p@portal.example:8080")
    );
    assert_eq!(content(&Sources::load(&ini)), content(&s));
}

#[test]
fn an_active_line_that_names_nothing_falls_back_to_the_first() {
    let s = Sources::parse(
        "source=3;m3u;A;http://a.example/1\nsource=5;m3u;B;http://b.example/2\nactive=9\n",
    );
    assert_eq!(s.active_id(), Some(3));
    let none = Sources::parse("");
    assert_eq!((none.list().len(), none.active_id()), (0, None));
}

#[test]
fn broken_source_lines_are_skipped() {
    let s = Sources::parse(
        "source=x;m3u;A;http://a.example/1\n\
         source=2;rss;A;http://a.example/1\n\
         source=2;m3u;A;\n\
         source=0;m3u;A;http://a.example/1\n\
         source=4;M3U;Goed;http://a.example/4\n\
         source=4;m3u;Dubbel;http://a.example/5\n",
    );
    assert_eq!(
        rows(s.list()),
        rows(&[m3u(4, "Goed", "http://a.example/4")])
    );
}

#[test]
fn keep_changes_a_saved_one_adds_a_new_one_and_numbers_from_the_highest() {
    let mut s = Sources::default();
    // A list never gets 1: source 1 keeps 0.6.1's files, which are the portal's.
    let first = s.keep(Editing::New, SourceKind::M3u, "A", "http://a.example/1");
    let second = s.keep(Editing::New, SourceKind::M3u, "B", "http://b.example/2");
    assert_eq!((first, second), (2, 3));
    // The first portal takes 1, which is free; the next one numbers from the
    // highest.
    assert_eq!(s.keep(Editing::New, SourceKind::Xtream, "C", "u:p@h:80"), 1);
    assert_eq!(s.keep(Editing::New, SourceKind::Xtream, "D", "v:q@h:80"), 4);
    assert_eq!(s.remove(2).map(|g| g.name), Some("A".into()));
    assert_eq!(
        s.keep(Editing::New, SourceKind::M3u, "E", "http://e.example/5"),
        5
    );
    // A saved one changes in place, its kind stays.
    assert_eq!(
        s.keep(
            Editing::Saved(3),
            SourceKind::M3u,
            "B2",
            "http://b.example/3"
        ),
        3
    );
    assert_eq!(
        s.get(3).map(row),
        Some(row(&m3u(3, "B2", "http://b.example/3")))
    );
    // The command line's portal is the first Xtream source...
    assert_eq!(
        s.keep(Editing::CommandLine, SourceKind::Xtream, "", "x:y@h:81"),
        1
    );
    assert_eq!(s.get(1).map(|c| c.address.as_str()), Some("x:y@h:81"));
    // ...and a new one when there is none: source 1, lists or not.
    for ini in ["", "source=2;m3u;A;http://a.example/1\n"] {
        let mut none = Sources::parse(ini);
        assert_eq!(
            none.keep(Editing::CommandLine, SourceKind::Xtream, "", "x:y@h:81"),
            1,
            "{ini:?}"
        );
    }
}

#[test]
fn a_list_saved_as_source_1_before_stays_source_1() {
    // Read as it is: no migration. Its number is taken, so the first portal
    // numbers from the highest, and so does the next list.
    let mut s = Sources::parse("source=1;m3u;Lijst;http://a.example/1\nactive=1\n");
    assert_eq!(
        rows(s.list()),
        rows(&[m3u(1, "Lijst", "http://a.example/1")])
    );
    assert_eq!(s.keep(Editing::New, SourceKind::Xtream, "", "u:p@h:80"), 2);
    assert_eq!(
        s.keep(Editing::New, SourceKind::M3u, "", "http://b.example/2"),
        3
    );
}

#[test]
fn names_lose_semicolons_and_line_breaks_addresses_their_spaces() {
    let mut s = Sources::default();
    let id = s.keep(
        Editing::New,
        SourceKind::M3u,
        " A;B\r\nC ",
        " http://a.example/1 \n",
    );
    assert_eq!(
        s.get(id).map(row),
        Some(row(&m3u(id, "A,BC", "http://a.example/1")))
    );
}

#[test]
fn a_password_with_semicolons_and_at_signs_goes_round() {
    let mut s = Sources::default();
    s.keep(Editing::New, SourceKind::M3u, "M", "http://a.example/1");
    let id = s.keep(
        Editing::New,
        SourceKind::Xtream,
        "X",
        "bob:p;a@ss=w:rd@srv.example.net:8080",
    );
    let back = Sources::parse(&s.to_text());
    assert_eq!(
        back.get(id).map(|x| x.address.as_str()),
        Some("bob:p;a@ss=w:rd@srv.example.net:8080")
    );
}

#[test]
fn a_save_replaces_the_ini_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::write(&ini, "portal=a:b@old.example.net:80\n").unwrap();
    let mut s = Sources::load(&ini);
    s.keep(Editing::New, SourceKind::M3u, "Gratis", IPTV_ORG);
    s.save(&ini).unwrap();
    assert_eq!(content(&Sources::load(&ini)), content(&s));
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(files, ["BongoPlayer.ini"]);
}

#[test]
fn a_failed_save_leaves_the_old_ini_and_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::write(&ini, "portal=a:b@old.example.net:80\n").unwrap();
    // A directory on the temp file's name: the new ini cannot be written there.
    std::fs::create_dir(ini.with_extension("ini.tmp")).unwrap();
    let mut s = Sources::load(&ini);
    s.keep(Editing::New, SourceKind::M3u, "Gratis", IPTV_ORG);
    assert!(s.save(&ini).is_err());
    assert_eq!(
        std::fs::read_to_string(&ini).unwrap(),
        "portal=a:b@old.example.net:80\n"
    );
    // A directory on the ini's name: the temp file is written but cannot replace it.
    let other = dir.path().join("other.ini");
    std::fs::create_dir(&other).unwrap();
    assert!(s.save(&other).is_err());
    assert!(!other.with_extension("ini.tmp").exists());
}

#[test]
fn removing_the_source_in_use_leaves_none_in_use() {
    let mut s = Sources::parse("portal=u:p@h:80\n");
    assert!(s.remove(1).is_some());
    assert_eq!((s.list().len(), s.active_id()), (0, None));
    assert!(s.remove(1).is_none());
    assert!(!s.set_active(7), "no such source");
}

#[test]
fn a_source_without_a_name_has_the_default_of_its_kind() {
    assert_eq!(xtream(1, "", "u:p@h:80").display_name(Lang::Nl), "Portaal");
    assert_eq!(m3u(2, "", IPTV_ORG).display_name(Lang::En), "M3U list");
    assert_eq!(m3u(2, "Gratis", IPTV_ORG).display_name(Lang::De), "Gratis");
    assert_eq!(SourceKind::from_code(" M3U "), Some(SourceKind::M3u));
    assert_eq!(SourceKind::Xtream.code(), "xtream");
}

#[test]
fn a_kinds_default_name_in_any_language_is_no_name_of_its_own() {
    for kind in [SourceKind::Xtream, SourceKind::M3u] {
        for lang in Lang::ALL {
            let name = kind.default_name(lang);
            assert!(kind.is_default_name(name), "{kind:?} in {lang:?}");
            assert!(kind.is_default_name(&format!("  {name} \r\n")), "{name}");
        }
        assert!(!kind.is_default_name("Thuis"));
        assert!(!kind.is_default_name(""));
    }
    // The default of the other kind is a name like any other.
    let portal = SourceKind::Xtream.default_name(Lang::En);
    assert!(!SourceKind::M3u.is_default_name(portal));
}

#[test]
fn a_web_address_is_http_or_https_with_a_host() {
    assert!(is_web_address(IPTV_ORG));
    assert!(is_web_address(" http://127.0.0.1:18080/m3u/list.m3u "));
    for no in [
        "C:\\lists\\tv.m3u",
        "ftp://a.example/x.m3u",
        "https://",
        "lijst.m3u",
        "",
    ] {
        assert!(!is_web_address(no), "{no}");
    }
}
