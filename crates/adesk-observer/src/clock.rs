//! Monotonic clock that bridges the compositor's `ts_ms` domain and tokio timers.
//!
//! Every [`adesk_core::RuntimeEvent`] carries `ts_ms` = monotonic milliseconds since
//! *compositor* start. The observer is created slightly later and has its own
//! origin, so it keeps an anchor `(ts_ms, tokio::time::Instant)` that is advanced
//! forward whenever an event or snapshot carries a timestamp ahead of the current
//! estimate:
//!
//! ```text
//! now_ms()        = anchor_ts_ms + (Instant::now() - anchor_instant).as_millis()
//! observe_ts(t)   = if t > now_ms() { anchor = (t, Instant::now()) }   // never backwards
//! deadline(t)     = anchor_instant + (t - now_ms())                    // saturating
//! ```
//!
//! Consequences that the waiters rely on:
//!
//! - Deadlines are expressed in the event `ts_ms` domain, so quiet detection
//!   (`now - max(anchor_ts, last counted commit ts) >= quiet_ms`, see
//!   `docs/architecture.md` §6) uses one clock end to end.
//! - Under `tokio::time::pause()` both `Instant::now()` and `elapsed()` are frozen,
//!   so `now_ms()` is exactly the last observed event timestamp — tests are
//!   deterministic without real sleeps.
//! - The clock never moves backwards, so a stale or replayed event cannot make a
//!   quiet window un-quiet retroactively.

use std::time::Duration;

/// Internal clock; owned by the service behind its own mutex (see `service.rs`).
#[derive(Debug)]
pub(crate) struct Clock {
    anchor_ts_ms: u64,
    anchor_instant: tokio::time::Instant,
}

impl Clock {
    /// Creates a clock anchored at `ts_ms = 0` and the current tokio instant.
    pub(crate) fn new() -> Self {
        Self {
            anchor_ts_ms: 0,
            anchor_instant: tokio::time::Instant::now(),
        }
    }

