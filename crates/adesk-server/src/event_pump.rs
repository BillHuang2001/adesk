//! Compositor `RuntimeEvent` broadcast → observer, subscription fan-out and
//! inspection caches; `QueryState` resync after broadcast lag.
//!
//! One pump task per runtime subscribes to [`adesk_compositor::CompositorHandle::subscribe`]
//! and is the **only** writer of the observer's event history
//! (`docs/architecture.md` §2). On `RecvError::Lagged` it issues `QueryState`
//! and calls `ObserverService::resync` so missed events become *uncertainty*
//! rather than silent loss.

use adesk_compositor::RuntimeCommand;
use adesk_core::RuntimeEvent;
use adesk_inspector::CommitInfo;
use tokio::sync::{broadcast, oneshot};

use crate::context::ServerContext;
use crate::error::{Result, ServerError};

/// Spawns the event pump task for the runtime's lifetime.
///
/// Returns the join handle; it resolves when the broadcast channel closes
/// (compositor shutdown) or the shutdown token fires.
pub fn spawn(context: ServerContext) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = context.compositor.subscribe();
        tracing::debug!("event pump started");
        loop {
            tokio::select! {
                biased;
                () = context.shutdown.cancelled() => break,
                received = events.recv() => match received {
                    Ok(event) => handle_event(&context, &event),
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "runtime event broadcast lagged; resynchronizing");
                        if let Err(error) = resync(&context).await {
                            tracing::warn!(%error, "resync after broadcast lag failed");
                            if context.shutdown.is_shutting_down() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        tracing::debug!("event pump stopped");
    })
}

/// Applies one runtime event: feeds the observer, updates the inspection cache
/// (windows, damage, commit, actions) and fans out to matching subscriptions.
///
/// Never blocks: the fan-out uses non-blocking sends and drops frames for
/// slow consumers.
pub fn handle_event(context: &ServerContext, event: &RuntimeEvent) {
    // The observer is the only writer of the event history; feed it first so
    // any waiter woken by the fan-out already sees the event.
    context.observer.handle_event(event);
    update_inspection(context, event);
    context.subscriptions.fan_out(event);
    prune_inspect_streams(context);
}

/// Folds a single event into the cached inspection snapshot, when primed.
///
/// This is a cheap incremental update: the (large) base frame is never
/// re-rendered or cloned here — [`crate::inspection::refresh`] does that before
/// every `inspect_*` frame.
fn update_inspection(context: &ServerContext, event: &RuntimeEvent) {
    let now_ms = context.now_ms();
    context.inspection.update(|snapshot| match event {
        RuntimeEvent::SurfaceCommit {
            window_id,
            commit_seq,
            ts_ms,
            damage,
            ..
        } if !damage.is_empty() => {
            let geometry = context
                .observer
                .window_state(*window_id)
                .and_then(|state| state.geometry);
            snapshot.damage = crate::inspection::damage_to_output(damage, geometry);
            snapshot.commit = Some(CommitInfo {
                commit_seq: *commit_seq,
                age_ms: now_ms.saturating_sub(*ts_ms),
            });
            snapshot.seq = event.seq();
            snapshot.ts_ms = now_ms;
        }
        RuntimeEvent::WindowActivated { window_id, .. } => {
            snapshot.active = Some(*window_id);
            snapshot.seq = event.seq();
            snapshot.ts_ms = now_ms;
        }
        RuntimeEvent::WindowDestroyed { window_id, .. } => {
            snapshot.windows.retain(|window| window.id != *window_id);
            if snapshot.active == Some(*window_id) {
                snapshot.active = None;
            }
            snapshot.seq = event.seq();
            snapshot.ts_ms = now_ms;
        }
        _ => {}
    });
}

/// Drops inspector streams whose connection has already closed.
///
/// The push loop removes its own stream on exit; this keeps the registry from
/// accumulating dead entries when a connection dies without unwinding its
/// tasks.
fn prune_inspect_streams(context: &ServerContext) {
    if context.inspect_subscriptions.is_empty() {
        return;
    }
    for subscription in context.inspect_subscriptions.list() {
        if subscription.sink.is_closed() {
            context.inspect_subscriptions.unsubscribe(subscription.id);
        }
    }
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
    let (reply, snapshot) = oneshot::channel();
    context.compositor.send(RuntimeCommand::QueryState { reply })?;
    let snapshot = snapshot.await.map_err(|_| ServerError::ShuttingDown)?;

    let report = context
        .observer
        .resync(crate::translate::observer_snapshot(&snapshot));
    tracing::debug!(
        seq = report.snapshot_seq,
        added = report.windows_added.len(),
        removed = report.windows_removed.len(),
        uncertain = report.marked_uncertain.len(),
        dropped = report.events_dropped,
        "observer resynchronized after broadcast lag"
    );

    let now_ms = context.now_ms();
    context.inspection.update(|cached| {
        cached.windows = snapshot.windows.clone();
        cached.active = snapshot.active_window_id;
        cached.seq = snapshot.seq;
        cached.ts_ms = now_ms;
    });
    Ok(())
}
