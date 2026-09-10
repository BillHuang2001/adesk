//! Deadline-bounded waiting helpers.
//!
//! Every helper in this module has an explicit timeout: a hung runtime makes a test
//! *fail* with [`crate::TestkitError::Timeout`], never block forever. There is no
//! unbounded sleep or wait anywhere in the harness — this module is the only place the
//! testkit sleeps.

use std::time::Duration;

use crate::error::{Result, TestkitError};

/// Default interval between condition re-checks (`10 ms`).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Waits until `cond` returns `true`, polling every [`DEFAULT_POLL_INTERVAL`].
///
/// `cond` is evaluated once immediately; `what` names the condition in the timeout error.
pub async fn wait_until<F>(timeout: Duration, what: &'static str, mut cond: F) -> Result<()>
where
    F: FnMut() -> bool,
{
    if cond() {
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(timeout_error(what, timeout));
        }
        tokio::time::sleep((deadline - now).min(DEFAULT_POLL_INTERVAL)).await;
        if cond() {
            return Ok(());
        }
    }
}

/// Synchronous variant of [`wait_until`] for `Drop` paths and non-async tests.
///
/// Blocks the calling thread in `min(interval, remaining)` increments; it must only be
/// used off the async executor.
pub fn block_until<F>(timeout: Duration, what: &'static str, mut cond: F) -> Result<()>
where
    F: FnMut() -> bool,
{
    if cond() {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(timeout_error(what, timeout));
        }
        std::thread::sleep((deadline - now).min(DEFAULT_POLL_INTERVAL));
        if cond() {
            return Ok(());
        }
    }
}

/// Builds the canonical timeout error for `what`.
pub(crate) fn timeout_error(what: &'static str, timeout: Duration) -> TestkitError {
    TestkitError::Timeout { what, timeout }
}
