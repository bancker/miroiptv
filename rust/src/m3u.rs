//! Reading an M3U list: its channels, a stable key and stream id for each,
//! and the headers a stream wants, carried in its URL. Pure: no I/O.
//!
//! Tolerant of what lists in the wild do: a BOM, CRLF line ends, no `#EXTM3U`
//! header, an `#EXTINF` with no URL after it (dropped), a URL with no `#EXTINF`
//! (named after its URL, the account in it masked), and `#` lines it does not
//! know (skipped).

use crate::portal::LiveChannel;
use std::collections::{HashMap, HashSet};

/// One channel of a list, as the file has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct M3uChannel {
    /// The name after the `#EXTINF`'s comma, else `tvg-name`, else the URL with
    /// the account in it masked (`redact`); the key of a channel with no other
    /// name, so two such channels that differ in the account only are numbered.
    pub name: String,
    /// The stream's address, without the headers that came with it.
    pub url: String,
    pub tvg_id: Option<String>,
    pub logo: Option<String>,
    /// `group-title`: one group, or several separated by `;`.
    pub group: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

/// A list as the file has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct M3uList {
    pub channels: Vec<M3uChannel>,
    /// The header's `x-tvg-url` (or `url-tvg`): the list's XMLTV guide, kept
    /// for round 2.
    pub epg_url: Option<String>,
}

/// What a stream needs sent with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

/// What a channel's stream id plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    /// The address with its headers (see [`with_headers`]).
    pub url: String,
    /// `tvg-logo`, kept for round 2.
    pub logo: Option<String>,
}

/// A list read for the app (see [`read`]).
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    /// The channels for the catalog, in file order.
    pub live: Vec<LiveChannel>,
    /// What each stream id plays.
    pub streams: HashMap<i64, Stream>,
    /// See [`M3uList::epg_url`].
    pub epg_url: Option<String>,
}

impl Playlist {
    /// About how many bytes the list keeps on the heap: what its strings hold,
    /// and the vector and the map around them. For the memory budget.
    pub fn heap_bytes(&self) -> usize {
        let opt = |s: &Option<String>| s.as_ref().map_or(0, String::capacity);
        let live: usize = self
            .live
            .iter()
            .map(|c| {
                c.name.capacity()
                    + opt(&c.category_id)
                    + opt(&c.epg_channel_id)
                    + opt(&c.stream_icon)
            })
            .sum();
        let streams: usize = self
            .streams
            .values()
            .map(|s| s.url.capacity() + opt(&s.logo))
            .sum();
        live + self.live.capacity() * std::mem::size_of::<LiveChannel>()
            + streams
            + self.streams.capacity() * (std::mem::size_of::<(i64, Stream)>() + 1)
            + opt(&self.epg_url)
    }
}

/// What an `#EXTINF` line says about the channel whose URL follows it.
#[derive(Debug, Default)]
struct Info {
    name: String,
    tvg_id: Option<String>,
    tvg_name: Option<String>,
    logo: Option<String>,
    group: Option<String>,
}

