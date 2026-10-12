//! Fetching the guide's EPG in the background, gently: the portal throttles
//! bursts, so the guide asks for a channel or two at a time and rests after a
//! failure.

use super::store::EpgStore;
use crate::epg::Epg;
use crate::portal::Portal;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A fetch that finished, on its way to the UI thread.
#[derive(Debug)]
pub struct Fetched {
    pub archive_id: i64,
    /// The error's text on failure (credentials masked, see `PortalError`).
    pub result: Result<Epg, String>,
}

/// After this many failures of one channel it is left alone for the rest of
/// the session (or until another portal).
const MAX_FAILURES: u8 = 3;

/// How soon `tick` looks through the channels again after a look that found
/// nothing (more) to start.
const WALK_AGAIN: Duration = Duration::from_secs(1);

/// How long to rest after this many failures in a row: 5 s, 10 s, then 30 s.
fn rest_after(failures_in_a_row: u32) -> Duration {
    match failures_in_a_row {
        0 => Duration::ZERO,
        1 => Duration::from_secs(5),
        2 => Duration::from_secs(10),
        _ => Duration::from_secs(30),
    }
}

/// Starts the guide's EPG fetches, a few at a time, and takes their results in.
pub struct EpgFetcher {
    portal: Arc<dyn Portal>,
    /// Where finished fetches land. A new one after `reset`: the old portal's
    /// fetches finish into a slot nobody reads any more.
    done: Arc<Mutex<Vec<Fetched>>>,
    in_flight: HashSet<i64>,
    failures: HashMap<i64, u8>,
    failures_in_a_row: u32,
    /// No new fetch before this moment (resting after a failure).
    not_before: Option<Instant>,
    /// No look through the channels before this moment: the last look found
    /// nothing (more) to start. `tick` runs every frame; this keeps it from
    /// walking every channel each time once the guide is fresh.
    next_walk: Option<Instant>,
}

impl EpgFetcher {
    pub fn new(portal: Arc<dyn Portal>) -> Self {
        Self {
            portal,
            done: Arc::new(Mutex::new(Vec::new())),
            in_flight: HashSet::new(),
            failures: HashMap::new(),
            failures_in_a_row: 0,
            not_before: None,
            next_walk: None,
        }
    }

    /// Another portal took over: start afresh.
    pub fn reset(&mut self, portal: Arc<dyn Portal>) {
        *self = Self::new(portal);
    }

    /// Catch-up ids being fetched now.
    pub fn in_flight(&self) -> &HashSet<i64> {
        &self.in_flight
    }

    /// Starts fetches while fewer than `limit` run, taking channels (catch-up
    /// ids) in `order` and skipping the ones in flight, the ones the store has
    /// fresh and the ones that failed too often. Nothing while resting after a
    /// failure.
    ///
    /// Called every frame, so it returns at once when nothing can start: with
    /// all `limit` slots taken, while resting, and for `WALK_AGAIN` after a
    /// look through `order` that left a slot free (every channel was in flight,
    /// fresh or given up on). `order` is not touched then.
    pub fn tick(
        &mut self,
        store: &EpgStore,
        order: impl Iterator<Item = i64>,
        limit: usize,
        now: Instant,
        utc_now: DateTime<Utc>,
    ) {
        if self.in_flight.len() >= limit
            || self.not_before.is_some_and(|t| now < t)
            || self.next_walk.is_some_and(|t| now < t)
        {
            return;
        }
        let mut free = limit - self.in_flight.len();
        for id in order {
            if free == 0 {
                break;
            }
            if self.in_flight.contains(&id)
                || self.failures.get(&id).is_some_and(|&n| n >= MAX_FAILURES)
                || !store.needs_fetch(id, utc_now)
            {
                continue;
            }
            self.start(id);
            free -= 1;
        }
        // Every channel was taken or skipped and a slot is still free: nothing
        // more can start until one turns stale, so look again in a second.
        self.next_walk = (free > 0).then(|| now + WALK_AGAIN);
    }

