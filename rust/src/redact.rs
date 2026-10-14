//! Credentials never reach the log: portal URLs carry them in the path
//! (`/live/<user>/<pass>/…`) or in the query (`username=…&password=…`); an M3U
//! list's address may hold them as `user:pass@` or as a token in its query.

use std::borrow::Cow;

const PATH_KINDS: [&str; 4] = ["/live/", "/movie/", "/series/", "/timeshift/"];

/// `text` with every portal user name and password replaced by `<u>` / `<p>`,
/// the `user:pass@` of any URL by `<u>:<p>@`, and the query of a list's
/// address (`.m3u`, `.m3u8`, `get.php`) by `<q>`. Borrows when there is
/// nothing to hide, the common case for log lines.
pub fn redact(text: &str) -> Cow<'_, str> {
    let has_path = PATH_KINDS.iter().any(|k| text.contains(k));
    let has_query = text.contains("username=") || text.contains("password=");
    let has_userinfo = text.contains("://") && text.contains('@');
    let has_list = text.contains(".m3u") || text.contains("get.php");
    if !has_path && !has_query && !has_userinfo && !has_list {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_string();
    for kind in PATH_KINDS {
        out = redact_path(&out, kind);
    }
    out = redact_query(&out, "username=", "<u>");
    out = redact_query(&out, "password=", "<p>");
    out = redact_userinfo(&out);
    out = redact_list_query(&out);
    if out == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(out)
    }
}

/// The host of a web address, for a log line that must not hold the address
/// itself; "file" for anything that is no URL.
pub fn url_host(address: &str) -> String {
    reqwest::Url::parse(address.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "file".to_owned())
}

