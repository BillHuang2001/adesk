//! Broadcast lag / resync (`docs/architecture.md` §1): the server answers
//! `RecvError::Lagged` with a `QueryState` snapshot instead of silently
//! continuing, and the observer must stay correct.
//!
//! Each test's doc comment is the normative scenario.

mod common;

use adesk_core::{Observation, WindowId};
use adesk_observer::{ObserverService, Result, StateSnapshot, WaitSpec, WindowSnapshot};

/// Spawns a `wait_for_change` on `window_id` with a horizon far beyond the test,
/// and yields until the waiter is provably in flight: `pending_observation` is
/// set on its first poll, so a resync that follows cannot land before the waiter
/// captured its filter point.
async fn change_waiter_in_flight(
    observer: &ObserverService,
    window_id: WindowId,
) -> tokio::task::JoinHandle<Result<Observation>> {
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move {
            observer
                .wait_for_change(WaitSpec::new().window(window_id).timeout_ms(60_000))
                .await
        }
    });
    let mut registered = false;
    for _ in 0..64 {
        if observer
            .window_state(window_id)
            .and_then(|state| state.pending_observation)
            .is_some()
        {
            registered = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(registered, "the waiter must be in flight before the resync");
    handle
}

/// `resync` with a snapshot containing an unknown window marks every affected
/// window `state_uncertain`, and the next real event clears the flag.
#[tokio::test(start_paused = true)]
async fn resync_marks_windows_uncertain() {
    let observer = ObserverService::new();
    let snapshot = StateSnapshot {
        seq: 10,
        ts_ms: 100,
        windows: vec![WindowSnapshot {
            window_id: WindowId(7),
            last_commit_seq: 3,
            geometry: common::rect(0, 0, 1280, 800),
            popup_count: 0,
        }],
    };
    let report = observer.resync(snapshot);

    assert_eq!(report.snapshot_seq, 10);
    assert_eq!(report.windows_added, vec![WindowId(7)]);
    assert_eq!(report.windows_removed, Vec::new());
    assert_eq!(report.marked_uncertain, vec![WindowId(7)]);
    assert_eq!(report.events_dropped, 0);
    assert_eq!(observer.watermark(), 10);

    let state = observer
        .window_state(WindowId(7))
        .expect("resync added the unknown window");
    assert!(state.state_uncertain, "the lag leaves a hole in the history");
    assert_eq!(
        state.last_commit_seq, 3,
        "the snapshot's commit watermark is adopted"
    );
    assert_eq!(state.geometry, Some(common::rect(0, 0, 1280, 800)));

    // The next real event for window 7 clears the flag.
    observer.handle_event(&common::title_changed(11, 110, 7, Some("ready")));
    let state = observer
        .window_state(WindowId(7))
        .expect("the window is still tracked");
    assert!(
        !state.state_uncertain,
        "a real event clears the uncertainty flag"
    );
    assert_eq!(
        state.last_commit_seq, 3,
        "a non-commit event keeps the commit watermark"
    );
}

/// A window whose `last_commit_seq` advanced during the lag gets a synthesized
/// commit at `snapshot.seq`, so a waiter counting commits still resolves:
/// `commits >= 1`, `last_commit_seq == snapshot.last_commit_seq`.
#[tokio::test(start_paused = true)]
async fn resync_synthesizes_missed_commits_for_waiters() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let snapshot = StateSnapshot {
        seq: 50,
        ts_ms: 500,
        windows: vec![WindowSnapshot {
            window_id: WindowId(7),
            last_commit_seq: 9,
            geometry: common::rect(0, 0, 1280, 800),
            popup_count: 0,
        }],
    };

    // Register the waiter before the resync: the synthesized commit is what must
    // resolve it.
    let waiter = change_waiter_in_flight(&observer, WindowId(7)).await;
    let report = observer.resync(snapshot);

    assert_eq!(report.snapshot_seq, 50);
    assert_eq!(report.windows_added, Vec::new());
    assert_eq!(report.windows_removed, Vec::new());
    assert_eq!(report.marked_uncertain, vec![WindowId(7)]);

    let observation = waiter
        .await
        .expect("waiter task must not panic")
        .expect("window 7 is known");
    assert!(
        !observation.timed_out,
        "the synthesized commit resolves the wait: {observation:?}"
    );
    assert!(
        observation.commits >= 1,
        "the snapshot's commit is counted: {observation:?}"
    );
    assert_eq!(observation.last_commit_seq, 9);
    assert_eq!(observation.window_id, Some(WindowId(7)));
    assert_eq!(observation.seq, 50);
}

