//! Concurrency: many waiters on one service, cancellation, and the
//! `pending_observation` bookkeeping (`docs/architecture.md` §6).
//!
//! Phase 1: bodies stop at `todo!()`; the doc comment is the scenario.

mod common;

use adesk_core::WindowId;
use adesk_observer::{ObserverService, QuietSpec, WaitSpec};

/// Three concurrent waiters on the same window must all resolve on the same
/// commit, each with its own `Observation` (`commits == 1`, `timed_out == false`).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn concurrent_waiters_on_same_window_all_resolve() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(1_000);
    let (a, b, c) = tokio::join!(
        observer.wait_for_change(spec.clone()),
        observer.wait_for_change(spec.clone()),
        observer.wait_for_change(spec),
    );
    let _ = (a, b, c);
    todo!("Phase 2: feed one commit while three waits are pending; all must resolve")
}

/// A global waiter and a window-filtered waiter must not interfere: the global
/// one resolves on a commit to window 8, the filtered one keeps waiting for
/// window 7 and times out.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn global_and_window_filtered_waiters_coexist() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::created(2, 0, 8));
    let global = observer.wait_for_change(WaitSpec::new().timeout_ms(100));
    let filtered = observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(100));
    let (global, filtered) = tokio::join!(global, filtered);
    let _ = (global, filtered);
    todo!("Phase 2: commit on window 8; assert global resolves and filtered times out")
}

/// Dropping a pending wait (client disconnect) must clear the window's
/// `pending_observation` entry and must not leak the journal or the generation
/// watcher: after cancellation `snapshot()` shows `pending_observation == None`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn dropped_waiter_clears_pending_observation() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let _spec = QuietSpec::new().window(WindowId(7)).quiet_ms(50).timeout_ms(1_000);
    let snapshot = observer.snapshot();
    let _ = snapshot;
    todo!("Phase 2: start a wait, drop it, then assert pending_observation is None")
}

/// A waiter must wake on the generation counter, not on time: with the paused
/// clock never advancing, an event fed after registration still resolves the
/// wait (`elapsed_ms` comes from the event timestamps).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn waiter_wakes_without_time_advancing() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let wait = observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000));
    let observation = wait.await;
    let _ = observation;
    todo!("Phase 2: feed a commit after registration without advancing tokio time")
}

/// While a wait is pending, `snapshot()`/`window_state()` expose
/// `pending_observation = Some(PendingObservation { since_seq, started_at })`
/// with `since_seq` = the filter point and `started_at` = wait start.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn pending_observation_is_visible_in_snapshot() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let state = observer.window_state(WindowId(7));
    let _ = state;
    todo!("Phase 2: assert pending_observation contents while a wait is in flight")
}
