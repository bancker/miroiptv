use super::{types::*, Portal, PortalError};
use crate::args::XtreamCreds;
use crate::epg::{Epg, EpgEntry};
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
        let qs: String = q
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!(
            "http://{}:{}/player_api.php?{}",
            self.creds.host, self.creds.port, qs
        )
    }

    fn base(&self) -> String {
        format!("http://{}:{}", self.creds.host, self.creds.port)
    }

    async fn parse_epg(&self, url: &str) -> Result<Epg, PortalError> {
        let txt = self.client.get(url).send().await?.text().await?;
        let bytes = txt.len();

        // Portals are inconsistent. We try the four shapes we've seen in
        // the wild before giving up - silently mapping an unknown shape to
        // "0 entries" is what made every channel show 'no EPG' in
        // guide-leeg.jpg / nok.jpg. Each attempt is fail-fast (serde
        // shape mismatch returns Err on the first wrong key).
        let listings: Vec<EpgRaw> = if let Ok(w) =
            serde_json::from_str::<EpgWrapper>(&txt)
        {
            w.epg_listings
        } else if let Ok(arr) = serde_json::from_str::<Vec<EpgRaw>>(&txt) {
            arr
        } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            // Last-resort: pluck any array we recognise from a top-level
            // object under one of the keys portals use.
            v.as_object()
                .and_then(|o| {
                    ["epg_listings", "epg", "data", "listings", "items"]
                        .iter()
                        .find_map(|k| o.get(*k))
                })
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|x| serde_json::from_value::<EpgRaw>(x).ok())
                .collect()
        } else {
            return Err(PortalError::Shape(format!(
                "EPG body not parseable as JSON ({} bytes)",
                bytes
            )));
        };

        let raw_count = listings.len();
        let entries: Vec<EpgEntry> = listings
            .into_iter()
            .filter_map(|r| {
                let start = as_i64(&r.start_timestamp)?;
                let end = as_i64(&r.stop_timestamp)?;
                Some(EpgEntry {
                    title: b64_decode(&r.title),
                    start: Utc.timestamp_opt(start, 0).single()?,
                    end: Utc.timestamp_opt(end, 0).single()?,
                })
            })
            .collect();

        // Empty result is the diagnostic-worthy state. Log a 300-char
        // sample of what the portal actually returned so we can adapt
        // the parser if the shape is new.
        if entries.is_empty() {
            let sample: String = txt.chars().take(300).collect();
            tracing::info!(
                "EPG empty after parse: {} bytes, {} raw listings, sample: {:?}",
                bytes,
                raw_count,
                sample
            );
        }
        Ok(Epg::new(entries))
    }
}

#[derive(Deserialize)]
struct EpgWrapper {
    #[serde(default)]
    epg_listings: Vec<EpgRaw>,
}

#[derive(Deserialize)]
struct EpgRaw {
    // Programme name. Most portals use "title"; some use "name" or
    // "programme" (XMLTV-style). Accept all.
    #[serde(default, alias = "name", alias = "programme")]
    title: String,
    // Start time as Unix epoch seconds (number or numeric string).
    // Field name varies: start_timestamp (standard) / start / start_time.
    #[serde(default, alias = "start", alias = "start_time")]
    start_timestamp: serde_json::Value,
    // End time. Field name varies: stop_timestamp (standard) / end /
    // stop / end_time / end_timestamp.
    #[serde(
        default,
        alias = "end",
        alias = "stop",
        alias = "end_time",
        alias = "end_timestamp"
    )]
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
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| s.to_owned())
}

#[async_trait::async_trait]
impl Portal for XtreamPortal {
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
        let live: Vec<LiveChannel> = self
            .client
            .get(self.api_url(&[("action", "get_live_streams")]))
            .send()
            .await?
            .json()
            .await?;
        let movies: Vec<Movie> = self
            .client
            .get(self.api_url(&[("action", "get_vod_streams")]))
            .send()
            .await?
            .json()
            .await?;
        let series: Vec<Series> = self
            .client
            .get(self.api_url(&[("action", "get_series")]))
            .send()
            .await?
            .json()
            .await?;
        Ok(Catalog {
            live,
            movies,
            series,
        })
    }

    async fn fetch_epg(&self, stream_id: i64) -> Result<Epg, PortalError> {
        let sid = stream_id.to_string();
        let url = self.api_url(&[
            ("action", "get_short_epg"),
            ("stream_id", &sid),
            ("limit", "100"),
        ]);
        self.parse_epg(&url).await
    }

    async fn fetch_day_epg(&self, stream_id: i64) -> Result<Epg, PortalError> {
        let sid = stream_id.to_string();
        let url = self.api_url(&[("action", "get_simple_data_table"), ("stream_id", &sid)]);
        self.parse_epg(&url).await
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
                        let id = ep
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_owned();
                        let title = ep
                            .get("title")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_owned();
                        let ext = ep
                            .get("container_extension")
                            .and_then(|x| x.as_str())
                            .unwrap_or("mp4")
                            .to_owned();
                        let n = ep
                            .get("episode_num")
                            .and_then(|x| x.as_i64())
                            .or_else(|| {
                                ep.get("episode_num")
                                    .and_then(|x| x.as_str())
                                    .and_then(|s| s.parse().ok())
                            })
                            .unwrap_or(0);
                        out.push(Episode {
                            id,
                            title,
                            container_extension: ext,
                            season,
                            episode_num: n,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    fn live_stream_url(&self, stream_id: i64) -> String {
        format!(
            "{}/live/{}/{}/{}.m3u8",
            self.base(),
            self.creds.username,
            self.creds.password,
            stream_id
        )
    }

    fn movie_stream_url(&self, stream_id: i64, container_ext: &str) -> String {
        format!(
            "{}/movie/{}/{}/{}.{}",
            self.base(),
            self.creds.username,
            self.creds.password,
            stream_id,
            container_ext
        )
    }

    fn series_stream_url(&self, episode_id: &str, container_ext: &str) -> String {
        format!(
            "{}/series/{}/{}/{}.{}",
            self.base(),
            self.creds.username,
            self.creds.password,
            episode_id,
            container_ext
        )
    }

    fn catchup_url(
        &self,
        stream_id: i64,
        start: chrono::DateTime<chrono::Utc>,
        duration_min: u32,
    ) -> String {
        // Xtream Codes timeshift convention:
        //   /timeshift/<user>/<pass>/<duration_min>/<YYYY-MM-DD:HH-MM>/<stream_id>.m3u8
        // The timestamp is in portal-local time. We use OS local time as a
        // best-effort approximation - works as long as the user runs the
        // app in the same timezone as the portal.
        let local = start.with_timezone(&chrono::Local);
        let ts = local.format("%Y-%m-%d:%H-%M");
        format!(
            "{}/timeshift/{}/{}/{}/{}/{}.m3u8",
            self.base(),
            self.creds.username,
            self.creds.password,
            duration_min,
            ts,
            stream_id
        )
    }
}
