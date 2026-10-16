use serde::{Deserialize, Serialize};

fn deser_id<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("not an i64")),
        serde_json::Value::String(s) => s.parse::<i64>().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("expected number or string")),
    }
}

/// Tolerant int deserializer: accepts JSON number, numeric string, null, or
/// missing field. Returns 0 for anything that isn't a valid integer.
/// Xtream portals are inconsistent about whether `tv_archive` comes back
/// as `0`/`1` (int) or `"0"`/`"1"` (string).
fn deser_int_or_zero<'de, D>(d: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
        Some(serde_json::Value::String(s)) => s.parse::<i32>().unwrap_or(0),
        _ => 0,
    })
}

/// Tolerant yes/no flag: `1`, `"1"`, `true` and `"true"` mean yes; anything
/// else, null or a missing field mean no. The portal sends `is_adult` as
/// `"0"`/`"1"`.
fn deser_flag<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(match v {
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true")
        }
        _ => false,
    })
}

/// Tolerant optional string: a JSON string gives `Some`; null, a missing field
/// or anything else (`false`, a number) gives `None`. Portals type fields
/// loosely (see `deser_flag`), and one odd value must not fail the whole list.
fn deser_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(match v {
        Some(serde_json::Value::String(s)) => Some(s),
        _ => None,
    })
}

/// Tolerant string: a JSON string as it is; anything else (null, a number, a
/// missing field) is empty.
fn deser_string_or_empty<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(deser_opt_string(d)?.unwrap_or_default())
}

/// Tolerant whole number: see [`whole_number`].
fn deser_i64_or_zero<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(v.as_ref().map_or(0, whole_number))
}

/// A whole number as portals send one: a JSON number (a fraction is cut off)
/// or a numeric string; 0 for anything else (null, "", a word).
pub(crate) fn whole_number(v: &serde_json::Value) -> i64 {
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// A rating as portals send one ("7.3", 7.3, "7,3", "" or null) in tenths, 0
/// to 100; 0 (no rating) for anything else, and for a value outside 0 to 10.
pub(crate) fn rating_tenths(v: &serde_json::Value) -> u8 {
    let x = match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().replace(',', ".").parse::<f64>().ok(),
        _ => None,
    };
    match x {
        Some(x) if x.is_finite() && (0.0..=10.0).contains(&x) => (x * 10.0).round() as u8,
        _ => 0,
    }
}

/// Tolerant rating: see [`rating_tenths`].
fn deser_rating<'de, D>(d: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(v.as_ref().map_or(0, rating_tenths))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveChannel {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub epg_channel_id: Option<String>,
    /// 1 = portal exposes a timeshift / catch-up endpoint for this channel.
    /// In NL Xtream portals the live and archive variants of the same
    /// programme are TWO DISTINCT entries with the same name: the live
    /// one has `tv_archive=0` and serves `/live/<u>/<p>/<id>.m3u8`; the
    /// archive one has `tv_archive=1` and serves
    /// `/timeshift/<u>/<p>/<dur>/<ts>/<id>.m3u8`. Hitting the wrong
    /// variant returns HTML / 502.
    #[serde(default, deserialize_with = "deser_int_or_zero")]
    pub tv_archive: i32,
    /// Days of catch-up history available on this archive channel.
    #[serde(default, deserialize_with = "deser_int_or_zero")]
    pub tv_archive_duration: i32,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
    /// The channel's logo URL, if the portal has one (often empty). Only the
    /// guide's list keeps logos (`GuideChannel::logo`); the stored list drops them.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub stream_icon: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Movie {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub container_extension: Option<String>,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
    /// The cover (`stream_icon`). The title index takes it when the catalog
    /// is stored: a stored catalog's films have none.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub stream_icon: Option<String>,
    /// The portal's rating in tenths (73: ★ 7.3); 0: none.
    #[serde(default, deserialize_with = "deser_rating")]
    pub rating: u8,
    /// When the portal added it, unix seconds; 0: unknown.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub added: i64,
    /// Its category (`get_vod_categories`); 0: none.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Series {
    #[serde(deserialize_with = "deser_id")]
    pub series_id: i64,
    pub name: String,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
    /// The cover. The title index takes it, as a film's.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub cover: Option<String>,
    /// The portal's rating in tenths; 0: none.
    #[serde(default, deserialize_with = "deser_rating")]
    pub rating: u8,
    /// `releaseDate` ("2019-05-03"); see [`Series::released`]. The title index
    /// takes it.
    #[serde(default, rename = "releaseDate", deserialize_with = "deser_opt_string")]
    pub release_date_camel: Option<String>,
    /// `release_date`, which some portals send instead, or as well: a field of
    /// its own, since a serde alias fails the whole list on a series that has
    /// both (a duplicate field, as `start` and `start_timestamp` once did to
    /// the EPG). The title index takes it.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub release_date: Option<String>,
    /// The genres as the portal names them ("Drama, Comedy"). The title index
    /// takes it.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub genre: Option<String>,
    /// Its category (`get_series_categories`); 0: none.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
    /// When the portal last changed it, unix seconds; 0: unknown.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub last_modified: i64,
}

