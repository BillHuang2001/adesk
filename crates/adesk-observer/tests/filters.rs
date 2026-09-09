//! Filter semantics: `window_id`, `after_action`, `since_commit`, and the
//! shape of the resulting [`adesk_core::Observation`] (AGP §5.4).
//!
//! Phase 1: bodies stop at `todo!()`; the doc comment is the scenario.

mod common;

use adesk_core::{ActionId, WindowId};
use adesk_observer::{Error, ObserverService, QuietSpec, WaitSpec};

/// Events before the action must not count.
/// `record_action` at watermark 3, then commit seq 4 and commit seq 5.
/// Expect: `after_action == Some(id)`, `commits == 2`, `last_commit_seq == 5`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn after_action_excludes_earlier_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let _action = observer.record_action(
        adesk_observer::ActionKind::Click,
        Some(WindowId(7)),
        None,
    );
    let spec = QuietSpec::new().window(WindowId(7)).after_action(ActionId(1));
    let _ = observer.wait_for_quiet(spec).await;
    todo!("Phase 2: feed commits before/after the action and assert filtering")
}

/// `after_action` naming an id the observer never allocated must fail with
/// `Error::UnknownAction` (mapped to AGP `invalid_request`), never silently
/// produce an observation.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn after_action_unknown_action_is_error() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new()
        .window(WindowId(7))
        .after_action(ActionId(999));
    let error: Error = observer.wait_for_quiet(spec).await.expect_err("unknown action");
    assert!(matches!(error, Error::UnknownAction(ActionId(999))));
    todo!("Phase 2: assert the error mapping to adesk_core::Error")
}

/// `since_commit = 1`: commits 1 and 2 exist before the wait, commit 3 during.
/// Expect: only commit 3 counts (`commits == 1`, `last_commit_seq == 3`);
/// a lifecycle event during the wait still counts (not commit-numbered).
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn since_commit_filters_commits_only() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).since_commit(1).timeout_ms(100);
    let _ = observer.wait_for_change(spec).await;
    todo!("Phase 2: assert commit_seq filtering and lifecycle pass-through")
}

/// Two windows, commits on both; a window-filtered wait must only count its own
/// window: `commits == 1`, `changed_regions` from that window only,
/// `new_windows`/`destroyed_windows` only for the filtered window.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn window_filter_ignores_other_windows() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::created(2, 0, 8));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let _ = observer.wait_for_change(spec).await;
    todo!("Phase 2: assert per-window isolation of counts and regions")
}

/// A wait on a window the observer never saw is `Error::UnknownWindow`
/// (AGP `unknown_window`), not a timeout.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn unknown_window_is_error() {
    let observer = ObserverService::new();
    let spec = WaitSpec::new().window(WindowId(99)).timeout_ms(50);
    let error: Error = observer
        .wait_for_change(spec)
        .await
        .expect_err("unknown window");
    assert!(matches!(error, Error::UnknownWindow(WindowId(99))));
    todo!("Phase 2: assert the error mapping to adesk_core::Error")
}

/// Damage union: commits with overlapping rects `(0,0,10,10)` and `(5,5,10,10)`
/// plus a disjoint `(50,50,4,4)`.
/// Expect: `changed_regions` is `Region::simplified()` of the union, clipped to
/// the window geometry when known (via `resync`), sorted by `(y, x, h, w)`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn changed_regions_are_union_simplified_and_clipped() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let _ = observer.wait_for_change(spec).await;
    todo!("Phase 2: assert coalesced/simplified damage and clipping")
}

/// One commit plus `title_changed`, `focus` to the window, `popup_appeared` and
/// `popup_disappeared`.
/// Expect: `title_changed == true`, `focus_changed == Some(true)`,
/// `popups_appeared == [3]`, `popups_disappeared == [3]`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn title_focus_and_popup_flags_are_reported() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let _ = observer.wait_for_change(spec).await;
    todo!("Phase 2: assert the auxiliary observation fields")
}

/// Global (unfiltered) wait across two windows: one created, one destroyed.
/// Expect: `new_windows == [WindowId(8)]`, `destroyed_windows == [WindowId(7)]`,
/// `last_commit_seq` = global max, `window_id == None`.
#[tokio::test(start_paused = true)]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
async fn new_and_destroyed_windows_are_reported_globally() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().timeout_ms(100);
    let _ = observer.wait_for_change(spec).await;
    todo!("Phase 2: assert global lifecycle reporting")
}
