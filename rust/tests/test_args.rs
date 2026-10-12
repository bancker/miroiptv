use bongo_player::args::{
    credentials_line, parse_xtream_creds, read_ini_creds, resolve_creds, without_newlines,
    CredsOrigin, CredsSource, XtreamCreds,
};

#[test]
fn parses_xtream_creds_with_port() {
    let c = parse_xtream_creds("user:pass@host.example.com:8080").unwrap();
    assert_eq!(
        c,
        XtreamCreds {
            username: "user".into(),
            password: "pass".into(),
            host: "host.example.com".into(),
            port: 8080,
        }
    );
}

#[test]
fn parses_xtream_creds_default_port() {
    let c = parse_xtream_creds("u:p@h.example.com").unwrap();
    assert_eq!(c.port, 80);
}

#[test]
fn rejects_invalid_creds() {
    assert!(parse_xtream_creds("nope").is_err());
    assert!(parse_xtream_creds("no@at").is_err());
}

#[test]
fn accepts_password_with_colon() {
    // Some portals use colons in passwords. We split on the FIRST colon.
    let c = parse_xtream_creds("user:pa:ss@h.example.com:80").unwrap();
    assert_eq!(c.username, "user");
    assert_eq!(c.password, "pa:ss");
}

#[test]
fn accepts_password_with_at_sign() {
    // The LAST '@' separates the server: a password may contain '@'.
    let c = parse_xtream_creds("user:p@ss@h.example.com:8080").unwrap();
    assert_eq!(c.password, "p@ss");
    assert_eq!(c.host, "h.example.com");
    assert_eq!(c.port, 8080);
}

#[test]
fn ini_bare_line_with_equals_in_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::write(&ini, "u:pa=ss@h.example.com:80\n").unwrap();
    assert_eq!(read_ini_creds(&ini).as_deref(), Some("u:pa=ss@h.example.com:80"));
}

#[test]
fn ini_key_value_line_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::write(&ini, "# portal\nportal = u:p@h.example.com:80\n").unwrap();
    assert_eq!(read_ini_creds(&ini).as_deref(), Some("u:p@h.example.com:80"));
}

#[test]
fn ini_saved_with_a_bom_still_reads() {
    // Notepad's "UTF-8 with BOM": the BOM is no part of the user name.
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::write(&ini, "\u{feff}u:p@h.example.com:80\r\n").unwrap();
    assert_eq!(read_ini_creds(&ini).as_deref(), Some("u:p@h.example.com:80"));
}

#[test]
fn placeholder_cli_credentials_fall_back_to_the_ini() {
    // The zip's run.bat passes placeholder values; saved credentials win.
    match resolve_creds(Some("user:pass@host.example.com:8080"), Some("real:pw@srv.example.net:80")) {
        CredsSource::Configured(c, origin) => {
            assert_eq!(c.username, "real");
            assert_eq!(origin, CredsOrigin::Ini);
        }
        other => panic!("expected the ini's credentials, got {other:?}"),
    }
}

#[test]
fn real_cli_credentials_still_win_over_the_ini() {
    match resolve_creds(Some("cli:pw@a.example.net:80"), Some("ini:pw@b.example.net:80")) {
        CredsSource::Configured(c, origin) => {
            assert_eq!(c.username, "cli");
            assert_eq!(origin, CredsOrigin::CommandLine);
        }
        other => panic!("expected the command-line credentials, got {other:?}"),
    }
}

#[test]
fn missing_placeholder_and_unreadable_credentials_are_told_apart() {
    assert_eq!(resolve_creds(None, None), CredsSource::Missing);
    assert_eq!(resolve_creds(Some("user:pass@host.example.com"), None), CredsSource::Placeholder);
    assert!(matches!(resolve_creds(None, Some("nonsense@")), CredsSource::Invalid(_)));
}

#[test]
fn credentials_written_by_the_app_read_back_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    bongo_player::args::write_ini_creds(&ini, "bob:p@ss=w:rd@srv.example.net:8080").unwrap();
    let back = parse_xtream_creds(&read_ini_creds(&ini).unwrap()).unwrap();
    assert_eq!(
        back,
        XtreamCreds {
            username: "bob".into(),
            password: "p@ss=w:rd".into(),
            host: "srv.example.net".into(),
            port: 8080,
        }
    );
}

