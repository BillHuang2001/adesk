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
    let _ = (&mut cond, timeout, what);
    todo!("stub: implementation phase — immediate check, then poll, Timeout at deadline")
}

/// Waits until the future produced by `cond` resolves to `true`.
///
/// Unlike [`wait_until`], the condition is awaited directly (no polling interval); the
/// overall deadline still applies.
pub async fn wait_until_async<F, Fut>(timeout: Duration, what: &'static str, mut cond: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let _ = (&mut cond, timeout, what);
    todo!("stub: implementation phase — wrap the whole loop in tokio::time::timeout")
}

/// Polls `f` every `interval` until it yields `Some(value)`.
///
/// Returns [`TestkitError::Timeout`] at the deadline; `f` is evaluated once immediately.
pub async fn poll_until<T, F>(
    timeout: Duration,
    interval: Duration,
    what: &'static str,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Option<T>,
{
    let _ = (&mut f, timeout, interval, what);
    todo!("stub: implementation phase — immediate check, then sleep(interval) between polls")
}

/// Synchronous variant of [`wait_until`] for `Drop` paths and non-async tests.
///
/// Blocks the calling thread in `min(interval, remaining)` increments; it must only be
/// used off the async executor.
pub fn block_until<F>(timeout: Duration, what: &'static str, mut cond: F) -> Result<()>
where
    F: FnMut() -> bool,
{
    let _ = (&mut cond, timeout, what);
    todo!("stub: implementation phase — std::thread::sleep in bounded increments")
}

/// Builds the canonical timeout error for `what`.
// Not yet called: the Phase 1 waiters are `todo!()` stubs; Phase 2 uses this in
// `wait_until`, `poll_until` and `block_until`.
#[allow(dead_code)]
pub(crate) fn timeout_error(what: &'static str, timeout: Duration) -> TestkitError {
    TestkitError::Timeout { what, timeout }
}
