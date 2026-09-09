//! Compositor `RuntimeEvent` broadcast → observer, subscription fan-out and
//! inspection caches; `QueryState` resync after broadcast lag.
//!
//! One pump task per runtime subscribes to [`adesk_compositor::CompositorHandle::subscribe`]
//! and is the **only** writer of the observer's event history
//! (`docs/architecture.md` §2). On `RecvError::Lagged` it issues `QueryState`
//! and calls `ObserverService::resync` so missed events become *uncertainty*
//! rather than silent loss.

use adesk_core::RuntimeEvent;

use crate::context::ServerContext;
use crate::error::Result;

/// Spawns the event pump task for the runtime's lifetime.
///
/// Returns the join handle; it resolves when the broadcast channel closes
/// (compositor shutdown) or the shutdown token fires.
pub fn spawn(context: ServerContext) -> tokio::task::JoinHandle<()> {
    todo!()
}

/// Applies one runtime event: feeds the observer, updates the inspection cache
/// (windows, damage, commit, actions) and fans out to matching subscriptions.
///
/// Never blocks: the fan-out uses non-blocking sends and drops frames for
/// slow consumers.
pub fn handle_event(context: &ServerContext, event: &RuntimeEvent) {
    todo!()
}

/// Re-synchronizes the observer and the inspection cache after a broadcast lag.
///
/// Issues `QueryState` to the compositor, translates the snapshot
/// (`crate::translate::observer_snapshot`) and calls `ObserverService::resync`;
/// windows the snapshot introduced are marked uncertain until their next commit.
///
/// # Errors
///
/// Returns [`crate::ServerError::ShuttingDown`] when the compositor is gone.
pub async fn resync(context: &ServerContext) -> Result<()> {
    todo!()
}