/// `scheme://user:pass@host` → `scheme://<u>:<p>@host` (`<u>@` without a
/// password), for every URL in `s`.
fn redact_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let after = i + 3;
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#' | '"' | '\'') || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(if authority[..at].contains(':') {
                    "<u>:<p>"
                } else {
                    "<u>"
                });
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The query of a list's address (`….m3u?…`, `….m3u8?…`, `…/get.php?…`) →
/// `?<q>`: a provider's token lives there as often as a password.
fn redact_list_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(q) = rest.find('?') {
        let before = &rest[..q];
        // The last word before the '?'. No byte arithmetic: a space may take
        // more than one byte.
        let word = before
            .rsplit(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '('))
            .next()
            .unwrap_or(before);
        let path = word.to_ascii_lowercase();
        let tail = &rest[q + 1..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')'))
            .unwrap_or(tail.len());
        out.push_str(before);
        if path.ends_with(".m3u") || path.ends_with(".m3u8") || path.ends_with("get.php") {
            out.push_str("?<q>");
        } else {
            out.push('?');
            out.push_str(&tail[..end]);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// `…/live/<user>/<pass>/…` → `…/live/<u>/<p>/…` wherever two path segments,
/// each closed by '/', follow the marker. The closing slashes are looked for in
/// the rest of the line, not only up to the first space or quote: the stream
/// URLs hold the raw user name and password, which may contain either.
fn redact_path(s: &str, kind: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(kind) {
        let after = i + kind.len();
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let end_of_line = tail.find(['\n', '\r']).unwrap_or(tail.len());
        let line = &tail[..end_of_line];
        match line
            .find('/')
            .and_then(|a| line[a + 1..].find('/').map(|b| a + 1 + b))
        {
            Some(second_slash) => {
                out.push_str("<u>/<p>");
                rest = &tail[second_slash..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// `key<value>` → `key<mask>`; the value ends at '&', whitespace, a quote or
/// the end of the text.
fn redact_query(s: &str, key: &str, mask: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(key) {
        let after = i + key.len();
        out.push_str(&rest[..after]);
        out.push_str(mask);
        let tail = &rest[after..];
        let end = tail
            .find(|c: char| c == '&' || c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_credentials_in_every_kind_of_stream_url() {
        for (url, want) in [
            (
                "http://h:8080/live/bob/s3cr3t/42.m3u8",
                "http://h:8080/live/<u>/<p>/42.m3u8",
            ),
            (
                "http://h/movie/bob/s3cr3t/9.mkv",
                "http://h/movie/<u>/<p>/9.mkv",
            ),
            (
                "http://h/series/bob/s3cr3t/7.mp4",
                "http://h/series/<u>/<p>/7.mp4",
            ),
            (
                "http://h/timeshift/bob/s3cr3t/35/2026-10-10:19-20/1.m3u8",
                "http://h/timeshift/<u>/<p>/35/2026-10-10:19-20/1.m3u8",
            ),
        ] {
            assert_eq!(redact(url), want);
        }
    }

    #[test]
    fn masks_query_credentials() {
        assert_eq!(
            redact("GET http://h/player_api.php?username=bob&password=s3cr3t&action=x failed"),
            "GET http://h/player_api.php?username=<u>&password=<p>&action=x failed"
        );
    }

    #[test]
    fn masks_every_url_in_a_line() {
        assert_eq!(
            redact("a http://h/live/u1/p1/1.ts b http://h/movie/u2/p2/2.mkv"),
            "a http://h/live/<u>/<p>/1.ts b http://h/movie/<u>/<p>/2.mkv"
        );
    }

    #[test]
    fn leaves_lines_without_credentials_alone() {
        // The relay's local URL has no credentials: one segment after /live/.
        let line = "loadfile http://127.0.0.1:5555/live/0.ts";
        assert!(matches!(redact(line), Cow::Borrowed(_)));
        assert!(matches!(redact("nothing to see"), Cow::Borrowed(_)));
    }

    // The stream URLs hold the raw user name and password, and an account may
    // use a space or a quote mark in either.
    #[test]
    fn masks_a_password_with_an_apostrophe() {
        assert_eq!(
            redact("resuming last watched (http://h:8080/live/bob/it's/3.m3u8)"),
            "resuming last watched (http://h:8080/live/<u>/<p>/3.m3u8)"
        );
    }

    #[test]
    fn masks_a_password_with_a_space() {
        assert_eq!(
            redact("resuming last watched (http://h:8080/live/bob/my pass/3.m3u8)"),
            "resuming last watched (http://h:8080/live/<u>/<p>/3.m3u8)"
        );
    }

    #[test]
    fn masks_a_password_with_a_double_quote() {
        assert_eq!(
            redact(r#"loadfile http://h/movie/bob/a"b/9.mkv"#),
            "loadfile http://h/movie/<u>/<p>/9.mkv"
        );
    }

    #[test]
    fn masks_a_user_name_with_a_space_or_a_quote() {
        for (url, want) in [
            (
                "http://h/live/my user/pw/3.m3u8",
                "http://h/live/<u>/<p>/3.m3u8",
            ),
            (
                "http://h/series/o'neil/pw/7.mp4",
                "http://h/series/<u>/<p>/7.mp4",
            ),
        ] {
            assert_eq!(redact(url), want);
        }
    }

    #[test]
    fn the_search_for_the_credentials_stops_at_a_line_break() {
        // The relay's local URL has no credentials; what follows on the next
        // line is no part of it.
        for line in [
            "loadfile http://127.0.0.1:5555/live/0.ts\nnext http://h/a/b/c",
            "loadfile http://127.0.0.1:5555/live/0.ts\r\nnext http://h/a/b/c",
        ] {
            assert!(matches!(redact(line), Cow::Borrowed(_)), "{line:?}");
        }
    }

    #[test]
    fn an_m3u_address_with_a_password_or_token_is_masked() {
        for (line, want) in [
            (
                "m3u: fetch https://bob:s3cr3t@lists.example/tv.m3u failed",
                "m3u: fetch https://<u>:<p>@lists.example/tv.m3u failed",
            ),
            (
                "loadfile https://lists.example/tv.m3u?token=abc123&x=1",
                "loadfile https://lists.example/tv.m3u?<q>",
            ),
            (
                "GET http://srv:8080/get.php?username=bob&password=s3cr3t&type=m3u_plus done",
                "GET http://srv:8080/get.php?<q> done",
            ),
            (
                "[mpv:ffmpeg] Opening 'https://cdn.example/live/index.m3u8?sig=zz' for reading",
                "[mpv:ffmpeg] Opening 'https://cdn.example/live/index.m3u8?<q>' for reading",
            ),
            (
                "loadfile http://u@cdn.example/x.ts",
                "loadfile http://<u>@cdn.example/x.ts",
            ),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    #[test]
    fn other_queries_and_plain_lines_stay_as_they_are() {
        // Not a list's address: its query says nothing secret (the old masks
        // still apply to username= and password=).
        let line = "GET http://h/player_api.php?action=get_live_streams ok";
        assert!(matches!(redact(line), Cow::Borrowed(_)));
        let line = "an @ in a sentence, and https://h/a.ts";
        assert!(matches!(redact(line), Cow::Borrowed(_)), "{line}");
    }

    #[test]
    fn an_address_alone_on_the_line_is_masked_too() {
        // No space, quote or bracket before it: the whole text before the '?'
        // is the address.
        for (line, want) in [
            (
                "https://lists.example/tv.m3u?token=abc",
                "https://lists.example/tv.m3u?<q>",
            ),
            (
                "https://bob:pw@lists.example/tv.m3u",
                "https://<u>:<p>@lists.example/tv.m3u",
            ),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    // `char::is_whitespace` also knows spaces of two or three bytes (U+00A0,
    // U+3000): the scan back to the start of the address must not cut one in
    // half, and the release build aborts on a panic.
    #[test]
    fn a_multi_byte_space_before_an_address_does_not_break_the_mask() {
        for (line, want) in [
            (
                "list\u{a0}https://h/tv.m3u?token=abc",
                "list\u{a0}https://h/tv.m3u?<q>",
            ),
            (
                "list\u{3000}https://h/tv.m3u8?token=abc",
                "list\u{3000}https://h/tv.m3u8?<q>",
            ),
            (
                "\u{2003}ok? https://h/a.ts\u{a0}and what.m3u?x=1",
                "\u{2003}ok? https://h/a.ts\u{a0}and what.m3u?<q>",
            ),
        ] {
            assert_eq!(redact(line), want, "{line:?}");
        }
    }

    #[test]
    fn url_host_names_the_host_or_says_file() {
        assert_eq!(
            url_host("https://bob:pw@iptv-org.github.io/iptv/index.m3u"),
            "iptv-org.github.io"
        );
        assert_eq!(
            url_host(" http://127.0.0.1:18080/m3u/list.m3u "),
            "127.0.0.1"
        );
        assert_eq!(url_host("C:\\lists\\tv.m3u"), "file");
        assert_eq!(url_host(""), "file");
    }
}
