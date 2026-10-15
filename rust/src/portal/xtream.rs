use super::{types::*, Portal, PortalError};
use crate::args::XtreamCreds;
use crate::epg::{Epg, EpgEntry};
use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use std::time::Duration;

/// What a portal stream URL points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Live,
    Movie,
    Series,
    Timeshift,
}

/// Kind and stream id of a portal URL (`…/live/u/p/42.m3u8` → live 42). The
/// id is the last path segment without its extension. `None` for URLs that
/// are not portal streams.
pub fn stream_ref(url: &str) -> Option<(StreamKind, i64)> {
    let path = url.split('?').next()?;
    let kind = [
        ("/live/", StreamKind::Live),
        ("/movie/", StreamKind::Movie),
        ("/series/", StreamKind::Series),
        ("/timeshift/", StreamKind::Timeshift),
    ]
    .into_iter()
    .find(|(marker, _)| path.contains(marker))?
    .1;
    let id = path.rsplit('/').next()?.split('.').next()?.parse().ok()?;
    Some((kind, id))
}

/// `Debug` for the tests only: it would print the password.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct XtreamPortal {
    pub creds: XtreamCreds,
    client: reqwest::Client,
}

impl XtreamPortal {
    pub fn new(creds: XtreamCreds) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("{}/{}", crate::APP_ID, env!("CARGO_PKG_VERSION")))
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
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            // A busy portal (503, 509) answers with an error page. That is not
            // "no programmes": whoever asked must know it failed.
            return Err(PortalError::Status(status.as_u16()));
        }
        let txt = resp.text().await?;
        parse_epg_body(&txt)
    }
}

/// Outcome of trying portal credentials (settings screen, setup prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnCheck {
    /// The login works. `channels` is the number of live channels when the
    /// portal's list arrived in time. The count is only a bonus: a slow, busy or
    /// odd answer to the list leaves it `None`, and the credentials are good all
    /// the same.
    Ok { channels: Option<usize> },
    /// The portal refuses the user name or the password.
    BadLogin,
    /// No answer from the server within the time allowed, or none that says
    /// anything about the login: this portal answers 503 or 509 when it is busy.
    Unreachable,
    /// The login got an answer that no Xtream portal gives.
    Invalid,
}

/// How long a whole credentials test may take.
const CHECK_BUDGET: Duration = Duration::from_secs(10);

impl XtreamPortal {
    /// Log in with these credentials and count the live channels, all within 10 s.
    pub async fn check(&self) -> ConnCheck {
        self.check_within(CHECK_BUDGET).await
    }

    /// `check` with another time budget. It is one budget for the whole test,
    /// login and list together. The tests use a short one.
    pub async fn check_within(&self, budget: Duration) -> ConnCheck {
        let deadline = tokio::time::Instant::now() + budget;
        match tokio::time::timeout_at(deadline, self.login()).await {
            Ok(Ok(())) => {}
            Ok(Err(refused)) => return refused,
            Err(_) => return ConnCheck::Unreachable,
        }
        // The credentials work. Whatever goes wrong with the list, they do.
        let channels = tokio::time::timeout_at(deadline, self.count_live())
            .await
            .ok()
            .flatten();
        ConnCheck::Ok { channels }
    }

    /// The login: `Ok` when `user_info.auth` is 1, else what was wrong.
    async fn login(&self) -> Result<(), ConnCheck> {
        let answer = self
            .client
            .get(self.api_url(&[]))
            .send()
            .await
            .map_err(|_| ConnCheck::Unreachable)?;
        if matches!(answer.status().as_u16(), 401 | 403) {
            return Err(ConnCheck::BadLogin);
        }
        // Any other refusal (503 and 509 are this portal's normal busy answers)
        // says nothing about the credentials: the server gave no usable answer.
        if !answer.status().is_success() {
            return Err(ConnCheck::Unreachable);
        }
        let body: serde_json::Value = answer.json().await.map_err(|_| ConnCheck::Invalid)?;
        let auth = body
            .get("user_info")
            .and_then(|u| u.get("auth"))
            .ok_or(ConnCheck::Invalid)?;
        if auth.as_i64() == Some(1) || auth.as_str() == Some("1") {
            Ok(())
        } else {
            Err(ConnCheck::BadLogin)
        }
    }

