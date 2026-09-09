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
//!   (`now - last_commit_at >= quiet_ms`) uses one clock end to end.
//! - Under `tokio::time::pause()` both `Instant::now()` and `elapsed()` are frozen,
//!   so `now_ms()` is exactly the last observed event timestamp — tests are
//!   deterministic without real sleeps.
//! - The clock never moves backwards, so a stale or replayed event cannot make a
//!   quiet window un-quiet retroactively.

// Phase 1 architecture skeleton: method bodies are `todo!()`, so the fields they
// will read look unused. Remove this allow together with the last `todo!()` in
// this file (see `CONTEXT.md` → Status).
#![allow(dead_code)]

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
        let _ = ();
        todo!("Phase 2: anchor at ts_ms = 0 with tokio::time::Instant::now()")
    }

    /// Current time in the `ts_ms` domain (monotonic, never decreasing).
    pub(crate) fn now_ms(&self) -> u64 {
        let _ = self;
        todo!("Phase 2: anchor_ts_ms + anchor_instant.elapsed().as_millis()")
    }

    /// Advances the anchor when `ts_ms` is ahead of the current estimate.
    ///
    /// Called for every event and snapshot `ts_ms`. Never moves the anchor
    /// backwards, so out-of-order or replayed timestamps are ignored.
    pub(crate) fn observe_ts(&mut self, ts_ms: u64) {
        let _ = (self, ts_ms);
        todo!("Phase 2: re-anchor forward only")
    }

    /// Tokio instant at which `ts_ms` is reached (already-past deadlines saturate
    /// to `Instant::now()`).
    ///
    /// `tokio::time::sleep_until(self.deadline(ts))` is the *only* way the observer
    /// waits for time — never a polling loop.
    pub(crate) fn deadline(&self, ts_ms: u64) -> tokio::time::Instant {
        let _ = (self, ts_ms);
        todo!("Phase 2: anchor_instant + Duration::from_millis(ts_ms.saturating_sub(now_ms()))")
    }
}

/// Convenience: the duration from now until `ts_ms` in the clock domain.
#[allow(dead_code)]
pub(crate) fn until(clock: &Clock, ts_ms: u64) -> Duration {
    Duration::from_millis(ts_ms.saturating_sub(clock.now_ms()))
}
