pub mod types;
pub mod xtream;

use crate::epg::Epg;
use thiserror::Error;
pub use types::*;

#[derive(Debug, Error)]
pub enum PortalError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("portal returned unexpected shape: {0}")]
    Shape(String),
}

#[async_trait::async_trait]
pub trait Portal: Send + Sync {
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
