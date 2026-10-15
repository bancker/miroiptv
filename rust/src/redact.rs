//! Credentials never reach the log: portal URLs carry them in the path
//! (`/live/<user>/<pass>/…`) or in the query (`username=…&password=…`); an M3U
//! list's address may hold them as `user:pass@` or as a token in its query, and
//! the stream URLs of a paid list in Xtream's short form,
//! `host/<user>/<pass>/<number>`.

use std::borrow::Cow;

const PATH_KINDS: [&str; 4] = ["/live/", "/movie/", "/series/", "/timeshift/"];

/// `text` with every portal user name and password replaced by `<u>` / `<p>`,
/// the `user:pass@` of any URL by `<u>:<p>@`, the user name and password of a
/// short-form stream URL (`host/<user>/<pass>/<number>`) by `<u>/<p>`, and the
/// query of a list's address (`.m3u`, `.m3u8`, `get.php`, in any letter case)
/// by `<q>`. Borrows when there is nothing to hide, the common case for log
/// lines.
pub fn redact(text: &str) -> Cow<'_, str> {
    let has_path = PATH_KINDS.iter().any(|k| text.contains(k));
    let has_query = text.contains("username=") || text.contains("password=");
    // A URL may hold `user:pass@` or be a short-form stream URL; a line
    // without `://` is neither.
    let has_url = text.contains("://");
    if !has_path && !has_query && !has_url && !has_list_marker(text) {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_string();
    for kind in PATH_KINDS {
        out = redact_path(&out, kind);
    }
    out = redact_query(&out, "username=", "<u>");
    out = redact_query(&out, "password=", "<p>");
    out = redact_userinfo(&out);
    out = redact_short_urls(&out);
    out = redact_list_query(&out);
    if out == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(out)
    }
}

/// Whether `text` holds `.m3u` or `get.php`, in any letter case: what makes an
/// address a list's. `redact_list_query` reads the address in any case, so this
/// gate must too. It looks at the dots only and does not allocate: it runs on
/// nearly every log line.
fn has_list_marker(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices('.').any(|(dot, _)| {
        bytes
            .get(dot..dot + 4)
            .is_some_and(|w| w.eq_ignore_ascii_case(b".m3u"))
            || dot
                .checked_sub(3)
                .and_then(|from| bytes.get(from..dot + 4))
                .is_some_and(|w| w.eq_ignore_ascii_case(b"get.php"))
    })
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
/// password), for every URL in `s`, also one behind the `://` of another
/// (`ffmpeg://http://user:pass@host/…`).
fn redact_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let after = i + 3;
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let authority = &tail[..authority_end(tail)];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(if authority[..at].contains(':') {
                    "<u>:<p>"
                } else {
                    "<u>"
                });
                // Go on from the '@': another URL may sit in what is left.
                rest = &tail[at..];
            }
            // Nothing to hide here, but a `://` may sit inside the host.
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Where the authority of a URL ends in `tail`, the text after its `://`: at
/// the first '/', '?', '#', quote or white space.
fn authority_end(tail: &str) -> usize {
    tail.find(|c: char| matches!(c, '/' | '?' | '#' | '"' | '\'') || c.is_whitespace())
        .unwrap_or(tail.len())
}