impl Series {
    /// Its release date: `releaseDate`, else `release_date`; none when both are
    /// missing or empty.
    pub fn released(&self) -> Option<&str> {
        [&self.release_date_camel, &self.release_date]
            .into_iter()
            .flatten()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
    }
}

/// One of the portal's categories (`get_vod_categories`,
/// `get_series_categories`): its id and name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
    #[serde(default, deserialize_with = "deser_string_or_empty")]
    pub category_name: String,
}

/// The portal's categories of films and of series: two id spaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Categories {
    pub vod: Vec<Category>,
    pub series: Vec<Category>,
}

/// What a film's or a series' page shows that the lists do not hold
/// (`get_vod_info`, `get_series_info`; spec 2026-10-15 §4.3): only those
/// fields, and no image address with an account in it. `vod::details` keeps
/// it as a file per title.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VodDetails {
    pub plot: String,
    pub cast: String,
    pub director: String,
    /// The genres as the portal names them.
    pub genre: String,
    /// The release date as the portal gives it ("1999-03-30").
    pub released: String,
    /// The year of `released`; 0: unknown.
    pub year: u16,
    /// The age rating as the portal gives it ("16", "PG-13").
    pub age: String,
    /// A film's length, a series' usual episode length: seconds; 0: unknown.
    pub duration: u32,
    /// ★ in tenths; 0: none.
    pub rating: u8,
    /// The YouTube code (`youtube_trailer`) as the portal gives it; the
    /// trailer checks it before it is used.
    pub trailer: String,
    /// The backdrop image (`backdrop_path`, the first one).
    pub backdrop: String,
    /// The id at TMDB; 0: none.
    pub tmdb: i64,
    /// The picture as the portal measured it: pixels, and whether it is HDR.
    pub width: u32,
    pub height: u32,
    pub hdr: bool,
    /// Sound channels: 6 is 5.1; 0: unknown.
    pub channels: u8,
    /// A series' seasons, by number; a film has none.
    pub seasons: Vec<SeasonInfo>,
    /// A series' episodes, by season and number; a film has none.
    pub episodes: Vec<EpisodeInfo>,
}

/// A season of a series: its number (0: the extras), name and cover.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeasonInfo {
    pub number: u32,
    pub name: String,
    pub cover: String,
}

/// An episode of a series, as its page lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EpisodeInfo {
    /// Its id as the portal gives it: what `Portal::series_stream_url` plays.
    pub id: String,
    pub season: u32,
    pub number: u32,
    pub title: String,
    /// Its container (`container_extension`): "mkv", "mp4".
    pub ext: String,
    /// Seconds; 0: unknown.
    pub duration: u32,
    pub plot: String,
    /// Its still (`movie_image`).
    pub still: String,
    /// When the portal added it, unix seconds; 0: unknown.
    pub added: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub title: String,
    pub container_extension: String,
    pub season: i64,
    pub episode_num: i64,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub live: Vec<LiveChannel>,
    pub movies: Vec<Movie>,
    pub series: Vec<Series>,
}
