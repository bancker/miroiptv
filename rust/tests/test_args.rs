use tvplayer::args::{parse_xtream_creds, XtreamCreds};

#[test]
fn parses_xtream_creds_with_port() {
    let c = parse_xtream_creds("user:pass@host.example.com:8080").unwrap();
    assert_eq!(c, XtreamCreds {
        username: "user".into(),
        password: "pass".into(),
        host: "host.example.com".into(),
        port: 8080,
    });
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