    fn start(&mut self, archive_id: i64) {
        self.in_flight.insert(archive_id);
        let (portal, done) = (self.portal.clone(), self.done.clone());
        tokio::spawn(async move {
            let result = fetch_one(portal.as_ref(), archive_id).await;
            done.lock().push(Fetched { archive_id, result });
        });
    }

    /// The fetches that finished since the last call. A failure counts for its
    /// channel and makes the fetcher rest from `now` (5, 10, then 30 s).
    pub fn drain(&mut self, now: Instant) -> Vec<Fetched> {
        let finished = std::mem::take(&mut *self.done.lock());
        for f in &finished {
            self.in_flight.remove(&f.archive_id);
            match &f.result {
                Ok(_) => self.failures_in_a_row = 0,
                Err(e) => {
                    *self.failures.entry(f.archive_id).or_insert(0) += 1;
                    self.failures_in_a_row += 1;
                    let rest = rest_after(self.failures_in_a_row);
                    self.not_before = Some(now + rest);
                    tracing::warn!(
                        "guide: epg fetch failed for sid={} ({}) - resting {} s",
                        f.archive_id,
                        e,
                        rest.as_secs()
                    );
                }
            }
        }
        finished
    }
}

/// One channel's day table. An empty one falls back to the short EPG (some
/// portals fill only that); a failed one is a failure: the portal is busy or
/// broken, and asking it again at once would only add to that.
async fn fetch_one(portal: &dyn Portal, archive_id: i64) -> Result<Epg, String> {
    let day = portal
        .fetch_day_epg(archive_id)
        .await
        .map_err(|e| e.to_string())?;
    if !day.entries().is_empty() {
        return Ok(day);
    }
    Ok(portal.fetch_epg(archive_id).await.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epg::EpgEntry;
    use crate::guide::store::ChannelEpg;
    use crate::portal::{Catalog, Episode, PortalError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A portal whose day tables answer what `answer` says for a stream id
    /// (0: a programme, 503: busy, 1: an empty table), each only once the gate
    /// lets it through.
    struct GatedPortal {
        gate: tokio::sync::Semaphore,
        asked: Mutex<Vec<i64>>,
        short_asked: AtomicUsize,
        answer: fn(i64) -> u16,
    }

    impl GatedPortal {
        fn new(answer: fn(i64) -> u16) -> Arc<Self> {
            Arc::new(Self {
                gate: tokio::sync::Semaphore::new(0),
                asked: Mutex::new(Vec::new()),
                short_asked: AtomicUsize::new(0),
                answer,
            })
        }

        fn release(&self, n: usize) {
            self.gate.add_permits(n);
        }

        fn asked(&self) -> Vec<i64> {
            self.asked.lock().clone()
        }
    }

    /// One programme from now until 30 hours ahead: fresh in the store.
    fn programme() -> Epg {
        let now = Utc::now();
        Epg::new(vec![EpgEntry {
            title: "Nieuws".into(),
            start: now,
            end: now + chrono::Duration::hours(30),
            has_archive: None,
        }])
    }

    #[async_trait::async_trait]
    impl Portal for GatedPortal {
        async fn fetch_catalog(&self) -> Result<Catalog, PortalError> {
            Ok(Catalog::default())
        }
        async fn fetch_epg(&self, _: i64) -> Result<Epg, PortalError> {
            self.short_asked.fetch_add(1, Ordering::SeqCst);
            Ok(programme())
        }
        async fn fetch_day_epg(&self, id: i64) -> Result<Epg, PortalError> {
            self.asked.lock().push(id);
            let _pass = self.gate.acquire().await;
            match (self.answer)(id) {
                503 => Err(PortalError::Status(503)),
                1 => Ok(Epg::default()),
                _ => Ok(programme()),
            }
        }
        async fn fetch_series_episodes(&self, _: i64) -> Result<Vec<Episode>, PortalError> {
            Ok(Vec::new())
        }
        fn live_stream_url(&self, id: i64) -> String {
            format!("live/{id}")
        }
        fn live_stream_id(&self, _: &str) -> Option<i64> {
            None
        }
        fn movie_stream_url(&self, id: i64, _: &str) -> String {
            format!("movie/{id}")
        }
        fn series_stream_url(&self, id: &str, _: &str) -> String {
            format!("series/{id}")
        }
        fn catchup_url(&self, id: i64, _: DateTime<Utc>, _: u32) -> String {
            format!("catchup/{id}")
        }
    }

    /// Waits (2 s at most) until `n` fetches came back, taking them in at `now`.
    async fn drained(f: &mut EpgFetcher, n: usize, now: Instant) -> Vec<Fetched> {
        let mut got = Vec::new();
        for _ in 0..400 {
            got.extend(f.drain(now));
            if got.len() >= n {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("only {} of {n} fetches came back", got.len());
    }

    #[tokio::test]
    async fn it_fetches_in_order_and_never_more_than_the_limit_at_a_time() {
        let portal = GatedPortal::new(|_| 0);
        let mut f = EpgFetcher::new(portal.clone());
        let store = EpgStore::default();
        let (now, utc) = (Instant::now(), Utc::now());
        f.tick(&store, [5, 6, 7, 8].into_iter(), 2, now, utc);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(portal.asked(), [5, 6]);
        f.tick(&store, [5, 6, 7, 8].into_iter(), 2, now, utc);
        assert_eq!(f.in_flight().len(), 2, "the limit holds");
        portal.release(2);
        let done = drained(&mut f, 2, now).await;
        assert!(done.iter().all(|d| d.result.is_ok()));
        assert!(f.in_flight().is_empty());
    }

    #[tokio::test]
    async fn nothing_starts_while_a_stream_starts() {
        let portal = GatedPortal::new(|_| 0);
        let mut f = EpgFetcher::new(portal.clone());
        f.tick(
            &EpgStore::default(),
            [5].into_iter(),
            0,
            Instant::now(),
            Utc::now(),
        );
        assert!(f.in_flight().is_empty());
    }

    #[tokio::test]
    async fn a_failed_fetch_backs_off_and_nothing_is_stored() {
        let portal = GatedPortal::new(|_| 503);
        portal.release(100);
        let mut f = EpgFetcher::new(portal.clone());
        let store = EpgStore::default();
        let (t0, utc) = (Instant::now(), Utc::now());
        f.tick(&store, [5].into_iter(), 2, t0, utc);
        let done = drained(&mut f, 1, t0).await;
        assert!(
            done[0].result.is_err(),
            "a busy day table is no empty guide"
        );
        f.tick(&store, [5].into_iter(), 2, t0 + Duration::from_secs(4), utc);
        assert!(
            f.in_flight().is_empty(),
            "5 s of rest after the first failure"
        );
        let t1 = t0 + Duration::from_secs(6);
        f.tick(&store, [5].into_iter(), 2, t1, utc);
        assert_eq!(f.in_flight().len(), 1, "then it tries again");
        drained(&mut f, 1, t1).await;
        f.tick(&store, [5].into_iter(), 2, t1 + Duration::from_secs(9), utc);
        assert!(
            f.in_flight().is_empty(),
            "10 s after the second failure in a row"
        );
        assert_eq!(
            portal.short_asked.load(Ordering::SeqCst),
            0,
            "a busy portal is not asked for the short EPG on top"
        );
    }

    #[tokio::test]
    async fn a_channel_that_keeps_failing_is_left_alone() {
        let portal = GatedPortal::new(|_| 503);
        portal.release(100);
        let mut f = EpgFetcher::new(portal.clone());
        let store = EpgStore::default();
        let mut now = Instant::now();
        for _ in 0..3 {
            f.tick(&store, [5].into_iter(), 2, now, Utc::now());
            drained(&mut f, 1, now).await;
            now += Duration::from_secs(60);
        }
        f.tick(&store, [5].into_iter(), 2, now, Utc::now());
        assert!(f.in_flight().is_empty(), "three failures: left alone");
        assert_eq!(portal.asked().len(), 3);
    }

    #[tokio::test]
    async fn an_empty_day_table_falls_back_to_the_short_epg() {
        let portal = GatedPortal::new(|_| 1);
        portal.release(1);
        let mut f = EpgFetcher::new(portal.clone());
        let now = Instant::now();
        f.tick(&EpgStore::default(), [5].into_iter(), 1, now, Utc::now());
        let done = drained(&mut f, 1, now).await;
        assert_eq!(done[0].result.as_ref().unwrap().entries().len(), 1);
        assert_eq!(portal.short_asked.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fresh_channels_are_left_alone() {
        let portal = GatedPortal::new(|_| 0);
        let mut f = EpgFetcher::new(portal.clone());
        let mut store = EpgStore::default();
        store.insert(5, ChannelEpg::new(programme(), Utc::now()));
        f.tick(&store, [5, 6].into_iter(), 2, Instant::now(), Utc::now());
        assert_eq!(f.in_flight().iter().copied().collect::<Vec<_>>(), [6]);
    }

    #[tokio::test]
    async fn after_a_reset_the_old_portals_answers_are_not_taken_in() {
        let old = GatedPortal::new(|_| 0);
        let mut f = EpgFetcher::new(old.clone());
        let now = Instant::now();
        f.tick(&EpgStore::default(), [5].into_iter(), 1, now, Utc::now());
        f.reset(GatedPortal::new(|_| 0));
        old.release(1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(f.drain(now).is_empty());
        assert!(f.in_flight().is_empty());
    }

    #[tokio::test]
    async fn with_every_channel_fresh_it_looks_again_only_a_second_later() {
        let portal = GatedPortal::new(|_| 0);
        let mut f = EpgFetcher::new(portal.clone());
        let mut store = EpgStore::default();
        store.insert(5, ChannelEpg::new(programme(), Utc::now()));
        store.insert(6, ChannelEpg::new(programme(), Utc::now()));
        let pulled = std::cell::Cell::new(0);
        let order = || [5, 6].into_iter().inspect(|_| pulled.set(pulled.get() + 1));
        let t0 = Instant::now();
        f.tick(&store, order(), 2, t0, Utc::now());
        assert_eq!(pulled.get(), 2, "the first tick looks at every channel");
        f.tick(
            &store,
            order(),
            2,
            t0 + Duration::from_millis(500),
            Utc::now(),
        );
        assert_eq!(pulled.get(), 2, "within the second it does not look again");
        f.tick(&store, order(), 2, t0 + Duration::from_secs(1), Utc::now());
        assert_eq!(pulled.get(), 4, "a second later it looks again");
        assert!(f.in_flight().is_empty());
    }

    #[tokio::test]
    async fn a_slot_that_frees_up_is_filled_at_once_not_a_second_later() {
        let portal = GatedPortal::new(|_| 0);
        portal.release(100);
        let mut f = EpgFetcher::new(portal.clone());
        let mut store = EpgStore::default();
        let (now, utc) = (Instant::now(), Utc::now());
        f.tick(&store, [5, 6].into_iter(), 1, now, utc);
        assert_eq!(f.in_flight().iter().copied().collect::<Vec<_>>(), [5]);
        drained(&mut f, 1, now).await;
        store.insert(5, ChannelEpg::new(programme(), utc));
        f.tick(&store, [5, 6].into_iter(), 1, now, utc);
        assert_eq!(
            f.in_flight().iter().copied().collect::<Vec<_>>(),
            [6],
            "a full limit is no empty look: the next fetch does not wait for it"
        );
    }
}
