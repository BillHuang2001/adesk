//! Broadcast lag / resync (`docs/architecture.md` §1): the server answers
//! `RecvError::Lagged` with a `QueryState` snapshot instead of silently
//! continuing, and the observer must stay correct.
//!
//! Phase 1: bodies stop at `todo!()`; the doc comment is the scenario.

mod common;

use adesk_core::WindowId;
use adesk_observer::{ObserverService, StateSnapshot, WaitSpec, WindowSnapshot};

/// `resync` with a snapshot containing an unknown window marks every affected
/// window `state_uncertain`, and the next real event clears the flag.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
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
    let _report = observer.resync(snapshot);
    todo!("Phase 2: assert windows_added == [7] and state_uncertain == true")
}

/// A window whose `last_commit_seq` advanced during the lag gets a synthesized
/// commit at `snapshot.seq`, so a waiter counting commits still resolves:
/// `commits >= 1`, `last_commit_seq == snapshot.last_commit_seq`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
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
    let _report = observer.resync(snapshot);
    let _ = observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(10));
    todo!("Phase 2: assert the synthesized commit resolves an in-flight wait")
}

/// A window missing from the snapshot was destroyed during the lag: it is
/// removed from state, and an in-flight waiter on it resolves with
/// `destroyed_windows == [id]` instead of hanging.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn resync_removes_destroyed_windows_and_notifies() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let snapshot = StateSnapshot {
        seq: 20,
        ts_ms: 200,
        windows: vec![],
    };
    let _report = observer.resync(snapshot);
    todo!("Phase 2: assert windows_removed == [7] and waiters see the destruction")
}

/// `resync` adopts `snapshot.seq` as the watermark (never moves it backwards),
/// prunes journal entries at or below it, and reports `events_dropped`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn resync_advances_watermark_and_prunes_journal() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(0, 0, 4, 4)]));
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
    let _report = observer.resync(snapshot);
    todo!("Phase 2: assert watermark == 2, journal pruned, report fields")
}

/// A stale snapshot (`seq` below the watermark, e.g. a reply that raced newer
/// events) must never move the watermark backwards nor drop newer state.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn resync_ignores_stale_snapshots() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(5, 50, 7));
    let snapshot = StateSnapshot {
        seq: 1,
        ts_ms: 10,
        windows: vec![],
    };
    let _report = observer.resync(snapshot);
    todo!("Phase 2: assert watermark stays 5 and window 7 still exists")
}
