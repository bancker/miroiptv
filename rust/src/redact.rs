//! Credentials never reach the log: portal URLs carry them in the path
//! (`/live/<user>/<pass>/…`) or in the query (`username=…&password=…`).

use std::borrow::Cow;

const PATH_KINDS: [&str; 4] = ["/live/", "/movie/", "/series/", "/timeshift/"];

/// `text` with every portal user name and password replaced by `<u>` / `<p>`.
/// Borrows when there is nothing to hide, the common case for log lines.
pub fn redact(text: &str) -> Cow<'_, str> {
    let has_path = PATH_KINDS.iter().any(|k| text.contains(k));
    let has_query = text.contains("username=") || text.contains("password=");
    if !has_path && !has_query {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_string();
    for kind in PATH_KINDS {
        out = redact_path(&out, kind);
    }
    out = redact_query(&out, "username=", "<u>");
    out = redact_query(&out, "password=", "<p>");
    if out == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(out)
    }
}

/// `…/live/<user>/<pass>/…` → `…/live/<u>/<p>/…` wherever two path segments,
/// each closed by '/', follow the marker.
fn redact_path(s: &str, kind: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(kind) {
        let after = i + kind.len();
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let end_of_url = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        let url = &tail[..end_of_url];
        match url.find('/').and_then(|a| url[a + 1..].find('/').map(|b| a + 1 + b)) {
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
            ("http://h:8080/live/bob/s3cr3t/42.m3u8", "http://h:8080/live/<u>/<p>/42.m3u8"),
            ("http://h/movie/bob/s3cr3t/9.mkv", "http://h/movie/<u>/<p>/9.mkv"),
            ("http://h/series/bob/s3cr3t/7.mp4", "http://h/series/<u>/<p>/7.mp4"),
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
}