#[test]
fn a_user_name_with_an_equals_sign_reads_back_the_same() {
    // Written as a bare line, "b=ob:..." would read as the key "b" with the value "ob:...".
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    bongo_player::args::write_ini_creds(&ini, "b=ob:p@ss=w:rd@srv.example.net:8080").unwrap();
    let back = parse_xtream_creds(&read_ini_creds(&ini).unwrap()).unwrap();
    assert_eq!(
        back,
        XtreamCreds {
            username: "b=ob".into(),
            password: "p@ss=w:rd".into(),
            host: "srv.example.net".into(),
            port: 8080,
        }
    );
}

#[test]
fn a_second_write_replaces_the_first_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    bongo_player::args::write_ini_creds(&ini, "a:b@old.example.net:80").unwrap();
    bongo_player::args::write_ini_creds(&ini, "c:d@new.example.net:81").unwrap();
    assert_eq!(read_ini_creds(&ini).as_deref(), Some("c:d@new.example.net:81"));
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, ["BongoPlayer.ini"]);
}

#[test]
fn a_write_that_fails_leaves_the_old_ini_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("BongoPlayer.ini");
    bongo_player::args::write_ini_creds(&ini, "a:b@old.example.net:80").unwrap();
    // A directory on the temp file's name: the new credentials cannot be written
    // there. The ini is only replaced by the last step, so it must still be the old one.
    std::fs::create_dir(ini.with_extension("ini.tmp")).unwrap();
    assert!(bongo_player::args::write_ini_creds(&ini, "c:d@new.example.net:81").is_err());
    assert_eq!(read_ini_creds(&ini).as_deref(), Some("a:b@old.example.net:80"));
}

#[test]
fn a_failed_replace_leaves_no_temp_file_with_the_password_in_it() {
    let dir = tempfile::tempdir().unwrap();
    // A directory on the ini's name: the temp file is written, but cannot replace it.
    let ini = dir.path().join("BongoPlayer.ini");
    std::fs::create_dir(&ini).unwrap();
    assert!(bongo_player::args::write_ini_creds(&ini, "c:d@new.example.net:81").is_err());
    assert!(!ini.with_extension("ini.tmp").exists());
}

#[test]
fn credentials_with_nothing_on_the_command_line_come_from_the_ini() {
    match resolve_creds(None, Some("ini:pw@b.example.net:80")) {
        CredsSource::Configured(c, origin) => {
            assert_eq!(c.username, "ini");
            assert_eq!(origin, CredsOrigin::Ini);
        }
        other => panic!("expected the ini's credentials, got {other:?}"),
    }
}

#[test]
fn pasted_fields_lose_their_line_breaks() {
    // egui keeps the CR/LF that come with a paste in a single-line field.
    assert_eq!(
        credentials_line("bob", "secret", "host:8080\r\n"),
        "bob:secret@host:8080"
    );
    assert_eq!(
        credentials_line("bob\n", "se\r\ncret\n", "host:8080\n"),
        "bob:secret@host:8080"
    );
    assert_eq!(without_newlines("a\r\nb\rc\nd"), "abcd");
}

#[test]
fn a_pasted_address_loses_its_scheme_and_its_trailing_slash() {
    for server in [
        "host:8080",
        "http://host:8080/",
        "https://host:8080",
        "HTTP://host:8080//",
        "  http://host:8080/ \r\n",
    ] {
        assert_eq!(
            credentials_line("bob", "secret", server),
            "bob:secret@host:8080",
            "{server:?}"
        );
    }
    // Only a scheme at the start goes, and only those two.
    assert_eq!(
        credentials_line("bob", "secret", "ftp://host:8080"),
        "bob:secret@ftp://host:8080"
    );
    assert_eq!(
        credentials_line("bob", "secret", "my-http://host"),
        "bob:secret@my-http://host"
    );
}

#[test]
fn the_user_and_the_server_lose_the_spaces_around_them_but_the_password_keeps_its_own() {
    assert_eq!(
        credentials_line("  bob ", "  se cret ", "  host:8080 "),
        "bob:  se cret @host:8080"
    );
    assert_eq!(credentials_line("bob", "a b", "host"), "bob:a b@host");
}

#[test]
fn a_cleaned_line_reads_back_as_the_fields_it_was_made_from() {
    let line = credentials_line(
        " bob\r\n",
        "p@ss w:rd\n",
        "https://srv.example.net:8080/\r\n",
    );
    let c = parse_xtream_creds(&line).unwrap();
    assert_eq!(
        c,
        XtreamCreds {
            username: "bob".into(),
            password: "p@ss w:rd".into(),
            host: "srv.example.net".into(),
            port: 8080,
        }
    );
}
