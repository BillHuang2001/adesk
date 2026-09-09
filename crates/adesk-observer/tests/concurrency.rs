//! Concurrency: many waiters on one service, cancellation, and the
//! `pending_observation` bookkeeping (`docs/architecture.md` §6).
//!
//! Every body drives exactly the scenario in its doc comment. Event-driven
//! resolution uses `tokio::join!(wait_futures..., feeder)` with a feeder that
//! yields once, so all waits are registered before the first event is fed;
//! `#[tokio::test(start_paused = true)]` keeps time deterministic.

mod common;

use adesk_core::WindowId;
use adesk_observer::{ObserverService, PendingObservation, QuietSpec, WaitSpec};

/// Three concurrent waiters on the same window must all resolve on the same
/// commit, each with its own `Observation` (`commits == 1`, `timed_out == false`).
#[tokio::test(start_paused = true)]
async fn concurrent_waiters_on_same_window_all_resolve() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(1_000);
    let (a, b, c, ()) = tokio::join!(
        observer.wait_for_change(spec.clone()),
        observer.wait_for_change(spec.clone()),
        observer.wait_for_change(spec),
        async {
            // Let all three waits register (subscribe + `PendingGuard`) first.
            tokio::task::yield_now().await;
            observer.handle_event(&common::commit(2, 40, 7, 1, &[common::rect(0, 0, 8, 8)]));
        },
    );

    for (index, observation) in [a, b, c].into_iter().enumerate() {
        let observation =
            observation.unwrap_or_else(|error| panic!("waiter {index} failed: {error}"));
        assert_eq!(
            observation.commits, 1,
            "waiter {index} counted the one commit"
        );
        assert!(
            !observation.timed_out,
            "waiter {index} resolved on the commit"
        );
        assert_eq!(observation.window_id, Some(WindowId(7)));
        assert_eq!(observation.last_commit_seq, 1);
        assert_eq!(
            observation.changed_regions,
            vec![common::rect(0, 0, 8, 8)],
            "waiter {index} reports the commit damage"
        );
        assert_eq!(observation.elapsed_ms, 40, "event clock domain");
        assert_eq!(observation.seq, 2);
    }

    // All three guards were released on resolution: no pending observation leaks.
    let state = observer
        .window_state(WindowId(7))
        .expect("window 7 tracked");
    assert_eq!(state.pending_observation, None);
}

/// A global waiter and a window-filtered waiter must not interfere: the global
/// one resolves on a commit to window 8, the filtered one keeps waiting for
/// window 7 and times out.
#[tokio::test(start_paused = true)]
async fn global_and_window_filtered_waiters_coexist() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::created(2, 0, 8));
    let global = observer.wait_for_change(WaitSpec::new().timeout_ms(100));
    let filtered = observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(100));
    let (global, filtered, ()) = tokio::join!(global, filtered, async {
        // Both waits register before the commit to window 8 is fed.
        tokio::task::yield_now().await;
        observer.handle_event(&common::commit(3, 10, 8, 1, &[common::rect(0, 0, 4, 4)]));
    });

    let global = global.expect("an unfiltered wait never errors");
    assert_eq!(
        global.commits, 1,
        "the global waiter counts the window-8 commit"
    );
    assert!(!global.timed_out, "the commit resolves the global waiter");
    assert_eq!(global.window_id, None);
    assert_eq!(global.last_commit_seq, 1);
    assert_eq!(global.changed_regions, vec![common::rect(0, 0, 4, 4)]);
    assert_eq!(global.elapsed_ms, 10);

    let filtered = filtered.expect("window 7 is tracked");
    assert!(
        filtered.timed_out,
        "the window-8 commit is invisible to window 7"
    );
    assert_eq!(filtered.commits, 0, "the filtered waiter counts nothing");
    assert_eq!(filtered.window_id, Some(WindowId(7)));
    assert_eq!(
        filtered.elapsed_ms, 100,
        "the filtered waiter ran to its timeout"
    );
}

/// Dropping a pending wait (client disconnect) must clear the window's
/// `pending_observation` entry and must not leak the journal or the generation
/// watcher: after cancellation `snapshot()` shows `pending_observation == None`.
#[tokio::test(start_paused = true)]
async fn dropped_waiter_clears_pending_observation() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let _spec = QuietSpec::new()
        .window(WindowId(7))
        .quiet_ms(50)
        .timeout_ms(1_000);

    // Poll the wait once so it is registered, then leave it pending.
    let mut wait = Box::pin(observer.wait_for_quiet(_spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    let pending = observer
        .window_state(WindowId(7))
        .expect("window 7 tracked")
        .pending_observation;
    assert_eq!(
        pending,
        Some(PendingObservation {
            since_seq: 1,
            started_at: 0
        }),
        "the wait is registered while it is pending"
    );

    // Client disconnect: the pending wait future is dropped.
    drop(wait);
    tokio::task::yield_now().await;

    let snapshot = observer.snapshot();
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].window_id, WindowId(7));
    assert_eq!(
        snapshot.windows[0].pending_observation, None,
        "dropping the waiter must release its pending_observation entry"
    );
    assert_eq!(
        observer
            .window_state(WindowId(7))
            .expect("window 7 tracked")
            .pending_observation,
        None
    );
    assert_eq!(
        snapshot.journal_len, 1,
        "cancellation must not leak journal entries"
    );
}

/// A waiter must wake on the generation counter, not on time: with the paused
/// clock never advancing, an event fed after registration still resolves the
/// wait (`elapsed_ms` comes from the event timestamps).
#[tokio::test(start_paused = true)]
async fn waiter_wakes_without_time_advancing() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let before = tokio::time::Instant::now();

    let mut wait =
        Box::pin(observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }

    // No `tokio::time::advance` and no timer involved: the generation counter
    // must wake the waiter.
    observer.handle_event(&common::commit(2, 40, 7, 1, &[common::rect(0, 0, 8, 8)]));

    let observation = wait.await.expect("known window");
    assert!(!observation.timed_out, "the commit resolves the wait");
    assert_eq!(observation.commits, 1);
    assert_eq!(observation.changed_regions, vec![common::rect(0, 0, 8, 8)]);
    assert_eq!(
        observation.elapsed_ms, 40,
        "elapsed_ms comes from the event ts_ms, not from tokio time"
    );
    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "the paused clock never advanced"
    );
}

/// While a wait is pending, `snapshot()`/`window_state()` expose
/// `pending_observation = Some(PendingObservation { since_seq, started_at })`
/// with `since_seq` = the filter point and `started_at` = wait start.
#[tokio::test(start_paused = true)]
async fn pending_observation_is_visible_in_snapshot() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));

    let mut wait =
        Box::pin(observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }

    let state = observer.window_state(WindowId(7));
    let state = state.expect("window 7 tracked");
    assert_eq!(
        state.pending_observation,
        Some(PendingObservation {
            since_seq: 1,
            started_at: 0
        }),
        "since_seq = watermark at wait start (1), started_at = wait start (0)"
    );

    let snapshot = observer.snapshot();
    assert_eq!(snapshot.seq, 1);
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(
        snapshot.windows[0].pending_observation, state.pending_observation,
        "the snapshot exposes the same pending observation as window_state()"
    );

    // Clean the pending wait up: dropping it must clear the entry again.
    drop(wait);
    assert_eq!(
        observer
            .window_state(WindowId(7))
            .expect("window 7 tracked")
            .pending_observation,
        None
    );
}
