use crate::portal::{Catalog as PortalCatalog, LiveChannel, Portal};
use crate::search::{rank, ItemKind, SearchItem};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStatus {
    Idle,
    Fetching(String),  // current step description
    Loaded,
    Failed(String),
}

pub struct CatalogStore {
    inner: Arc<RwLock<Option<PortalCatalog>>>,
    portal: Arc<dyn Portal>,
    status: Arc<RwLock<CatalogStatus>>,
}

impl CatalogStore {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            portal,
            status: Arc::new(RwLock::new(CatalogStatus::Idle)),
        }
    }

    /// Spawn an async fetch on the current tokio runtime. Non-blocking.
    /// Safe to call repeatedly: subsequent calls trigger a re-fetch.
    pub fn spawn_fetch(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let portal = self.portal.clone();
        let status = self.status.clone();
        *status.write() = CatalogStatus::Fetching("connecting...".into());

        tokio::spawn(async move {
            let t0 = Instant::now();
            *status.write() = CatalogStatus::Fetching("fetching catalog...".into());
            info!("catalog: starting fetch");

            match portal.fetch_catalog().await {
                Ok(c) => {
                    let elapsed_ms = t0.elapsed().as_millis();
                    info!(
                        "catalog: loaded in {} ms ({} live, {} movies, {} series)",
                        elapsed_ms,
                        c.live.len(),
                        c.movies.len(),
                        c.series.len()
                    );
                    *inner.write() = Some(c);
                    *status.write() = CatalogStatus::Loaded;
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    warn!("catalog: fetch failed after {} ms: {}", t0.elapsed().as_millis(), msg);
                    *status.write() = CatalogStatus::Failed(msg);
                }
            }
        });
    }

    pub fn status(&self) -> CatalogStatus {
        self.status.read().clone()
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.read().is_some()
    }

    pub fn live_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| c.live.clone())
            .unwrap_or_default()
    }

    /// Archive (catch-up) channels: `tv_archive == 1`. These serve
    /// `/timeshift/...` and `get_simple_data_table` returns the full
    /// catch-up history; the matching live channel (`tv_archive == 0`)
    /// is needed for live URLs.
    pub fn archive_channels(&self) -> Vec<LiveChannel> {
        self.inner
            .read()
            .as_ref()
            .map(|c| {
                c.live
                    .iter()
                    .filter(|x| x.tv_archive == 1)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find the live (non-archive) variant of a channel by exact name match.
    /// Used by the guide when the user picks an airing-now programme on an
    /// archive channel - we have to swap to the live id to actually play.
    pub fn live_id_by_name(&self, name: &str) -> Option<i64> {
        self.inner.read().as_ref().and_then(|c| {
            c.live
                .iter()
                .find(|x| x.tv_archive != 1 && x.name == name)
                .map(|x| x.stream_id)
        })
    }

    pub fn search_items(&self) -> Vec<SearchItem> {
        let g = self.inner.read();
        let Some(c) = g.as_ref() else {
            return Vec::new();
        };
        let mut v = Vec::with_capacity(c.live.len() + c.movies.len() + c.series.len());
        for x in &c.live {
            v.push(SearchItem {
                id: x.stream_id,
                name: x.name.clone(),
                kind: ItemKind::Live,
            });
        }
        for x in &c.movies {
            v.push(SearchItem {
                id: x.stream_id,
                name: x.name.clone(),
                kind: ItemKind::Movie,
            });
        }
        for x in &c.series {
            v.push(SearchItem {
                id: x.series_id,
                name: x.name.clone(),
                kind: ItemKind::Series,
            });
        }
        v
    }

    pub fn search(&self, query: &str) -> Vec<SearchItem> {
        let items = self.search_items();
        rank(query, &items).into_iter().cloned().collect()
    }

    pub fn portal(&self) -> &Arc<dyn Portal> {
        &self.portal
    }

    /// Look up a movie's container extension by stream_id; default "mkv".
    pub fn movie_extension(&self, stream_id: i64) -> String {
        self.inner
            .read()
            .as_ref()
            .and_then(|c| c.movies.iter().find(|m| m.stream_id == stream_id))
            .and_then(|m| m.container_extension.clone())
            .unwrap_or_else(|| "mkv".to_owned())
    }
}