impl Info {
    /// `rest` is what follows `#EXTINF:`: the duration and the attributes, a
    /// comma, the name. The first comma outside quotes ends the attributes, so
    /// a name may hold commas of its own.
    fn of(rest: &str) -> Self {
        let mut in_quotes = false;
        let mut comma = None;
        for (i, c) in rest.char_indices() {
            match c {
                '"' => in_quotes = !in_quotes,
                ',' if !in_quotes => {
                    comma = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let (head, name) = match comma {
            Some(i) => (&rest[..i], rest[i + 1..].trim()),
            None => (rest, ""),
        };
        let pairs = pairs(head);
        Info {
            name: name.to_owned(),
            tvg_id: value_of(&pairs, "tvg-id"),
            tvg_name: value_of(&pairs, "tvg-name"),
            logo: value_of(&pairs, "tvg-logo"),
            group: value_of(&pairs, "group-title"),
        }
    }
}

/// Reads a list (see the module's doc). A line is a stream only when it holds
/// `://`; `#EXTVLCOPT` headers wait for the next one.
pub fn parse(text: &str) -> M3uList {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut list = M3uList::default();
    let mut info: Option<Info> = None;
    let mut pending = Headers::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = strip_prefix_ci(line, "#EXTM3U") {
            let pairs = pairs(rest);
            list.epg_url = value_of(&pairs, "x-tvg-url").or_else(|| value_of(&pairs, "url-tvg"));
        } else if let Some(rest) = strip_prefix_ci(line, "#EXTINF:") {
            info = Some(Info::of(rest));
        } else if let Some(rest) = strip_prefix_ci(line, "#EXTVLCOPT:") {
            if let Some((key, value)) = rest.split_once('=') {
                let value = Some(value.trim().to_owned()).filter(|v| !v.is_empty());
                match key.trim().to_ascii_lowercase().as_str() {
                    "http-user-agent" => pending.user_agent = value,
                    "http-referrer" | "http-referer" => pending.referrer = value,
                    _ => {}
                }
            }
        } else if !line.starts_with('#') && line.contains("://") {
            let (url, piped) = split_headers(line);
            let info = info.take().unwrap_or_default();
            let headers = std::mem::take(&mut pending);
            // No name at all: the address, with the account masked. The name
            // reaches the toasts, the search, the favorites and the log, and a
            // paid provider's address holds user and password.
            let name = [info.name, info.tvg_name.unwrap_or_default()]
                .into_iter()
                .find(|n| !n.is_empty())
                .unwrap_or_else(|| crate::redact::redact(url).into_owned());
            list.channels.push(M3uChannel {
                name,
                url: url.to_owned(),
                tvg_id: info.tvg_id,
                logo: info.logo,
                group: info.group,
                user_agent: piped.user_agent.or(headers.user_agent),
                referrer: piped.referrer.or(headers.referrer),
            });
        }
    }
    list
}

/// The `key="value"` (or `key=value`) pairs in `text`, keys in lower case.
fn pairs(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq]
            .rsplit(char::is_whitespace)
            .next()
            .unwrap_or("")
            .trim();
        let after = &rest[eq + 1..];
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                Some(end) => (&quoted[..end], &quoted[end + 1..]),
                None => (quoted, ""),
            },
            None => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if !key.is_empty() {
            found.push((key.to_ascii_lowercase(), value.to_owned()));
        }
        rest = next;
    }
    found
}

/// The value of `key` among `pairs`, unless it is blank.
fn value_of(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// `line` after `prefix`, when it starts with it in any case.
fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &line[prefix.len()..])
}

/// Each channel's key, in the order of `channels`: its `tvg-id` when no other
/// channel of the list has that one, else its name. A key that came before
/// gets "#2", "#3", … in file order, skipping keys that are taken already.
pub fn keys(channels: &[M3uChannel]) -> Vec<String> {
    let mut tvg_ids: HashMap<&str, usize> = HashMap::new();
    for c in channels {
        if let Some(id) = c
            .tvg_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            *tvg_ids.entry(id).or_default() += 1;
        }
    }
    let mut taken: HashSet<String> = HashSet::with_capacity(channels.len());
    let mut next: HashMap<String, usize> = HashMap::new();
    channels
        .iter()
        .map(|c| {
            let base = match c.tvg_id.as_deref().map(str::trim) {
                Some(id) if tvg_ids.get(id) == Some(&1) => id.to_owned(),
                _ => c.name.trim().to_owned(),
            };
            let mut n = next.get(&base).copied().unwrap_or(1);
            let mut key = if n == 1 {
                base.clone()
            } else {
                format!("{base}#{n}")
            };
            while taken.contains(&key) {
                n += 1;
                key = format!("{base}#{n}");
            }
            next.insert(base, n + 1);
            taken.insert(key.clone());
            key
        })
        .collect()
}

