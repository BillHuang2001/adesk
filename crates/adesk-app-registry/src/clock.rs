//! Monotonic time source for launch records and correlation deadlines.
//!
//! Timestamps follow the workspace convention: monotonic milliseconds since the
//! runtime started, never wall clock (`./CONTEXT.md` → Timestamps). The registry
//! and the [`crate::Correlator`] MUST share one clock instance so
//! [`crate::LaunchRecord::started_at_ms`] is comparable with correlation deadlines.

/// Milliseconds since the runtime started.
///
/// Implementations must be monotonic (never decrease) and `Send + Sync` because
/// the registry is shared across the server's tasks.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Monotonic milliseconds since the runtime start.
    fn now_ms(&self) -> u64;
}

/// Production clock backed by [`std::time::Instant`].
///
/// The zero point is the moment the clock is constructed; the server creates one
/// instance at startup and shares it as `Arc<dyn Clock>`.
#[derive(Debug)]
pub struct MonotonicClock {
    start: std::time::Instant,
}

impl MonotonicClock {
    /// Creates a clock whose zero is the instant of this call.
    pub fn new() -> MonotonicClock {
        MonotonicClock {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn now_ms_never_goes_backwards() {
        let clock = MonotonicClock::new();
        let mut previous = clock.now_ms();
        for _ in 0..1_000 {
            let now = clock.now_ms();
            assert!(now >= previous, "clock went backwards: {now} < {previous}");
            previous = now;
        }
    }

    #[test]
    fn zero_point_is_construction_time() {
        let before = std::time::Instant::now();
        let clock = MonotonicClock::new();
        let now = clock.now_ms();
        let elapsed = before.elapsed().as_millis();

        assert!(
            u128::from(now) <= elapsed,
            "now_ms {now} exceeds the {elapsed} ms since before construction"
        );
    }

    #[test]
    fn default_behaves_like_new() {
        let clock = MonotonicClock::default();
        assert!(clock.now_ms() < 3_600_000, "not a since-construction value");
    }

    #[test]
    fn works_as_a_shared_trait_object() {
        fn require_send_sync_debug<T: Send + Sync + std::fmt::Debug>() {}
        require_send_sync_debug::<MonotonicClock>();

        let clock: Arc<dyn Clock> = Arc::new(MonotonicClock::new());
        assert!(clock.now_ms() < 3_600_000);
        assert!(format!("{clock:?}").contains("MonotonicClock"));
    }
}