/// A window missing from the snapshot was destroyed during the lag: it is
/// removed from state, and an in-flight waiter on it resolves with
/// `destroyed_windows == [id]` instead of hanging.
#[tokio::test(start_paused = true)]
async fn resync_removes_destroyed_windows_and_notifies() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let snapshot = StateSnapshot {
        seq: 20,
        ts_ms: 200,
        windows: vec![],
    };

    // The waiter must already be pending when the snapshot proves the window gone.
    let waiter = change_waiter_in_flight(&observer, WindowId(7)).await;
    let report = observer.resync(snapshot);

    assert_eq!(report.snapshot_seq, 20);
    assert_eq!(report.windows_removed, vec![WindowId(7)]);
    assert_eq!(report.windows_added, Vec::new());
    assert_eq!(report.marked_uncertain, Vec::new());
    assert_eq!(
        observer.window_state(WindowId(7)),
        None,
        "the destroyed window's state is dropped"
    );

    let observation = waiter
        .await
        .expect("waiter task must not panic")
        .expect("window 7 was known when the wait started");
    assert!(
        !observation.timed_out,
        "destruction resolves the wait instead of hanging: {observation:?}"
    );
    assert_eq!(observation.destroyed_windows, vec![WindowId(7)]);
    assert_eq!(observation.window_id, Some(WindowId(7)));
}

/// `resync` adopts `snapshot.seq` as the watermark (never moves it backwards),
/// prunes journal entries at or below it, and reports `events_dropped`.
#[tokio::test(start_paused = true)]
async fn resync_advances_watermark_and_prunes_journal() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(0, 0, 4, 4)]));
    assert_eq!(observer.snapshot().journal_len, 2, "both events are retained");
    let snapshot = StateSnapshot {
        seq: 2,
        ts_ms: 20,
        windows: vec![WindowSnapshot {
            window_id: WindowId(7),
            last_commit_seq: 1,
            geometry: common::rect(0, 0, 1280, 800),
            popup_count: 0,
        }],
    };
    let report = observer.resync(snapshot);

    assert_eq!(report.snapshot_seq, 2);
    assert_eq!(
        report.events_dropped, 2,
        "seq 1 and seq 2 are superseded by the snapshot"
    );
    assert_eq!(report.windows_added, Vec::new());
    assert_eq!(report.windows_removed, Vec::new());
    assert_eq!(report.marked_uncertain, vec![WindowId(7)]);
    assert_eq!(observer.watermark(), 2);
    assert_eq!(
        observer.snapshot().journal_len,
        0,
        "journal pruned through snapshot.seq"
    );
}

/// A stale snapshot (`seq` below the watermark, e.g. a reply that raced newer
/// events) must never move the watermark backwards nor drop newer state.
#[tokio::test(start_paused = true)]
async fn resync_ignores_stale_snapshots() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(5, 50, 7));
    let snapshot = StateSnapshot {
        seq: 1,
        ts_ms: 10,
        windows: vec![],
    };
    let report = observer.resync(snapshot);

    assert_eq!(
        report.snapshot_seq, 5,
        "the report carries the current watermark, not the stale one"
    );
    assert_eq!(report.windows_added, Vec::new());
    assert_eq!(
        report.windows_removed,
        Vec::new(),
        "a stale snapshot must not prune"
    );
    assert_eq!(report.marked_uncertain, Vec::new());
    assert_eq!(report.events_dropped, 0);

    assert_eq!(observer.watermark(), 5, "never moves backwards");
    assert!(
        observer.window_state(WindowId(7)).is_some(),
        "window 7 still exists"
    );
    assert_eq!(
        observer.snapshot().journal_len,
        1,
        "the journal is untouched"
    );
    assert_eq!(observer.now_ms(), 50, "the clock never moves backwards");
}
