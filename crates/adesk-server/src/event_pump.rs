//! Compositor `RuntimeEvent` broadcast → observer, subscription fan-out and
//! inspection caches; `QueryState` resync after broadcast lag.
//!
//! One pump task per runtime subscribes to [`adesk_compositor::CompositorHandle::subscribe`]
//! and is the **only** writer of the observer's event history
//! (`docs/architecture.md` §2). On `RecvError::Lagged` it issues `QueryState`
//! and calls `ObserverService::resync` so missed events become *uncertainty*
//! rather than silent loss.

use std::borrow::Cow;
use std::sync::Mutex;

use adesk_app_registry::{CorrelationOutcome, Correlator, WindowCandidate};
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
    // The matching half of the launch → window correlation: `launch_app`
    // records the launch, the pump attributes a mapping to it.
    let event = correlate_window(&context.correlator, event);
    let event = event.as_ref();

    // The observer is the only writer of the event history; feed it first so
    // any waiter woken by the fan-out already sees the event.
    context.observer.handle_event(event);
    update_inspection(context, event);
    context.subscriptions.fan_out(event);
    prune_inspect_streams(context);
}

/// Stamps `launch_id` onto a `WindowCreated` event that does not carry one yet
/// (`docs/architecture.md` §7).
///
/// `launch_app` registers the [`adesk_app_registry::LaunchRecord`] with the
/// correlator ([`crate::dispatch::apps::launch_app`]); this is the matching
/// side. The rewritten event is handed to *every* downstream consumer
/// (observer, inspection cache, fan-out), so the AGP event, the observer state
/// and the inspection snapshot agree. A window no pending launch matches is
/// passed through unchanged — correlation is reported, never guessed — and
/// events other than `WindowCreated` are never inspected.
///
/// Never blocks: the correlator lock is held only for the in-memory match, and
/// a poisoned lock is recovered (no panic on the event path).
fn correlate_window<'a>(
    correlator: &Mutex<Correlator>,
    event: &'a RuntimeEvent,
) -> Cow<'a, RuntimeEvent> {
    let RuntimeEvent::WindowCreated {
        window_id,
        pid,
        app_id,
        launch_id: None,
        title,
        ..
    } = event
    else {
        return Cow::Borrowed(event);
    };

    let candidate = WindowCandidate {
        window_id: *window_id,
        pid: *pid,
        app_id: app_id.as_ref().map(|id| id.as_str()),
        title: title.as_deref(),
    };
    let outcome = correlator
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .correlate(&candidate);

    let CorrelationOutcome::Correlated(correlation) = outcome else {
        return Cow::Borrowed(event);
    };

    tracing::debug!(
        window_id = window_id.0,
        launch_id = correlation.launch.launch_id.0,
        evidence = ?correlation.evidence,
        "window correlated with a pending launch"
    );

    let mut stamped = event.clone();
    if let RuntimeEvent::WindowCreated { launch_id, .. } = &mut stamped {
        *launch_id = Some(correlation.launch.launch_id);
    }
    Cow::Owned(stamped)
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
    context
        .compositor
        .send(RuntimeCommand::QueryState { reply })?;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use adesk_app_registry::{LaunchRecord, MonotonicClock};
    use adesk_core::{AppId, LaunchId, WindowId};

    use super::*;

    fn correlator() -> Mutex<Correlator> {
        Mutex::new(Correlator::new(Arc::new(MonotonicClock::new())))
    }

    fn app(id: &str, name: &str, startup_wm_class: Option<&str>) -> adesk_core::AppInfo {
        adesk_core::AppInfo {
            id: AppId::from(id),
            name: name.into(),
            icon: None,
            exec: Some("/bin/true".into()),
            terminal: false,
            categories: Vec::new(),
            startup_wm_class: startup_wm_class.map(str::to_owned),
            dbus_activatable: false,
            hidden: false,
            no_display: false,
            try_exec: None,
        }
    }

    fn launch(launch_id: u64, app_id: &str, pid: Option<i32>) -> LaunchRecord {
        LaunchRecord {
            launch_id: LaunchId(launch_id),
            app_id: AppId::from(app_id),
            pid,
            started_at_ms: 0,
        }
    }

    fn window_created(launch_id: Option<LaunchId>) -> RuntimeEvent {
        RuntimeEvent::WindowCreated {
            seq: 7,
            ts_ms: 70,
            window_id: WindowId(3),
            app_id: Some(AppId::from("org.example.launched")),
            pid: Some(4242),
            launch_id,
            title: Some("Launched Fixture".into()),
        }
    }

    #[test]
    fn window_created_without_launch_id_is_stamped_by_the_correlator() {
        let correlator = correlator();
        correlator
            .lock()
            .unwrap()
            .record_launch(
                launch(1, "org.example.launched", Some(4242)),
                &app("org.example.launched", "Launched Fixture", None),
            );

        let event = window_created(None);
        let rewritten = correlate_window(&correlator, &event);

        let RuntimeEvent::WindowCreated {
            seq,
            ts_ms,
            window_id,
            app_id,
            pid,
            launch_id,
            title,
        } = rewritten.as_ref()
        else {
            panic!("correlated WindowCreated must stay a WindowCreated");
        };
        assert_eq!(launch_id, &Some(LaunchId(1)));
        // The rest of the event is preserved verbatim.
        assert_eq!(seq, &7);
        assert_eq!(ts_ms, &70);
        assert_eq!(window_id, &WindowId(3));
        assert_eq!(app_id.as_ref(), Some(&AppId::from("org.example.launched")));
        assert_eq!(pid, &Some(4242));
        assert_eq!(title.as_deref(), Some("Launched Fixture"));
        assert_eq!(
            correlator.lock().unwrap().pending(),
            1,
            "a launch stays pending: several windows of one launch may correlate"
        );
    }

    #[test]
    fn uncorrelated_window_created_is_passed_through_unchanged() {
        let correlator = correlator();
        let event = window_created(None);

        let rewritten = correlate_window(&correlator, &event);

        assert!(
            matches!(rewritten, Cow::Borrowed(_)),
            "an unmatched window must not be rewritten (never guess)"
        );
        assert_eq!(rewritten.as_ref(), &event);
    }

    #[test]
    fn window_created_with_a_launch_id_is_never_re_correlated() {
        let correlator = correlator();
        correlator
            .lock()
            .unwrap()
            .record_launch(
                launch(9, "org.example.launched", Some(4242)),
                &app("org.example.launched", "Launched Fixture", None),
            );
        let event = window_created(Some(LaunchId(1)));

        let rewritten = correlate_window(&correlator, &event);

        assert!(
            matches!(rewritten, Cow::Borrowed(_)),
            "an event that already carries a launch id is left alone"
        );
        assert_eq!(rewritten.as_ref(), &event);
    }

    #[test]
    fn events_other_than_window_created_are_never_correlated() {
        let correlator = correlator();
        correlator
            .lock()
            .unwrap()
            .record_launch(
                launch(1, "org.example.launched", Some(4242)),
                &app("org.example.launched", "Launched Fixture", None),
            );

        let events = [
            RuntimeEvent::WindowDestroyed {
                seq: 8,
                ts_ms: 80,
                window_id: WindowId(3),
            },
            RuntimeEvent::TitleChanged {
                seq: 9,
                ts_ms: 90,
                window_id: WindowId(3),
                title: Some("renamed".into()),
            },
            RuntimeEvent::WindowActivated {
                seq: 10,
                ts_ms: 100,
                window_id: WindowId(3),
                previous: None,
            },
        ];
        for event in &events {
            let rewritten = correlate_window(&correlator, event);
            assert!(matches!(rewritten, Cow::Borrowed(_)), "{event:?}");
            assert_eq!(rewritten.as_ref(), event);
        }
        assert_eq!(
            correlator.lock().unwrap().pending(),
            1,
            "non-`WindowCreated` events must not consume a pending launch"
        );
    }

    #[test]
    fn a_poisoned_correlator_lock_still_correlates() {
        let correlator = correlator();
        correlator
            .lock()
            .unwrap()
            .record_launch(
                launch(1, "org.example.launched", Some(4242)),
                &app("org.example.launched", "Launched Fixture", None),
            );

        // Poison the lock the way a panicking request handler would.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = correlator.lock().unwrap();
            panic!("poison the correlator lock");
        }));
        assert!(correlator.is_poisoned(), "the lock must be poisoned");

        let event = window_created(None);
        let rewritten = correlate_window(&correlator, &event);

        assert!(
            matches!(
                rewritten.as_ref(),
                RuntimeEvent::WindowCreated {
                    launch_id: Some(LaunchId(1)),
                    ..
                }
            ),
            "the event path recovers the poisoned lock instead of panicking: {rewritten:?}"
        );
    }
}
