use crate::portal::{Catalog as PortalCatalog, LiveChannel, Portal};
use crate::search::{rank, ItemKind, SearchItem};
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{info, warn};

pub struct CatalogStore {
    inner: Arc<RwLock<Option<PortalCatalog>>>,
    portal: Arc<dyn Portal>,
}

impl CatalogStore {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            portal,
        }
    }

    /// Spawn an async fetch on the current tokio runtime. Non-blocking.
    pub fn spawn_fetch(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let portal = self.portal.clone();
        tokio::spawn(async move {
            match portal.fetch_catalog().await {
                Ok(c) => {
                    info!(
                        "catalog loaded: {} live, {} movies, {} series",
                        c.live.len(),
                        c.movies.len(),
                        c.series.len()
                    );
                    *inner.write() = Some(c);
                }
                Err(e) => warn!("catalog fetch failed: {}", e),
            }
        });
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