    /// The number of live channels; `None` unless the portal answers with a
    /// JSON array.
    async fn count_live(&self) -> Option<usize> {
        let answer = self
            .client
            .get(self.api_url(&[("action", "get_live_streams")]))
            .send()
            .await
            .ok()?;
        if !answer.status().is_success() {
            return None;
        }
        // Counted, not kept: a big portal's list is megabytes of channels.
        let list: Vec<serde::de::IgnoredAny> = answer.json().await.ok()?;
        Some(list.len())
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
    // Xtream standard: Unix epoch seconds (number or numeric string).
    #[serde(default)]
    start_timestamp: serde_json::Value,
    #[serde(default)]
    stop_timestamp: serde_json::Value,
    // Formatted local datetimes ("YYYY-MM-DD HH:MM:SS"). Kept as DISTINCT
    // fields, NOT serde aliases of *_timestamp: many portals send BOTH
    // `start` and `start_timestamp`, and an alias makes serde raise a
    // duplicate-field error that silently zeroes every listing (the v0.3.x
    // "geen EPG" regression). Used only as a fallback when no epoch is given.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    stop: Option<String>,
    // 1 when the portal keeps the programme for catch-up
    // (get_simple_data_table); a number, a numeric string or a bool.
    #[serde(default)]
    has_archive: serde_json::Value,
}

fn as_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// `has_archive` as the portal sends it; `None` when it is missing or odd.
fn archive_flag(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        other => as_i64(other).map(|n| n != 0),
    }
}

/// How many programmes say `has_archive` 1, 0, or nothing.
fn archive_counts(entries: &[EpgEntry]) -> (usize, usize, usize) {
    entries
        .iter()
        .fold((0, 0, 0), |(yes, no, unknown), e| match e.has_archive {
            Some(true) => (yes + 1, no, unknown),
            Some(false) => (yes, no + 1, unknown),
            None => (yes, no, unknown + 1),
        })
}

fn b64_decode(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| s.to_owned())
}

/// Resolve a programme timestamp: prefer the explicit Unix epoch, else fall
/// back to a "YYYY-MM-DD HH:MM:SS" local datetime string.
fn epoch_or_dt(ts: &serde_json::Value, dt: Option<&str>) -> Option<chrono::DateTime<Utc>> {
    if let Some(epoch) = as_i64(ts) {
        if epoch > 0 {
            return Utc.timestamp_opt(epoch, 0).single();
        }
    }
    let naive = NaiveDateTime::parse_from_str(dt?.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|l| l.with_timezone(&Utc))
}

/// Parse an Xtream EPG body into an `Epg`. Tolerant of the shapes seen in the
/// wild (wrapped `{epg_listings:[...]}`, bare array, or a top-level object
/// under a known key). Pure + sync so it's unit-testable without HTTP.
///
/// A body that is no JSON (an error page or nothing served with 200, a cut-off
/// download), or JSON that is neither an object nor a list, is an error, not
/// "no programmes": the guide would keep an empty listing for an hour in place
/// of the EPG it had, and write it to its cache file. JSON with an empty list
/// is an empty EPG.
fn parse_epg_body(txt: &str) -> Result<Epg, PortalError> {
    let bytes = txt.len();
    let listings: Vec<EpgRaw> = if let Ok(w) = serde_json::from_str::<EpgWrapper>(txt) {
        w.epg_listings
    } else if let Ok(arr) = serde_json::from_str::<Vec<EpgRaw>>(txt) {
        arr
    } else {
        let v = match serde_json::from_str::<serde_json::Value>(txt) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("EPG body not parseable as JSON ({} bytes)", bytes);
                return Err(PortalError::Json(e));
            }
        };
        if !(v.is_object() || v.is_array()) {
            tracing::warn!("EPG body is JSON but no object or list ({} bytes)", bytes);
            return Err(PortalError::Shape("EPG body is no object or list".into()));
        }
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
    };

    let raw_count = listings.len();
    let entries: Vec<EpgEntry> = listings
        .into_iter()
        .filter_map(|r| {
            Some(EpgEntry {
                title: b64_decode(&r.title),
                start: epoch_or_dt(&r.start_timestamp, r.start.as_deref())?,
                end: epoch_or_dt(&r.stop_timestamp, r.end.as_deref().or(r.stop.as_deref()))?,
                has_archive: archive_flag(&r.has_archive),
            })
        })
        .collect();

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

#[cfg(test)]
mod epg_tests {
    use super::{parse_epg_body, PortalError};

    #[test]
    fn a_body_that_is_no_json_is_an_error() {
        let cut_off = r#"{"epg_listings":[{"title":"QQ==","start_timestamp":"17810"#;
        for body in ["<html>Service Unavailable</html>", "", cut_off] {
            let parsed = parse_epg_body(body);
            assert!(matches!(parsed, Err(PortalError::Json(_))), "{body:?}");
        }
    }

