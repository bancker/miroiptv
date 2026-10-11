use bongo_player::args::{parse_xtream_creds, read_ini_creds, resolve_creds, CredsSource, XtreamCreds};

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
        CredsSource::Configured(c) => assert_eq!(c.username, "real"),
        other => panic!("expected the ini's credentials, got {other:?}"),
    }
}

#[test]
fn real_cli_credentials_still_win_over_the_ini() {
    match resolve_creds(Some("cli:pw@a.example.net:80"), Some("ini:pw@b.example.net:80")) {
        CredsSource::Configured(c) => assert_eq!(c.username, "cli"),
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
