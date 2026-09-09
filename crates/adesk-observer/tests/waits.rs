//! `wait_for_change` / `wait_for_quiet` / `observe` semantics (AGP §5.4).
//!
//! Each body drives exactly the scenario of its doc comment (the
//! normative acceptance spec) and asserts the documented values. Waits are
//! deterministic: `tokio::time::pause()` freezes tokio time, while the observer's
//! clock only moves when an event re-anchors it (`Clock::observe_ts`, so feeding
//! an event with `ts_ms = t` makes the event domain reach `t` immediately) or when
//! the runtime auto-advances to a parked wait's deadline.
//!
//! Driving patterns:
//! - event-driven resolution: park the wait first (`select!` + `yield_now`), then
//!   feed the event — the generation bump wakes the waiter, which absorbs the
//!   event from the journal;
//! - deadline-driven resolution: just `await` the wait — with all tasks idle the
//!   runtime advances time to the next deadline in the event clock domain.

mod common;

use std::time::Duration;

use adesk_core::{Observation, WindowId};
use adesk_observer::{Condition, ObserveSpec, ObserverService, QuietSpec, WaitSpec};

/// Feed `common::commit(2, 40, 7, 1, ..)` while the wait is pending.
/// Expect: `commits == 1`, `timed_out == false`, `quiet == false`,
/// `elapsed_ms == 40`, `last_commit_seq == 1`, `window_id == Some(WindowId(7))`,
/// and `changed_regions` containing the commit damage.
#[tokio::test(start_paused = true)]
async fn wait_for_change_resolves_on_first_counted_commit() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(1_000);

    // Park the wait first, then feed the commit: the waiter must count the live
    // event (this is the no-lost-wakeup case).
    let mut wait = Box::pin(observer.wait_for_change(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::commit(2, 40, 7, 1, &[common::rect(0, 0, 4, 4)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(
        !observation.timed_out,
        "the first counted commit resolves the wait"
    );
    assert_eq!(observation.commits, 1);
    assert!(!observation.quiet, "0 ms < the 250 ms evidence threshold");
    assert_eq!(observation.elapsed_ms, 40);
    assert_eq!(observation.last_commit_seq, 1);
    assert_eq!(observation.window_id, Some(WindowId(7)));
    assert_eq!(
        observation.changed_regions,
        vec![common::rect(0, 0, 4, 4)],
        "the commit damage (simplified) is reported"
    );
}

/// No event at all, `timeout_ms = 300`.
/// Expect: resolves at exactly 300 ms with `timed_out == true`, `commits == 0`,
/// `changed_regions == []`, `quiet == true` (window has been quiet since the
/// wait started), `elapsed_ms == 300`, `seq == 1` (the only event's watermark).
#[tokio::test(start_paused = true)]
async fn wait_for_change_times_out_without_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(300);

    // Nothing is ever fed: with every task idle the runtime advances to the
    // wait's deadline in the event clock domain.
    let observation: Observation = observer.wait_for_change(spec).await.expect("known window");

    assert!(
        observation.timed_out,
        "a deadline is an observation, never an error"
    );
    assert_eq!(observation.commits, 0);
    assert_eq!(observation.changed_regions, Vec::new());
    assert!(
        observation.quiet,
        "the window has been quiet since the wait started"
    );
    assert_eq!(observation.elapsed_ms, 300);
    assert_eq!(observation.seq, 1, "the only event's watermark");
}

/// Commits at 50 ms and 120 ms while the wait is parked; `timeout_ms = 400`.
/// Expect: resolution on the first counted commit (`timed_out == false`), with
/// every event absorbed before the waiter ran reported: `commits == 2`,
/// `last_commit_seq == 2`, `changed_regions` = union of both commits
/// (simplified), `quiet == false`, `elapsed_ms == 120`.
///
/// Per docs/protocol.md §5.4, wait_for_change resolves on the first counted surface commit; this spec was corrected to match the normative protocol.
#[tokio::test(start_paused = true)]
async fn wait_for_change_timeout_reports_accumulated_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(400);

    let mut wait = Box::pin(observer.wait_for_change(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::commit(2, 50, 7, 1, &[common::rect(0, 0, 4, 4)]));
    observer.handle_event(&common::commit(3, 120, 7, 2, &[common::rect(10, 10, 2, 2)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(
        !observation.timed_out,
        "the first counted surface commit resolves the wait: {observation:?}"
    );
    assert_eq!(observation.commits, 2);
    assert_eq!(observation.last_commit_seq, 2);
    assert_eq!(
        observation.changed_regions,
        vec![common::rect(0, 0, 4, 4), common::rect(10, 10, 2, 2)],
        "union of both commit damages, simplified"
    );
    assert!(
        !observation.quiet,
        "0 ms since the last commit < the 250 ms evidence threshold"
    );
    assert_eq!(
        observation.elapsed_ms, 120,
        "resolved at the second commit's timestamp"
    );
}

/// `quiet_ms = 100`, one commit at 30 ms, silence after.
/// Expect: resolution at 130 ms, `timed_out == false`, `quiet == true`,
/// `commits == 1`, `elapsed_ms == 130`.
#[tokio::test(start_paused = true)]
async fn wait_for_quiet_resolves_after_quiet_ms() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new()
        .window(WindowId(7))
        .quiet_ms(100)
        .timeout_ms(1_000);

    let mut wait = Box::pin(observer.wait_for_quiet(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::commit(2, 30, 7, 1, &[common::rect(0, 0, 4, 4)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(!observation.timed_out);
    assert!(observation.quiet);
    assert_eq!(observation.commits, 1);
    assert_eq!(observation.elapsed_ms, 130, "commit ts(30) + quiet_ms(100)");
    assert_eq!(observer.now_ms(), 130, "resolved at the quiet deadline");
}

/// Commits every 60 ms with `quiet_ms = 100`.
/// Expect: the timer re-arms on every commit, so the wait only resolves
/// `quiet_ms` after the **last** commit (`elapsed_ms == last_commit_ts + 100`),
/// with `commits == 3` and `quiet == true`.
#[tokio::test(start_paused = true)]
async fn wait_for_quiet_rearms_on_every_commit() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new()
        .window(WindowId(7))
        .quiet_ms(100)
        .timeout_ms(1_000);

    let mut wait = Box::pin(observer.wait_for_quiet(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::commit(2, 60, 7, 1, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(3, 120, 7, 2, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(4, 180, 7, 3, &[common::rect(0, 0, 2, 2)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(!observation.timed_out);
    assert_eq!(observation.commits, 3);
    assert!(observation.quiet);
    assert_eq!(
        observation.elapsed_ms, 280,
        "last commit ts(180) + quiet_ms(100)"
    );
    assert_eq!(observer.now_ms(), 280, "the timer re-armed on every commit");
}

/// Commits every 40 ms with `quiet_ms = 100`, `timeout_ms = 200`.
/// Expect: quiescence never holds for 100 ms, so the wait times out at 200 ms
/// with `timed_out == true`, `quiet == false`, `commits == 5`.
#[tokio::test(start_paused = true)]
async fn wait_for_quiet_times_out_while_commits_keep_arriving() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new()
        .window(WindowId(7))
        .quiet_ms(100)
        .timeout_ms(200);

    let mut wait = Box::pin(observer.wait_for_quiet(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    // Feed all five commits before the waiter is polled again, so the commit at
    // ts = 200 is absorbed before the deadline check at 200 ms.
    observer.handle_event(&common::commit(2, 40, 7, 1, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(3, 80, 7, 2, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(4, 120, 7, 3, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(5, 160, 7, 4, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(6, 200, 7, 5, &[common::rect(0, 0, 2, 2)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(
        observation.timed_out,
        "quiescence never holds for quiet_ms(100)"
    );
    assert!(!observation.quiet, "0 ms of quiet at the deadline");
    assert_eq!(observation.commits, 5);
    assert_eq!(observation.elapsed_ms, 200);
}

/// `Condition::Timeout` with `timeout_ms = 250`, commits at 20/60/140 ms.
/// Expect: the condition *is* the horizon, so `timed_out == false`,
/// `commits == 3`, `elapsed_ms == 250`, `quiet` reflects the 250 ms default
/// threshold (last commit 110 ms ago → `quiet == false`).
#[tokio::test(start_paused = true)]
async fn observe_timeout_condition_samples_animation_window() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Timeout)
        .window(WindowId(7))
        .timeout_ms(250);

    let mut wait = Box::pin(observer.observe(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::commit(2, 20, 7, 1, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(3, 60, 7, 2, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(4, 140, 7, 3, &[common::rect(0, 0, 2, 2)]));
    let observation: Observation = wait.await.expect("known window");

    assert!(!observation.timed_out, "the horizon *is* the condition");
    assert_eq!(observation.commits, 3);
    assert_eq!(observation.elapsed_ms, 250);
    assert!(
        !observation.quiet,
        "110 ms since the last commit < the 250 ms default threshold"
    );
}

/// `Condition::Quiet { quiet_ms: 100 }` must behave exactly like
/// `wait_for_quiet(quiet_ms = 100)` (same machinery, same resolution time).
#[tokio::test(start_paused = true)]
async fn observe_quiet_condition_matches_wait_for_quiet() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Quiet { quiet_ms: 100 })
        .window(WindowId(7))
        .timeout_ms(1_000);

    // Reference: the dedicated `wait_for_quiet` with the same threshold, driven
    // concurrently so both resolve at the same point of the event clock.
    let reference_service = ObserverService::new();
    reference_service.handle_event(&common::created(1, 0, 7));
    let reference_spec = QuietSpec::new()
        .window(WindowId(7))
        .quiet_ms(100)
        .timeout_ms(1_000);

    let (observation, reference) = tokio::join!(
        observer.observe(spec),
        reference_service.wait_for_quiet(reference_spec),
    );
    let observation = observation.expect("known window");
    let reference = reference.expect("known window");

    assert!(!observation.timed_out);
    assert!(observation.quiet);
    assert_eq!(observation.commits, 0);
    assert_eq!(observation.elapsed_ms, 100, "anchor + quiet_ms");
    assert_eq!(observation, reference, "observe(quiet) == wait_for_quiet");
}

/// `Condition::Change` resolves on a lifecycle event (not only commits):
/// feed `common::title_changed(2, 25, 7, Some("hi"))` while waiting.
/// Expect: `commits == 0`, `title_changed == true`, `timed_out == false`,
/// `quiet == false`, `elapsed_ms == 25`.
#[tokio::test(start_paused = true)]
async fn observe_change_condition_resolves_on_lifecycle_event() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Change)
        .window(WindowId(7))
        .timeout_ms(500);

    let mut wait = Box::pin(observer.observe(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    observer.handle_event(&common::title_changed(2, 25, 7, Some("hi")));
    let observation: Observation = wait.await.expect("known window");

    assert!(!observation.timed_out);
    assert_eq!(observation.commits, 0);
    assert!(observation.title_changed);
    assert!(!observation.quiet, "25 ms < the 250 ms evidence threshold");
    assert_eq!(observation.elapsed_ms, 25);
}

/// The observation's `elapsed_ms` is measured in the event `ts_ms` domain:
/// with events at 0 and 40 ms and resolution at 90 ms, `elapsed_ms == 90`.
#[tokio::test(start_paused = true)]
async fn observation_elapsed_uses_event_clock_domain() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(90);

    let mut wait = Box::pin(observer.wait_for_change(spec));
    tokio::select! {
        biased;
        observation = &mut wait => panic!("resolved before any event: {observation:?}"),
        () = tokio::task::yield_now() => {}
    }
    // `AppLaunched` is never counted: it only advances the watermark and the
    // event clock, so the `Change` condition still has to wait for the deadline.
    observer.handle_event(&common::app_launched(2, 40, 1));
    let observation: Observation = wait.await.expect("known window");
    let _ = Duration::from_millis(0);

    assert_eq!(
        observation.elapsed_ms, 90,
        "measured in the event ts_ms domain"
    );
    assert_eq!(observation.seq, 2, "the launch advanced the watermark");
}
