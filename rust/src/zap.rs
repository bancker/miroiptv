//! Zap pacing. Every load opens a portal session, the portal allows one per
//! account and answers bursts with HTTP 503 - after which even the channel
//! the user stops on starts slowly. A held arrow key repeats ~30x a second.

use std::time::{Duration, Instant};

/// Debounce with a leading edge: a lone press zaps at once; presses that
/// follow each other within `settle`, or a held key's repeats, only move the
/// target, and the last one zaps once the user pauses.
pub struct ZapPacer<T> {
    settle: Duration,
    last_press: Option<Instant>,
    pending: Option<(T, Instant)>,
}

impl<T> ZapPacer<T> {
    pub fn new(settle: Duration) -> Self {
        Self {
            settle,
            last_press: None,
            pending: None,
        }
    }

    /// A zap press towards `target`: returns it when it should load now.
    pub fn press(&mut self, now: Instant, target: T, repeat: bool) -> Option<T> {
        let flipping = repeat
            || self
                .last_press
                .is_some_and(|last| now.saturating_duration_since(last) < self.settle);
        self.last_press = Some(now);
        if flipping {
            self.pending = Some((target, now + self.settle));
            None
        } else {
            self.pending = None;
            Some(target)
        }
    }

    /// The target the user settled on, once they paused long enough.
    pub fn due(&mut self, now: Instant) -> Option<T> {
        match &self.pending {
            Some((_, due)) if now >= *due => self.pending.take().map(|(target, _)| target),
            _ => None,
        }
    }

    /// The target of the zap that waits for the user to pause.
    pub fn waiting(&self) -> Option<&T> {
        self.pending.as_ref().map(|(target, _)| target)
    }

    /// Forget a waiting zap: something else is being loaded.
    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_lone_press_zaps_at_once() {
        let t0 = Instant::now();
        let mut p = ZapPacer::new(ms(350));
        assert_eq!(p.press(t0, "A", false), Some("A"));
        assert_eq!(p.due(t0 + ms(1000)), None);
    }

    #[test]
    fn quick_presses_zap_only_to_the_last_once_the_user_pauses() {
        let t0 = Instant::now();
        let mut p = ZapPacer::new(ms(350));
        assert_eq!(p.press(t0, "A", false), Some("A"));
        assert_eq!(p.press(t0 + ms(150), "B", false), None);
        assert_eq!(p.press(t0 + ms(300), "C", false), None);
        assert_eq!(
            p.waiting(),
            Some(&"C"),
            "the next press continues from here"
        );
        assert_eq!(p.due(t0 + ms(600)), None, "still flipping");
        assert_eq!(p.due(t0 + ms(650)), Some("C"));
        assert_eq!(p.due(t0 + ms(700)), None, "zapped once");
        assert_eq!(p.waiting(), None);
    }

    #[test]
    fn a_held_key_only_zaps_where_it_is_released() {
        let t0 = Instant::now();
        let mut p = ZapPacer::new(ms(350));
        assert_eq!(p.press(t0, "A", false), Some("A"));
        // The keyboard's repeat delay can exceed the settle time.
        assert_eq!(p.press(t0 + ms(500), "B", true), None);
        assert_eq!(p.press(t0 + ms(533), "C", true), None);
        assert_eq!(p.due(t0 + ms(882)), None);
        assert_eq!(p.due(t0 + ms(883)), Some("C"));
    }

    #[test]
    fn a_press_after_a_pause_zaps_at_once_again() {
        let t0 = Instant::now();
        let mut p = ZapPacer::new(ms(350));
        assert_eq!(p.press(t0, "A", false), Some("A"));
        assert_eq!(p.press(t0 + ms(400), "B", false), Some("B"));
    }

    #[test]
    fn cancel_forgets_a_waiting_zap() {
        let t0 = Instant::now();
        let mut p = ZapPacer::new(ms(350));
        p.press(t0, "A", false);
        p.press(t0 + ms(100), "B", false);
        p.cancel();
        assert_eq!(p.waiting(), None);
        assert_eq!(p.due(t0 + ms(1000)), None);
    }
}
