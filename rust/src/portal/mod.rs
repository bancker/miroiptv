pub mod m3u;
pub mod types;
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
    async fn fetch_catalog(&self) -> Result<Catalog, PortalError>;
    /// Short "now + next few entries" EPG. Cheap, for the strip overlay
    /// and zap-toast enrichment.
    async fn fetch_epg(&self, stream_id: i64) -> Result<Epg, PortalError>;
    /// Full-day EPG schedule (includes already-aired programmes). Heavier,
    /// for the guide's right-pane programme list on archive channels where
    /// we want the catch-up history visible.
    async fn fetch_day_epg(&self, stream_id: i64) -> Result<Epg, PortalError>;
    async fn fetch_series_episodes(&self, series_id: i64) -> Result<Vec<Episode>, PortalError>;
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
