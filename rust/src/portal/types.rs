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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveChannel {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub epg_channel_id: Option<String>,
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
