pub mod m3u;
pub mod types;
pub mod vod_info;
pub mod xtream;

use crate::epg::Epg;
use crate::sources::SourceKind;
use thiserror::Error;
pub use types::*;

#[derive(Debug, Error)]
pub enum PortalError {
    #[error("http: {}", crate::redact::redact(&.0.to_string()))]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("portal returned unexpected shape: {0}")]
    Shape(String),
    /// The portal answered with this HTTP status instead of data (503 and 509
    /// mean it is busy).
    #[error("http status {0}")]
    Status(u16),
    /// An M3U list that is the same as the saved one: nothing to replace.
    #[error("the list has not changed")]
    Unchanged,
    /// What came is no M3U list: an HTML page, or no stream in it.
    #[error("not a channel list")]
    NotAList,
    /// An M3U file that cannot be read: its path and the system's words.
    #[error("cannot read {0}")]
    File(String),
    /// This source has no films or series to tell about (an M3U list).
    #[error("no film or series details on this source")]
    NoDetails,
}

#[async_trait::async_trait]
pub trait Portal: Send + Sync {
    /// What kind of source this is.
    fn kind(&self) -> SourceKind {
        SourceKind::Xtream
    }
    /// A catalog to show at once while `fetch_catalog` is under way: an M3U
    /// source's saved list. None for a portal that keeps none.
    async fn saved_catalog(&self) -> Option<Catalog> {
        None
    }
    /// The app no longer runs on this portal: its source was switched away
    /// from, saved with another address or deleted. A fetch still under way
    /// writes nothing for it any more (an M3U source's saved list). Once this
    /// returns, no such write is left to come.
    fn retire(&self) {}
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError>;
    /// Short "now + next few entries" EPG. Cheap, for the strip overlay
    /// and zap-toast enrichment.
    async fn fetch_epg(&self, stream_id: i64) -> Result<Epg, PortalError>;
    /// Full-day EPG schedule (includes already-aired programmes). Heavier,
    /// for the guide's right-pane programme list on archive channels where
    /// we want the catch-up history visible.
    async fn fetch_day_epg(&self, stream_id: i64) -> Result<Epg, PortalError>;
    async fn fetch_series_episodes(&self, series_id: i64) -> Result<Vec<Episode>, PortalError>;
    /// The portal's categories of films and of series (spec 2026-10-15 §4.1),
    /// fetched in the catalog's round, beside its lists. A source without
    /// films or series has none.
    // `double_must_use` (clippy 1.99) flags the boxed future that `async_trait`
    // makes of every `async fn` here; the three new methods carry the allow so
    // that they add none to the warnings the older ones already draw.
    #[allow(clippy::double_must_use)]
    async fn fetch_categories(&self) -> Result<Categories, PortalError> {
        Ok(Categories::default())
    }
    /// What a film's page shows (`get_vod_info`). A source without films has
    /// nothing to tell.
    #[allow(clippy::double_must_use)]
    async fn fetch_vod_info(&self, _vod_id: i64) -> Result<VodDetails, PortalError> {
        Err(PortalError::NoDetails)
    }
    /// What a series' page shows: its info, seasons and episodes
    /// (`get_series_info`). A source without series has nothing to tell.
    #[allow(clippy::double_must_use)]
    async fn fetch_series_info(&self, _series_id: i64) -> Result<VodDetails, PortalError> {
        Err(PortalError::NoDetails)
    }
    fn live_stream_url(&self, stream_id: i64) -> String;
    /// The stream id of a URL from [`Portal::live_stream_url`]; `None` for
    /// anything else (catch-up, VOD).
    fn live_stream_id(&self, url: &str) -> Option<i64>;
    fn movie_stream_url(&self, stream_id: i64, container_ext: &str) -> String;
    fn series_stream_url(&self, episode_id: &str, container_ext: &str) -> String;
    /// Catch-up / timeshift URL for an already-broadcast programme.
    /// `start` is the programme start time (UTC); the impl is responsible
    /// for translating to whatever wall-clock format the portal expects.
    /// `duration_min` is how many minutes of the broadcast to request.
    fn catchup_url(
        &self,
        stream_id: i64,
        start: chrono::DateTime<chrono::Utc>,
        duration_min: u32,
    ) -> String;
}
