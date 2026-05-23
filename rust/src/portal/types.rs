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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movie {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub container_extension: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    #[serde(deserialize_with = "deser_id")]
    pub series_id: i64,
    pub name: String,
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
