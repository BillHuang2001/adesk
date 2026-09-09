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
    // stub: the field is populated when `now_ms` is implemented.
    #[allow(dead_code)]
    start: std::time::Instant,
}

impl MonotonicClock {
    /// Creates a clock whose zero is the instant of this call.
    pub fn new() -> MonotonicClock {
        todo!("stub: implementation phase")
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        todo!("stub: implementation phase")
    }
}