/// `scheme://host[:port]/<user>/<pass>/<number>[.<ext>]` → the same with
/// `<u>/<p>` for the user name and password: the short form of an Xtream
/// stream URL, as a paid provider's list holds it, with nothing in it such as
/// /live/ to find the account by. Only exactly these three path segments
/// count, the last one a number with an optional extension (`123`, `123.ts`,
/// `123.m3u8`); any other URL is left alone.
fn redact_short_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let after = i + 3;
        let tail = &rest[after..];
        let end = authority_end(tail);
        // The short form has a host, and a path that opens right behind it.
        let path = tail[end..].strip_prefix('/').filter(|_| end > 0);
        if let Some(second_slash) = path.and_then(short_form_end) {
            out.push_str(&rest[..after + end]);
            out.push_str("/<u>/<p>");
            rest = &tail[end + 1 + second_slash..];
        } else {
            // Not one, but a `://` may sit inside what looks like the host.
            out.push_str(&rest[..after]);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// `path` is what follows the '/' that opens a URL's path. When it is exactly
/// `<user>/<pass>/<number>[.<ext>]`, with neither name empty: the place of the
/// '/' that closes `<pass>`.
fn short_form_end(path: &str) -> Option<usize> {
    let second_slash = credentials_end(path)?;
    let (user, pass) = path[..second_slash].split_once('/')?;
    (!user.is_empty() && !pass.is_empty() && is_stream_number(&path[second_slash + 1..]))
        .then_some(second_slash)
}

/// Whether the last path segment is a stream number, `<digits>[.<ext>]`. `s` is
/// that segment and whatever follows the URL. The segment ends at the first
/// character that can not be part of one; if that is a '/', more segments
/// follow and it is no number.
fn is_stream_number(s: &str) -> bool {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~' | '%')))
        .unwrap_or(s.len());
    if s[end..].starts_with('/') {
        return false;
    }
    // A full stop right behind the number may end a sentence.
    let segment = s[..end].trim_end_matches('.');
    let (number, ext) = segment.split_once('.').unwrap_or((segment, ""));
    !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && ext.bytes().all(|b| b.is_ascii_alphanumeric())
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
/// each closed by '/', follow the marker (`credentials_end` finds where).
fn redact_path(s: &str, kind: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(kind) {
        let after = i + kind.len();
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        match credentials_end(tail) {
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

/// In `tail`, the place of the '/' that closes a `<user>/<pass>` at its start:
/// the second '/'. The closing slashes are looked for in the rest of the line,
/// not only up to the first space or quote: the stream URLs hold the raw user
/// name and password, which may contain either. The slashes are found first,
/// and only the text up to them is checked for a line break: looking for the
/// end of the line first would read the rest of it for every URL in it.
fn credentials_end(tail: &str) -> Option<usize> {
    let a = tail.find('/')?;
    let b = a + 1 + tail[a + 1..].find('/')?;
    (!tail[..b].contains(['\n', '\r'])).then_some(b)
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

    // The mask reads an address in any letter case, so the check that lets a
    // line reach it must too: reqwest keeps the path as it was typed.
    #[test]
    fn a_list_address_in_capitals_is_masked_too() {
        for (line, want) in [
            (
                "GET https://lists.example/TV.M3U?token=S3CR3T",
                "GET https://lists.example/TV.M3U?<q>",
            ),
            ("GET https://h/TV.M3U?token=abc", "GET https://h/TV.M3U?<q>"),
            (
                "Opening 'https://cdn.example/Index.M3u8?sig=zz' for reading",
                "Opening 'https://cdn.example/Index.M3u8?<q>' for reading",
            ),
            (
                "GET http://srv:8080/GET.PHP?token=abc done",
                "GET http://srv:8080/GET.PHP?<q> done",
            ),
            (
                "GET http://srv:8080/Get.Php?token=abc done",
                "GET http://srv:8080/Get.Php?<q> done",
            ),
            // No scheme: the capitals are the only mark of a list.
            (
                "retry lists.example/TV.M3U?token=abc later",
                "retry lists.example/TV.M3U?<q> later",
            ),
            ("GET /GET.PHP?token=abc", "GET /GET.PHP?<q>"),
            // At the very start and end of a line.
            ("get.php?x=1", "get.php?<q>"),
            ("GET.PHP?x=1", "GET.PHP?<q>"),
            (".m3u?x=1", ".m3u?<q>"),
            (".M3U8?x=1", ".M3U8?<q>"),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    #[test]
    fn a_line_that_only_looks_like_a_list_stays_as_it_is() {
        for line in [
            // Too short to hold a marker, or only almost one.
            "",
            ".",
            "..",
            "a.",
            ".m3",
            "get.ph",
            "et.php",
            "t.php",
            "a.m3x?b",
            "xget,php?b",
            // A marker, in capitals, with no query to hide.
            "loaded C:\\lists\\TV.M3U",
            "GET /GET.PHP",
            "see Index.M3u8 now",
        ] {
            assert!(matches!(redact(line), Cow::Borrowed(_)), "{line:?}");
        }
    }

    // A paid provider's list holds its stream URLs in Xtream's short form,
    // `host/<user>/<pass>/<number>`: the account in the path, and no marker
    // such as /live/ to find it by.
    #[test]
    fn a_short_form_stream_url_is_masked() {
        for (line, want) in [
            (
                "loadfile http://host:8080/bob/S3CR3T/123",
                "loadfile http://host:8080/<u>/<p>/123",
            ),
            (
                "loadfile http://h/u/p/42.ts",
                "loadfile http://h/<u>/<p>/42.ts",
            ),
            (
                "[mpv:ffmpeg] Opening 'https://h:443/u/p/42.m3u8' for reading",
                "[mpv:ffmpeg] Opening 'https://h:443/<u>/<p>/42.m3u8' for reading",
            ),
            (
                "[mpv:cplayer] Playing: http://host:8080/bob/S3CR3T/123",
                "[mpv:cplayer] Playing: http://host:8080/<u>/<p>/123",
            ),
            // Alone on the line.
            ("http://h/u/p/42.ts", "http://h/<u>/<p>/42.ts"),
            ("https://h:443/u/p/42.m3u8", "https://h:443/<u>/<p>/42.m3u8"),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    #[test]
    fn a_short_form_stream_url_is_masked_whatever_follows_it() {
        for (line, want) in [
            (
                "resuming last watched (http://h:8080/bob/S3CR3T/123.m3u8)",
                "resuming last watched (http://h:8080/<u>/<p>/123.m3u8)",
            ),
            // The full stop ends the sentence, not the number.
            ("failed: http://h/u/p/123.", "failed: http://h/<u>/<p>/123."),
            (
                "urls [http://h/u/p/1], more",
                "urls [http://h/<u>/<p>/1], more",
            ),
            ("http://h/u/p/123.ts?x=1", "http://h/<u>/<p>/123.ts?x=1"),
            (
                "http://h/u/p/123.m3u8?sig=zz",
                "http://h/<u>/<p>/123.m3u8?<q>",
            ),
            (
                "http://h/u/p/123|User-Agent=VLC",
                "http://h/<u>/<p>/123|User-Agent=VLC",
            ),
            (
                "a http://h/u1/p1/1 b http://h2:81/u2/p2/2.ts c",
                "a http://h/<u>/<p>/1 b http://h2:81/<u>/<p>/2.ts c",
            ),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    // As with /live/…, the stream URLs hold the raw user name and password, and
    // an account may use a space or a quote mark in either.
    #[test]
    fn a_short_form_stream_url_with_a_space_or_a_quote_in_the_account_is_masked() {
        for (line, want) in [
            ("http://h:8080/bob/my pass/3", "http://h:8080/<u>/<p>/3"),
            ("http://h/my user/pw/3.ts", "http://h/<u>/<p>/3.ts"),
            (
                "Opening 'http://h/bob/it's/3.ts' for reading",
                "Opening 'http://h/<u>/<p>/3.ts' for reading",
            ),
            (
                r#"loadfile http://h/bob/a"b/9"#,
                "loadfile http://h/<u>/<p>/9",
            ),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    #[test]
    fn a_url_that_is_no_short_form_stream_url_stays_as_it_is() {
        for line in [
            // Fewer or more than three path segments.
            "http://h/a/123",
            "loadfile http://h/123",
            "loadfile http://h/a/123",
            "loadfile http://h/a/b/c/123",
            // The last segment is no number with an extension.
            "http://h/a/b/name.ts",
            "loadfile http://h/a/b/name.ts",
            "loadfile http://h/a/b/12ab",
            "loadfile http://h/a/b/12-3",
            "loadfile http://h/a/b/1.ts.bak",
            // No user name, no password, no host.
            "loadfile http://h//b/123",
            "loadfile http://h/a//123",
            "loadfile file:///home/bob/123",
            // The relay's own URLs and a segment of a portal's playlist.
            "loadfile http://127.0.0.1:5555/live/0.ts",
            "loadfile http://127.0.0.1:5555/vod/7.m3u8",
            "loadfile http://h:80/hls/aa/981950_4703.ts",
            // No path, or a line break before the rest.
            "see http://h and 1/2/3",
            "loadfile http://h/u\nnext 1/2/3",
            "loadfile http://h/u\r\nnext 1/2/3",
        ] {
            assert!(matches!(redact(line), Cow::Borrowed(_)), "{line:?}");
        }
    }

    #[test]
    fn a_short_form_stream_url_and_a_list_address_in_one_line() {
        for (line, want) in [
            (
                "play http://host:8080/bob/S3CR3T/123 from https://lists.example/tv.m3u?token=abc",
                "play http://host:8080/<u>/<p>/123 from https://lists.example/tv.m3u?<q>",
            ),
            (
                "from https://lists.example/tv.m3u?token=abc play http://host:8080/bob/S3CR3T/123",
                "from https://lists.example/tv.m3u?<q> play http://host:8080/<u>/<p>/123",
            ),
            (
                "list https://bob:pw@lists.example/tv.m3u plays http://host/bob/S3CR3T/123.ts",
                "list https://<u>:<p>@lists.example/tv.m3u plays http://host/<u>/<p>/123.ts",
            ),
            // Both in one URL.
            (
                "loadfile http://bob:pw@host:8080/bob/S3CR3T/123",
                "loadfile http://<u>:<p>@host:8080/<u>/<p>/123",
            ),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    // mpv's protocol wrappers (`ffmpeg://`, `lavf://`) put a URL right behind
    // the `://` of another: the first one's host must not hide the second.
    #[test]
    fn a_url_inside_another_url_is_masked_too() {
        for (line, want) in [
            (
                "ffmpeg://http://bob:pw@host/x.ts",
                "ffmpeg://http://<u>:<p>@host/x.ts",
            ),
            (
                "lavf://http://host:8080/bob/S3CR3T/123",
                "lavf://http://host:8080/<u>/<p>/123",
            ),
            ("a://u@h://c:d@e/x", "a://<u>@h://<u>:<p>@e/x"),
        ] {
            assert_eq!(redact(line), want, "{line}");
        }
    }

    // Every `://` looks for the slashes after it. Looking for the end of the
    // line first, as it once did, read the rest of the line from each one: a
    // line of nothing but `://` took a second on the mpv thread's log path.
    #[test]
    fn a_long_line_of_url_marks_is_masked_in_one_pass() {
        let line = "://".repeat(64 * 1024 / 3);
        assert_eq!(redact(&line), line.as_str());
    }

    // The logger redacts a text that an error type has redacted already.
    #[test]
    fn masking_a_masked_line_changes_nothing() {
        for line in [
            "loadfile http://host:8080/bob/S3CR3T/123",
            "http://h/u/p/42.ts",
            "[mpv:ffmpeg] Opening 'https://h:443/u/p/42.m3u8' for reading",
            "http://h:8080/bob/my pass/3",
            "play http://host:8080/bob/S3CR3T/123 from https://lists.example/tv.m3u?token=abc",
            "list https://bob:pw@lists.example/tv.m3u plays http://host/bob/S3CR3T/123.ts",
            "GET https://lists.example/TV.M3U?token=S3CR3T",
            "http://h:8080/live/bob/S3CR3T/42.m3u8",
        ] {
            let once = redact(line).into_owned();
            assert_eq!(redact(&once), once, "{line}");
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
