use crate::args::XtreamCreds;
use crate::epg::{Epg, EpgEntry};
use super::{Portal, PortalError, types::*};
use chrono::{TimeZone, Utc};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct XtreamPortal {
    pub creds: XtreamCreds,
    client: reqwest::Client,
}

impl XtreamPortal {
    pub fn new(creds: XtreamCreds) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("tvplayer/0.1")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("build reqwest client");
        Self { creds, client }
    }

    fn api_url(&self, params: &[(&str, &str)]) -> String {
        let mut q: Vec<(&str, &str)> = vec![
            ("username", self.creds.username.as_str()),
            ("password", self.creds.password.as_str()),
        ];
        q.extend_from_slice(params);
        let qs: String = q.iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("http://{}:{}/player_api.php?{}", self.creds.host, self.creds.port, qs)
    }

    fn base(&self) -> String {
        format!("http://{}:{}", self.creds.host, self.creds.port)
    }
}

#[derive(Deserialize)]
struct EpgWrapper {
    #[serde(default)]
    epg_listings: Vec<EpgRaw>,
}

#[derive(Deserialize)]
struct EpgRaw {
    #[serde(default)]
    title: String,
    #[serde(default)]
    start_timestamp: serde_json::Value,
    #[serde(default)]
    stop_timestamp: serde_json::Value,
}

fn as_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn b64_decode(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| s.to_owned())
}

#[async_trait::async_trait]
impl Portal for XtreamPortal {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        let live: Vec<LiveChannel> = self.client
            .get(self.api_url(&[("action", "get_live_streams")]))
            .send().await?.json().await?;
        let movies: Vec<Movie> = self.client
            .get(self.api_url(&[("action", "get_vod_streams")]))
            .send().await?.json().await?;
        let series: Vec<Series> = self.client
            .get(self.api_url(&[("action", "get_series")]))
            .send().await?.json().await?;
        Ok(Catalog { live, movies, series })
    }

    async fn fetch_epg(&self, stream_id: i64) -> Result<Epg, PortalError> {
        let sid = stream_id.to_string();
        let url = self.api_url(&[
            ("action", "get_short_epg"),
            ("stream_id", &sid),
            ("limit", "100"),
        ]);
        let txt = self.client.get(url).send().await?.text().await?;
        let wrap: EpgWrapper = serde_json::from_str(&txt)
            .map_err(|e| PortalError::Shape(e.to_string()))?;
        let entries: Vec<EpgEntry> = wrap.epg_listings.into_iter().filter_map(|r| {
            let start = as_i64(&r.start_timestamp)?;
            let end   = as_i64(&r.stop_timestamp)?;
            Some(EpgEntry {
                title: b64_decode(&r.title),
                start: Utc.timestamp_opt(start, 0).single()?,
                end:   Utc.timestamp_opt(end, 0).single()?,
            })
        }).collect();
        Ok(Epg::new(entries))
    }

    async fn fetch_series_episodes(&self, series_id: i64) -> Result<Vec<Episode>, PortalError> {
        let sid = series_id.to_string();
        let url = self.api_url(&[("action", "get_series_info"), ("series_id", &sid)]);
        let v: serde_json::Value = self.client.get(url).send().await?.json().await?;
        let mut out = Vec::new();
        if let Some(seasons) = v.get("episodes").and_then(|x| x.as_object()) {
            for (season_str, eps) in seasons.iter() {
                let season: i64 = season_str.parse().unwrap_or(0);
                if let Some(arr) = eps.as_array() {
                    for ep in arr {
                        let id = ep.get("id").and_then(|x| x.as_str()).unwrap_or("").to_owned();
                        let title = ep.get("title").and_then(|x| x.as_str()).unwrap_or("").to_owned();
                        let ext = ep.get("container_extension").and_then(|x| x.as_str()).unwrap_or("mp4").to_owned();
                        let n = ep.get("episode_num").and_then(|x| x.as_i64())
                            .or_else(|| ep.get("episode_num").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
                            .unwrap_or(0);
                        out.push(Episode { id, title, container_extension: ext, season, episode_num: n });
                    }
                }
            }
        }
        Ok(out)
    }

    fn live_stream_url(&self, stream_id: i64) -> String {
        format!("{}/live/{}/{}/{}.m3u8",
            self.base(), self.creds.username, self.creds.password, stream_id)
    }

    fn movie_stream_url(&self, stream_id: i64, container_ext: &str) -> String {
        format!("{}/movie/{}/{}/{}.{}",
            self.base(), self.creds.username, self.creds.password, stream_id, container_ext)
    }

    fn series_stream_url(&self, episode_id: &str, container_ext: &str) -> String {
        format!("{}/series/{}/{}/{}.{}",
            self.base(), self.creds.username, self.creds.password, episode_id, container_ext)
    }
}
