//! A film's and a series' details as Xtream portals send them
//! (`get_vod_info`, `get_series_info`), read into [`VodDetails`]: only what a
//! page shows. Portals differ in names and types (a number as a string, one
//! backdrop or a list of them, episodes by season or as lists), so every field
//! is read tolerantly and an odd one is left empty, never an error. An image
//! address that `redact` would change holds an account: it is dropped.

use super::types::{rating_tenths, whole_number, EpisodeInfo, SeasonInfo, VodDetails};
use super::PortalError;
use serde_json::Value;

/// A film's details from its `get_vod_info` answer. An answer without an
/// `info` (portals answer `[]` for an id they do not know, and an object such
/// as a login's `user_info` for a refusal) is an error. An `info` that is null,
/// an empty list or anything else but an object (a film without metadata) gives
/// empty details, as a series' does.
pub fn parse_vod_info(answer: &Value) -> Result<VodDetails, PortalError> {
    let info = answer
        .get("info")
        .ok_or_else(|| PortalError::Shape("no film info".into()))?;
    let mut details = common(info);
    details.duration = seconds(info);
    if let Some(video) = info.get("video") {
        details.width = clamp_u32(whole_number(video.get("width").unwrap_or(&Value::Null)));
        details.height = clamp_u32(whole_number(video.get("height").unwrap_or(&Value::Null)));
        let transfer = text(video, &["color_transfer"]).to_ascii_lowercase();
        details.hdr = matches!(transfer.as_str(), "smpte2084" | "arib-std-b67");
    }
    if let Some(audio) = info.get("audio") {
        let channels = whole_number(audio.get("channels").unwrap_or(&Value::Null));
        details.channels = channels.clamp(0, u8::MAX as i64) as u8;
    }
    Ok(details)
}

/// A series' details from its `get_series_info` answer: its info, seasons and
/// episodes. An answer that is no object is an error; one without info, seasons
/// or episodes is a series with none of them.
pub fn parse_series_info(answer: &Value) -> Result<VodDetails, PortalError> {
    if !answer.is_object() {
        return Err(PortalError::Shape("no series info".into()));
    }
    let info = answer.get("info").filter(|i| i.is_object());
    let mut details = info.map(common).unwrap_or_default();
    if let Some(info) = info {
        // A series names its usual episode length in minutes.
        let minutes = whole_number(info.get("episode_run_time").unwrap_or(&Value::Null));
        details.duration = clamp_u32(minutes.saturating_mul(60));
    }
    details.seasons = answer
        .get("seasons")
        .and_then(Value::as_array)
        .map(|seasons| seasons.iter().filter_map(season).collect())
        .unwrap_or_default();
    details.seasons.sort_by_key(|s| s.number);
    details.seasons.dedup_by_key(|s| s.number);
    details.episodes = episodes(answer.get("episodes"));
    details.episodes.sort_by_key(|e| (e.season, e.number));
    Ok(details)
}

/// What films and series share in their `info`.
fn common(info: &Value) -> VodDetails {
    let released = text(info, &["releasedate", "release_date", "releaseDate"]);
    VodDetails {
        plot: text(info, &["plot", "description"]),
        cast: text(info, &["cast", "actors"]),
        director: text(info, &["director"]),
        genre: text(info, &["genre"]),
        year: year_of(&released),
        released,
        age: text(info, &["age", "mpaa_rating"]),
        rating: info.get("rating").map_or(0, rating_tenths),
        trailer: text(info, &["youtube_trailer", "trailer"]),
        backdrop: image(info, &["backdrop_path", "backdrop"]),
        tmdb: info.get("tmdb_id").map_or(0, whole_number),
        ..VodDetails::default()
    }
}

/// A season from the `seasons` list; none for anything that is no object.
fn season(v: &Value) -> Option<SeasonInfo> {
    v.is_object().then(|| SeasonInfo {
        number: clamp_u32(number(v, &["season_number", "number"])),
        name: text(v, &["name"]),
        cover: image(v, &["cover", "cover_big"]),
    })
}

