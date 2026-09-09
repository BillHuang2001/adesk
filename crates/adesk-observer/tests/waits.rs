//! `wait_for_change` / `wait_for_quiet` / `observe` semantics (AGP §5.4).
//!
//! Phase 1: every test is `#[ignore]`d and its body stops at `todo!()`; the doc
//! comment is the scenario and the expected assertions. Phase 2 rewrites each
//! body — do **not** merely drop `#[ignore]`: waits driven by
//! `tokio::time::pause()` hang forever when the implementation misses a wakeup,
//! so the scenario must feed events/advance time exactly as described.
//!
//! Driving patterns for Phase 2:
//! - event-driven resolution: `tokio::join!(wait_future, feeder_future)` — the
//!   wait registers first, the feeder then calls `handle_event`;
//! - deadline-driven resolution: poll the wait with a manual loop and
//!   `tokio::time::advance(Duration)` between polls.

mod common;

use std::time::Duration;

use adesk_core::{Observation, WindowId};
use adesk_observer::{Condition, ObserveSpec, ObserverService, QuietSpec, WaitSpec};

/// Feed `common::commit(2, 40, 7, 1, ..)` while the wait is pending.
/// Expect: `commits == 1`, `timed_out == false`, `quiet == false`,
/// `elapsed_ms == 40`, `last_commit_seq == 1`, `window_id == Some(WindowId(7))`,
/// and `changed_regions` containing the commit damage.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_change_resolves_on_first_counted_commit() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(1_000);
    let _observation: Observation = observer.wait_for_change(spec).await.expect("known window");
    todo!("Phase 2: drive the commit during the wait and assert the doc comment")
}

/// No event at all, `timeout_ms = 300`.
/// Expect: resolves at exactly 300 ms with `timed_out == true`, `commits == 0`,
/// `changed_regions == []`, `quiet == true` (window has been quiet since the
/// wait started), `elapsed_ms == 300`, `seq == 1` (the only event's watermark).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_change_times_out_without_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(300);
    let _observation: Observation = observer.wait_for_change(spec).await.expect("known window");
    todo!("Phase 2: advance 300 ms and assert the timeout observation")
}

/// Commits at 50 ms and 120 ms, then silence until the 400 ms timeout.
/// Expect: `timed_out == true`, `commits == 2`, `last_commit_seq == 2`,
/// `changed_regions` = union of both commits (simplified), `elapsed_ms == 400`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_change_timeout_reports_accumulated_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(400);
    let _observation: Observation = observer.wait_for_change(spec).await.expect("known window");
    todo!("Phase 2: feed two commits while waiting, assert accumulated counts")
}

/// `quiet_ms = 100`, one commit at 30 ms, silence after.
/// Expect: resolution at 130 ms, `timed_out == false`, `quiet == true`,
/// `commits == 1`, `elapsed_ms == 130`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_quiet_resolves_after_quiet_ms() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new().window(WindowId(7)).quiet_ms(100).timeout_ms(1_000);
    let _observation: Observation = observer.wait_for_quiet(spec).await.expect("known window");
    todo!("Phase 2: feed one commit, advance 130 ms, assert quiet resolution")
}

/// Commits every 60 ms with `quiet_ms = 100`.
/// Expect: the timer re-arms on every commit, so the wait only resolves
/// `quiet_ms` after the **last** commit (`elapsed_ms == last_commit_ts + 100`),
/// with `commits == 3` and `quiet == true`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_quiet_rearms_on_every_commit() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new().window(WindowId(7)).quiet_ms(100).timeout_ms(1_000);
    let _observation: Observation = observer.wait_for_quiet(spec).await.expect("known window");
    todo!("Phase 2: feed commits at 60/120/180 ms and assert re-arming")
}

/// Commits every 40 ms with `quiet_ms = 100`, `timeout_ms = 200`.
/// Expect: quiescence never holds for 100 ms, so the wait times out at 200 ms
/// with `timed_out == true`, `quiet == false`, `commits == 5`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn wait_for_quiet_times_out_while_commits_keep_arriving() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new().window(WindowId(7)).quiet_ms(100).timeout_ms(200);
    let _observation: Observation = observer.wait_for_quiet(spec).await.expect("known window");
    todo!("Phase 2: assert timeout while commits continue")
}

/// `Condition::Timeout` with `timeout_ms = 250`, commits at 20/60/140 ms.
/// Expect: the condition *is* the horizon, so `timed_out == false`,
/// `commits == 3`, `elapsed_ms == 250`, `quiet` reflects the 250 ms default
/// threshold (last commit 110 ms ago → `quiet == false`).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn observe_timeout_condition_samples_animation_window() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Timeout)
        .window(WindowId(7))
        .timeout_ms(250);
    let _observation: Observation = observer.observe(spec).await.expect("known window");
    todo!("Phase 2: sample three commits over 250 ms and assert the observation")
}

/// `Condition::Quiet { quiet_ms: 100 }` must behave exactly like
/// `wait_for_quiet(quiet_ms = 100)` (same machinery, same resolution time).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn observe_quiet_condition_matches_wait_for_quiet() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Quiet { quiet_ms: 100 })
        .window(WindowId(7))
        .timeout_ms(1_000);
    let _observation: Observation = observer.observe(spec).await.expect("known window");
    todo!("Phase 2: assert observe(quiet) == wait_for_quiet resolution")
}

/// `Condition::Change` resolves on a lifecycle event (not only commits):
/// feed `common::title_changed(2, 25, 7, Some(\"hi\"))` while waiting.
/// Expect: `commits == 0`, `title_changed == true`, `timed_out == false`,
/// `quiet == false`, `elapsed_ms == 25`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn observe_change_condition_resolves_on_lifecycle_event() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = ObserveSpec::new(Condition::Change)
        .window(WindowId(7))
        .timeout_ms(500);
    let _observation: Observation = observer.observe(spec).await.expect("known window");
    todo!("Phase 2: feed a title change during the wait and assert resolution")
}

/// The observation's `elapsed_ms` is measured in the event `ts_ms` domain:
/// with events at 0 and 40 ms and resolution at 90 ms, `elapsed_ms == 90`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn observation_elapsed_uses_event_clock_domain() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(90);
    let _observation: Observation = observer.wait_for_change(spec).await.expect("known window");
    let _ = Duration::from_millis(0);
    todo!("Phase 2: assert elapsed_ms == 90 in the event clock domain")
}