    #[test]
    fn json_that_is_neither_an_object_nor_a_list_is_an_error() {
        for body in ["null", "\"busy\"", "42", "true"] {
            let parsed = parse_epg_body(body);
            assert!(matches!(parsed, Err(PortalError::Shape(_))), "{body:?}");
        }
    }

    #[test]
    fn an_empty_list_in_any_shape_is_an_empty_epg() {
        for body in [r#"{"epg_listings":[]}"#, "[]", "{}", r#"{"data":[]}"#] {
            let epg = parse_epg_body(body).unwrap_or_else(|e| panic!("{body:?}: {e}"));
            assert!(epg.entries().is_empty(), "{body:?}");
        }
    }

    #[test]
    fn parses_listing_with_both_start_and_start_timestamp() {
        // The user's portal shape: each listing carries BOTH the datetime
        // `start`/`end` AND the epoch `start_timestamp`/`stop_timestamp`. The
        // old serde-alias version tripped a duplicate-field error -> 0 entries.
        let body = r#"{"epg_listings":[
            {"id":"1","title":"QmVsb3cgRGVjayBEb3duIFVuZGVy","lang":"",
             "start":"2026-06-16 18:10:00","end":"2026-06-16 19:05:00",
             "start_timestamp":"1781021400","stop_timestamp":"1781024700"}
        ]}"#;
        let epg = parse_epg_body(body).unwrap();
        assert_eq!(epg.entries().len(), 1, "must parse despite dual fields");
        assert_eq!(epg.entries()[0].title, "Below Deck Down Under");
    }

    #[test]
    fn parses_epoch_only() {
        let body = r#"{"epg_listings":[{"title":"VGVzdA==","start_timestamp":1781021400,"stop_timestamp":1781024700}]}"#;
        assert_eq!(parse_epg_body(body).unwrap().entries().len(), 1);
    }

    #[test]
    fn parses_datetime_only_fallback() {
        let body = r#"{"epg_listings":[{"title":"VGVzdA==","start":"2026-06-16 18:10:00","end":"2026-06-16 19:05:00"}]}"#;
        assert_eq!(parse_epg_body(body).unwrap().entries().len(), 1);
    }

    #[test]
    fn empty_listings_is_no_entries() {
        let epg = parse_epg_body(r#"{"epg_listings":[]}"#).unwrap();
        assert_eq!(epg.entries().len(), 0);
    }

    #[test]
    fn bare_array_shape_parses() {
        let body =
            r#"[{"title":"VGVzdA==","start_timestamp":"1781021400","stop_timestamp":"1781024700"}]"#;
        assert_eq!(parse_epg_body(body).unwrap().entries().len(), 1);
    }

    #[test]
    fn reads_has_archive_in_every_form() {
        let body = r#"{"epg_listings":[
            {"title":"QQ==","start_timestamp":"1781021400","stop_timestamp":"1781024700","has_archive":1},
            {"title":"Qg==","start_timestamp":"1781024700","stop_timestamp":"1781028300","has_archive":"0"},
            {"title":"Qw==","start_timestamp":"1781028300","stop_timestamp":"1781031900","has_archive":true},
            {"title":"RA==","start_timestamp":"1781031900","stop_timestamp":"1781035500"}
        ]}"#;
        let flags: Vec<Option<bool>> = parse_epg_body(body)
            .unwrap()
            .entries()
            .iter()
            .map(|e| e.has_archive)
            .collect();
        assert_eq!(flags, [Some(true), Some(false), Some(true), None]);
    }
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
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            // A busy portal (503, 509) answers with an error page. That is not
            // "no programmes": the guide must wait before it asks again.
            return Err(PortalError::Status(status.as_u16()));
        }
        let txt = resp.text().await?;
        let epg = parse_epg_body(&txt)?;
        let (yes, no, unknown) = archive_counts(epg.entries());
        tracing::info!(
            "epg: day table for sid={} - {} KB, {} programmes, has_archive 1/0/-: {}/{}/{}",
            stream_id,
            txt.len() / 1024,
            epg.entries().len(),
            yes,
            no,
            unknown
        );
        Ok(epg)
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

    fn live_stream_id(&self, url: &str) -> Option<i64> {
        match stream_ref(url) {
            Some((StreamKind::Live, id)) => Some(id),
            _ => None,
        }
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