/// The episodes of `episodes`: an object of lists keyed by season number (the
/// common shape) or a list of lists. An episode that names its season keeps
/// it; else it is in the season of its key.
fn episodes(episodes: Option<&Value>) -> Vec<EpisodeInfo> {
    let lists: Vec<(u32, &Value)> = match episodes {
        Some(Value::Object(by_season)) => by_season
            .iter()
            .map(|(key, list)| (key.trim().parse().unwrap_or(0), list))
            .collect(),
        Some(Value::Array(lists)) => lists.iter().map(|list| (0, list)).collect(),
        _ => Vec::new(),
    };
    lists
        .into_iter()
        .filter_map(|(season, list)| Some((season, list.as_array()?)))
        .flat_map(|(season, list)| list.iter().filter_map(move |e| episode(e, season)))
        .collect()
}

/// An episode; none without an id, which is what plays it.
fn episode(e: &Value, season_of_key: u32) -> Option<EpisodeInfo> {
    let id = text(e, &["id"]);
    if id.is_empty() {
        return None;
    }
    let info = e.get("info").unwrap_or(&Value::Null);
    let season = clamp_u32(number(e, &["season"]));
    Some(EpisodeInfo {
        id,
        season: if season > 0 { season } else { season_of_key },
        number: clamp_u32(number(e, &["episode_num"])),
        title: text(e, &["title"]),
        ext: text(e, &["container_extension"]),
        duration: seconds(info),
        plot: text(info, &["plot", "description"]),
        still: image(info, &["movie_image"]),
        added: e.get("added").map_or(0, whole_number),
    })
}

/// Seconds from `duration_secs`, else from `duration` ("01:47:12" or "47:12").
/// A part of the clock is whatever the portal wrote, up to the end of an `i64`:
/// the sums saturate, as `episode_run_time`'s does, and the result is cut to a
/// `u32`.
fn seconds(info: &Value) -> u32 {
    let secs = number(info, &["duration_secs"]);
    if secs > 0 {
        return clamp_u32(secs);
    }
    let clock = text(info, &["duration"]);
    let parts: Vec<i64> = clock
        .split(':')
        .map(|p| p.trim().parse::<i64>().unwrap_or(-1))
        .collect();
    let in_seconds = |h: i64, m: i64, s: i64| {
        h.saturating_mul(3600)
            .saturating_add(m.saturating_mul(60))
            .saturating_add(s)
    };
    match parts.as_slice() {
        [h, m, s] if *h >= 0 && *m >= 0 && *s >= 0 => clamp_u32(in_seconds(*h, *m, *s)),
        [m, s] if *m >= 0 && *s >= 0 => clamp_u32(in_seconds(0, *m, *s)),
        _ => 0,
    }
}

/// The first of `keys` that holds a string with text in it (or a number),
/// trimmed; empty when none does.
fn text(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|k| match v.get(*k)? {
            Value::String(s) => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// The first of `keys` with a whole number in it; 0 when none has.
fn number(v: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .filter_map(|k| v.get(*k))
        .map(whole_number)
        .find(|n| *n != 0)
        .unwrap_or(0)
}

/// An image address from the first of `keys` that has one: a string, or the
/// first string of a list. Dropped when `redact` would change it: it holds an
/// account, and the details are kept in a file.
fn image(v: &Value, keys: &[&str]) -> String {
    let found = keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_owned),
        _ => None,
    });
    match found {
        Some(url) if crate::redact::redact(&url) == url.as_str() => url,
        _ => String::new(),
    }
}

/// The year a date starts with ("1999-03-30", "1999"); 0 when it starts with
/// none from 1900 to 2100. It allocates nothing: the catalog reads one per
/// series at every load.
pub fn year_of(date: &str) -> u16 {
    match date.trim().as_bytes() {
        [a, b, c, d, ..] if [a, b, c, d].iter().all(|x| x.is_ascii_digit()) => {
            let y = [a, b, c, d]
                .iter()
                .fold(0u16, |y, x| y * 10 + u16::from(**x - b'0'));
            if (1900..=2100).contains(&y) {
                y
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn clamp_u32(n: i64) -> u32 {
    n.clamp(0, u32::MAX as i64) as u32
}