    /// Current time in the `ts_ms` domain (monotonic, never decreasing).
    pub(crate) fn now_ms(&self) -> u64 {
        // `as_millis()` truncates, so the estimate is always ≤ the true elapsed
        // time and never over-reports. The anchor only ever moves forward, so the
        // result is monotonic; both conversions saturate instead of wrapping.
        let elapsed_ms =
            u64::try_from(self.anchor_instant.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.anchor_ts_ms.saturating_add(elapsed_ms)
    }

    /// Advances the anchor when `ts_ms` is ahead of the current estimate.
    ///
    /// Called for every event and snapshot `ts_ms`. Never moves the anchor
    /// backwards, so out-of-order or replayed timestamps are ignored.
    pub(crate) fn observe_ts(&mut self, ts_ms: u64) {
        if ts_ms > self.now_ms() {
            self.anchor_ts_ms = ts_ms;
            self.anchor_instant = tokio::time::Instant::now();
        }
    }

    /// Tokio instant at which `ts_ms` is reached (already-past deadlines saturate
    /// to `Instant::now()`).
    ///
    /// `tokio::time::sleep_until(self.deadline(ts))` is the *only* way the observer
    /// waits for time — never a polling loop.
    pub(crate) fn deadline(&self, ts_ms: u64) -> tokio::time::Instant {
        // `saturating_sub` makes an already-past `ts_ms` a zero-length offset, so
        // the deadline lands at (or before) now and `sleep_until` returns at once.
        self.anchor_instant + Duration::from_millis(ts_ms.saturating_sub(self.now_ms()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The observer's own origin is `ts_ms = 0`; it only learns compositor time
    /// through events (`observe_ts`) or elapsed tokio time.
    #[test]
    fn new_anchors_at_zero() {
        assert_eq!(Clock::new().now_ms(), 0);
    }

    /// Time in the event domain moves exactly as fast as the tokio clock.
    #[tokio::test(start_paused = true)]
    async fn now_ms_advances_with_the_tokio_clock() {
        let clock = Clock::new();
        assert_eq!(clock.now_ms(), 0);

        tokio::time::advance(Duration::from_millis(150)).await;
        assert_eq!(clock.now_ms(), 150);

        tokio::time::advance(Duration::from_millis(900)).await;
        assert_eq!(clock.now_ms(), 1_050);
    }

    /// An event timestamp ahead of the estimate is adopted immediately — the
    /// observer never waits for wall-clock time to catch up with compositor time.
    #[tokio::test(start_paused = true)]
    async fn observe_ts_re_anchors_forward() {
        let mut clock = Clock::new();

        clock.observe_ts(1_000);
        assert_eq!(clock.now_ms(), 1_000);

        tokio::time::advance(Duration::from_millis(40)).await;
        assert_eq!(clock.now_ms(), 1_040);

        clock.observe_ts(5_000);
        assert_eq!(clock.now_ms(), 5_000);
    }

    /// Stale, replayed and equal timestamps must never move the clock backwards —
    /// otherwise a late event could make an already-quiet window un-quiet.
    #[tokio::test(start_paused = true)]
    async fn observe_ts_ignores_stale_timestamps() {
        let mut clock = Clock::new();
        clock.observe_ts(2_000);

        // Out of order (replayed) event.
        clock.observe_ts(1_000);
        assert_eq!(clock.now_ms(), 2_000);

        // Equal timestamp: nothing to re-anchor.
        clock.observe_ts(2_000);
        assert_eq!(clock.now_ms(), 2_000);

        tokio::time::advance(Duration::from_millis(25)).await;

        // A timestamp already behind the current estimate is ignored as well.
        clock.observe_ts(2_010);
        assert_eq!(clock.now_ms(), 2_025);
    }

    /// Under paused time the clock freezes at the last observed event timestamp;
    /// this is what makes the waiter tests deterministic without real sleeps.
    #[tokio::test(start_paused = true)]
    async fn clock_freezes_at_last_observed_ts() {
        let mut clock = Clock::new();
        clock.observe_ts(7_500);

        for _ in 0..3 {
            assert_eq!(clock.now_ms(), 7_500);
        }
    }

    /// Deadlines live in the event domain: the offset is relative to the anchor,
    /// and an already-past timestamp saturates to "now" instead of going negative.
    #[tokio::test(start_paused = true)]
    async fn deadline_is_anchored_and_saturates_for_past_ts() {
        let mut clock = Clock::new();
        clock.observe_ts(1_000);

        assert_eq!(
            clock.deadline(1_250),
            tokio::time::Instant::now() + Duration::from_millis(250)
        );

        let past = clock.deadline(0);
        assert!(past <= tokio::time::Instant::now());

        // Sleeping on a saturated deadline returns at once and does not move time.
        tokio::time::sleep_until(past).await;
        assert_eq!(clock.now_ms(), 1_000);
    }

    /// `sleep_until(deadline(t))` is the only way the observer waits for time: it
    /// must resolve exactly when the event domain reaches `t`.
    #[tokio::test(start_paused = true)]
    async fn sleep_until_deadline_reaches_the_target_ts() {
        let mut clock = Clock::new();
        clock.observe_ts(1_000);

        tokio::time::sleep_until(clock.deadline(1_250)).await;
        assert_eq!(clock.now_ms(), 1_250);
    }

    /// A bogus timestamp saturates instead of wrapping; the clock stays usable.
    #[tokio::test(start_paused = true)]
    async fn extreme_timestamps_saturate_instead_of_wrapping() {
        let mut clock = Clock::new();
        clock.observe_ts(u64::MAX);
        assert_eq!(clock.now_ms(), u64::MAX);

        // Elapsed time can never push the estimate past `u64::MAX`.
        tokio::time::advance(Duration::from_millis(1_000)).await;
        assert_eq!(clock.now_ms(), u64::MAX);

        // Everything is now "in the past", so the anchor stays where it is.
        clock.observe_ts(1);
        assert_eq!(clock.now_ms(), u64::MAX);

        // Deadlines still yield a sane (saturated) instant, never a panic.
        assert!(clock.deadline(u64::MAX) <= tokio::time::Instant::now());
    }
}