/// The stream id of a channel key: the first 8 bytes of its SHA-256 as a
/// big-endian number, shifted right by one bit so it is never negative. The
/// same on every machine and every Rust version, which std's `DefaultHasher`
/// does not promise. Shift, not mask: saved favorites and presets hold these
/// ids, so the pinned values in `tests/test_m3u.rs` must never change.
pub fn stream_id(key: &str) -> i64 {
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest.as_ref()[..8]);
    (u64::from_be_bytes(first) >> 1) as i64
}

/// The list in `text` for the app: `None` when it is no channel list (an HTML
/// page, or no stream in it). The catalog's entries come in file order, with
/// their [`keys`]' stream ids; each stream carries its headers in its URL.
pub fn read(text: &str) -> Option<Playlist> {
    if text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        return None;
    }
    let list = parse(text);
    if list.channels.is_empty() {
        return None;
    }
    let keys = keys(&list.channels);
    let mut live = Vec::with_capacity(list.channels.len());
    let mut streams = HashMap::with_capacity(list.channels.len());
    for (channel, key) in list.channels.into_iter().zip(keys) {
        let stream_id = stream_id(&key);
        let url = with_headers(
            channel.url,
            channel.user_agent.as_deref(),
            channel.referrer.as_deref(),
        );
        streams.insert(
            stream_id,
            Stream {
                url,
                logo: channel.logo,
            },
        );
        live.push(LiveChannel {
            stream_id,
            name: channel.name,
            category_id: channel.group,
            epg_channel_id: channel.tvg_id,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        });
    }
    Some(Playlist {
        live,
        streams,
        epg_url: list.epg_url,
    })
}

/// `url` with the headers its server wants, the way Kodi writes them:
/// `url|User-Agent=…&Referer=…`, the values percent-encoded. With none, `url`
/// as it is.
pub fn with_headers(url: String, user_agent: Option<&str>, referrer: Option<&str>) -> String {
    let parts: Vec<String> = [("User-Agent", user_agent), ("Referer", referrer)]
        .into_iter()
        .filter_map(|(name, value)| Some(format!("{name}={}", urlencoding::encode(value?))))
        .collect();
    if parts.is_empty() {
        url
    } else {
        format!("{url}|{}", parts.join("&"))
    }
}

/// The address and the headers of a URL that [`with_headers`] made, or that a
/// list wrote in Kodi's way. Header names count in any case, `Referrer` as
/// `Referer`; other headers are ignored.
pub fn split_headers(url: &str) -> (&str, Headers) {
    let Some((address, options)) = url.split_once('|') else {
        return (url, Headers::default());
    };
    let mut headers = Headers::default();
    for option in options.split('&') {
        let Some((name, value)) = option.split_once('=') else {
            continue;
        };
        let value = urlencoding::decode(value)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| value.to_owned());
        match name.trim().to_ascii_lowercase().as_str() {
            "user-agent" => headers.user_agent = Some(value),
            "referer" | "referrer" => headers.referrer = Some(value),
            _ => {}
        }
    }
    (address, headers)
}

/// The credentials (`user:pass@host:port`) of an Xtream portal's M3U link,
/// `…/get.php?username=…&password=…`; `None` for any other address.
pub fn xtream_link(address: &str) -> Option<String> {
    let url = reqwest::Url::parse(address.trim()).ok()?;
    let is_get_php = url.path().to_ascii_lowercase().ends_with("/get.php");
    if !matches!(url.scheme(), "http" | "https") || !is_get_php {
        return None;
    }
    let (mut user, mut pass) = (None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "username" => user = Some(value.into_owned()),
            "password" => pass = Some(value.into_owned()),
            _ => {}
        }
    }
    let user = user.filter(|u| !u.is_empty())?;
    let pass = pass.filter(|p| !p.is_empty())?;
    let port = url.port_or_known_default().unwrap_or(80);
    Some(format!("{user}:{pass}@{}:{port}", url.host_str()?))
}
