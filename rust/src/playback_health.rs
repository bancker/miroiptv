//! Per-stream playback health: detects when the picture freezes on a cache
//! underrun (mpv's `paused-for-cache`) and tracks how often / how long.
//!
//! Pure logic, no mpv/egui dependencies: the player thread feeds it
//! `paused-for-cache` transitions and the first-frame signal, the debug HUD
//! reads the counters. Counters are per stream — call [`PlaybackHealth::reset`]
//! when a new stream lands. Startup/zap buffering (before the first frame) is
//! shown live but not counted; counting arms on [`PlaybackHealth::mark_started`].

use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct PlaybackHealth {
    /// Raw live `paused-for-cache` flag (true == picture stalled on cache).
    buffering: bool,
    /// First frame of the current stream displayed yet? Gates counting so
    /// startup/zap buffering isn't recorded as a freeze.
    started: bool,
    /// Start instant of the freeze currently in progress — `Some` only for a
    /// *counted* freeze (i.e. one that began after `started`).
    current_freeze_start: Option<Instant>,
    freeze_count: u32,
    total_frozen: Duration,
    last_freeze: Option<Duration>,
}

impl PlaybackHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// New stream landed: clear counters and re-arm startup suppression.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// First frame of the current stream displayed (mpv `PLAYBACK_RESTART`).
    /// Idempotent. Until called, buffering counts as startup and is not tallied.
    pub fn mark_started(&mut self) {
        self.started = true;
    }

    /// Feed a `paused-for-cache` transition observed at `now`.
    pub fn on_buffering_changed(&mut self, now: Instant, on: bool) {
        if on == self.buffering {
            return; // no transition — idempotent
        }
        self.buffering = on;
        if on {
            // Count only freezes that begin after the first frame; startup/zap
            // buffering (started == false) is shown live but not tallied.
            if self.started {
                self.current_freeze_start = Some(now);
            }
        } else if let Some(start) = self.current_freeze_start.take() {
            let d = now.saturating_duration_since(start);
            self.freeze_count += 1;
            self.total_frozen += d;
            self.last_freeze = Some(d);
        }
    }

    /// Live duration of the freeze currently in progress (counted freezes
    /// only), else `None`.
    pub fn elapsed(&self, now: Instant) -> Option<Duration> {
        self.current_freeze_start
            .map(|s| now.saturating_duration_since(s))
    }

    pub fn is_buffering(&self) -> bool {
        self.buffering
    }

    pub fn freeze_count(&self) -> u32 {
        self.freeze_count
    }

    pub fn total_frozen(&self) -> Duration {
        self.total_frozen
    }

    pub fn last_freeze(&self) -> Option<Duration> {
        self.last_freeze
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream past the startup-buffering window (first frame seen).
    fn started() -> PlaybackHealth {
        let mut h = PlaybackHealth::new();
        h.mark_started();
        h
    }

    #[test]
    fn single_freeze_records_count_duration_and_total() {
        let t0 = Instant::now();
        let mut h = started();
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(2), false);
        assert_eq!(h.freeze_count(), 1);
        assert_eq!(h.last_freeze(), Some(Duration::from_secs(2)));
        assert_eq!(h.total_frozen(), Duration::from_secs(2));
        assert!(!h.is_buffering());
    }

    #[test]
    fn multiple_freezes_accumulate() {
        let t0 = Instant::now();
        let mut h = started();
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(1), false); // 1s
        h.on_buffering_changed(t0 + Duration::from_secs(10), true);
        h.on_buffering_changed(t0 + Duration::from_secs(13), false); // 3s
        assert_eq!(h.freeze_count(), 2);
        assert_eq!(h.last_freeze(), Some(Duration::from_secs(3)));
        assert_eq!(h.total_frozen(), Duration::from_secs(4));
    }

    #[test]
    fn startup_buffering_is_not_counted() {
        // No mark_started(): buffering before the first frame is startup.
        let t0 = Instant::now();
        let mut h = PlaybackHealth::new();
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(5), false);
        assert_eq!(h.freeze_count(), 0);
        assert_eq!(h.total_frozen(), Duration::ZERO);
        assert_eq!(h.last_freeze(), None);
    }

    #[test]
    fn freeze_after_start_is_counted() {
        let t0 = Instant::now();
        let mut h = PlaybackHealth::new();
        // startup buffering -> not counted
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(2), false);
        // first frame shown
        h.mark_started();
        // genuine mid-stream freeze
        h.on_buffering_changed(t0 + Duration::from_secs(10), true);
        h.on_buffering_changed(t0 + Duration::from_secs(11), false);
        assert_eq!(h.freeze_count(), 1);
        assert_eq!(h.last_freeze(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn redundant_true_does_not_restart_freeze() {
        let t0 = Instant::now();
        let mut h = started();
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(1), true); // ignored
        h.on_buffering_changed(t0 + Duration::from_secs(3), false);
        assert_eq!(h.freeze_count(), 1);
        // duration measured from the first `true` (t0) -> 3s
        assert_eq!(h.last_freeze(), Some(Duration::from_secs(3)));
    }

    #[test]
    fn false_without_active_freeze_is_noop() {
        let t0 = Instant::now();
        let mut h = started();
        h.on_buffering_changed(t0, false);
        assert_eq!(h.freeze_count(), 0);
        assert!(!h.is_buffering());
    }

    #[test]
    fn elapsed_is_some_during_counted_freeze_else_none() {
        let t0 = Instant::now();
        let mut h = started();
        assert_eq!(h.elapsed(t0), None);
        h.on_buffering_changed(t0, true);
        assert_eq!(
            h.elapsed(t0 + Duration::from_secs(2)),
            Some(Duration::from_secs(2))
        );
        h.on_buffering_changed(t0 + Duration::from_secs(3), false);
        assert_eq!(h.elapsed(t0 + Duration::from_secs(4)), None);
    }

    #[test]
    fn startup_buffering_sets_is_buffering_but_no_elapsed() {
        let t0 = Instant::now();
        let mut h = PlaybackHealth::new(); // not started
        h.on_buffering_changed(t0, true);
        assert!(h.is_buffering()); // live truth: yes, buffering
        assert_eq!(h.elapsed(t0 + Duration::from_secs(1)), None); // but not a counted freeze
    }

    #[test]
    fn reset_zeroes_counters_and_disarms_started() {
        let t0 = Instant::now();
        let mut h = started();
        h.on_buffering_changed(t0, true);
        h.on_buffering_changed(t0 + Duration::from_secs(2), false);
        h.reset();
        assert_eq!(h.freeze_count(), 0);
        assert_eq!(h.total_frozen(), Duration::ZERO);
        assert_eq!(h.last_freeze(), None);
        assert!(!h.is_buffering());
        // started is false again -> buffering not counted until mark_started
        h.on_buffering_changed(t0 + Duration::from_secs(3), true);
        h.on_buffering_changed(t0 + Duration::from_secs(4), false);
        assert_eq!(h.freeze_count(), 0);
    }
}
