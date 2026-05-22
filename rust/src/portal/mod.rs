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
    async fn fetch_epg(&self, stream_id: i64) -> Result<Epg, PortalError>;
    async fn fetch_series_episodes(&self, series_id: i64) -> Result<Vec<Episode>, PortalError>;
    fn live_stream_url(&self, stream_id: i64) -> String;
    fn movie_stream_url(&self, stream_id: i64, container_ext: &str) -> String;
    fn series_stream_url(&self, episode_id: &str, container_ext: &str) -> String;
}
