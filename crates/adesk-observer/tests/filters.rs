//! Filter semantics: `window_id`, `after_action`, `since_commit`, and the
//! shape of the resulting [`adesk_core::Observation`] (AGP §5.4).

mod common;

use adesk_core::{ActionId, ErrorCode, WindowId};
use adesk_observer::{
    Error, ObserverService, QuietSpec, StateSnapshot, WaitSpec, WindowSnapshot,
};

/// Events before the action must not count.
/// `record_action` at watermark 3, then commit seq 4 and commit seq 5.
/// Expect: `after_action == Some(id)`, `commits == 2`, `last_commit_seq == 5`.
#[tokio::test(start_paused = true)]async fn after_action_excludes_earlier_events() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    // Two pre-action commits advance the watermark to 3.
    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(0, 0, 4, 4)]));
    observer.handle_event(&common::commit(3, 20, 7, 2, &[common::rect(0, 0, 4, 4)]));
    let action = observer.record_action(
        adesk_observer::ActionKind::Click,
        Some(WindowId(7)),
        None,
    );
    assert_eq!(action, ActionId(1), "the first recorded action is 1");
    assert_eq!(
        observer.action_seq(action),
        Some(3),
        "the action is anchored at watermark 3"
    );

    let spec = QuietSpec::new().window(WindowId(7)).after_action(ActionId(1));
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_quiet(spec).await }
    });
    tokio::task::yield_now().await;

    // Commits 4 and 5 happen after the action was accepted: both count.
    observer.handle_event(&common::commit(4, 30, 7, 4, &[common::rect(0, 0, 4, 4)]));
    observer.handle_event(&common::commit(5, 40, 7, 5, &[common::rect(0, 0, 4, 4)]));

    let observation = handle.await.expect("waiter task").expect("known window");
    assert!(!observation.timed_out);
    assert_eq!(observation.after_action, Some(ActionId(1)));
    assert_eq!(
        observation.commits, 2,
        "only the commits after the action count"
    );
    assert_eq!(observation.last_commit_seq, 5);
}

/// `after_action` naming an id the observer never allocated must fail with
/// `Error::UnknownAction` (mapped to AGP `invalid_request`), never silently
/// produce an observation.
#[tokio::test(start_paused = true)]async fn after_action_unknown_action_is_error() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = QuietSpec::new()
        .window(WindowId(7))
        .after_action(ActionId(999));
    let error: Error = observer.wait_for_quiet(spec).await.expect_err("unknown action");
    assert!(matches!(error, Error::UnknownAction(ActionId(999))));

    // Crate-boundary mapping: an unknown action is a bad request, not a wait error.
    let mapped: adesk_core::Error = error.into();
    assert_eq!(mapped.code, ErrorCode::InvalidRequest);
    assert_eq!(mapped.code.as_str(), "invalid_request");
}

/// `since_commit = 1`: commits 1 and 2 exist before the wait, commit 3 during.
/// Expect: only commit 3 counts (`commits == 1`, `last_commit_seq == 3`);
/// a lifecycle event during the wait still counts (not commit-numbered).
#[tokio::test(start_paused = true)]async fn since_commit_filters_commits_only() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    // Commits 1 and 2 already exist before the wait starts.
    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(3, 20, 7, 2, &[common::rect(0, 0, 2, 2)]));
    let spec = WaitSpec::new().window(WindowId(7)).since_commit(1).timeout_ms(100);
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_change(spec).await }
    });
    tokio::task::yield_now().await;

    // Commit 3 counts; the title event is not commit-numbered and counts too.
    observer.handle_event(&common::commit(4, 30, 7, 3, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::title_changed(5, 40, 7, Some("titled")));

    let observation = handle.await.expect("waiter task").expect("known window");
    assert!(!observation.timed_out);
    assert_eq!(
        observation.commits, 1,
        "commits 1 and 2 are at or below since_commit"
    );
    assert_eq!(observation.last_commit_seq, 3);
    assert!(
        observation.title_changed,
        "lifecycle events are not commit-numbered and always count"
    );
}

/// Two windows, commits on both; a window-filtered wait must only count its own
/// window: `commits == 1`, `changed_regions` from that window only,
/// `new_windows`/`destroyed_windows` only for the filtered window.
#[tokio::test(start_paused = true)]async fn window_filter_ignores_other_windows() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    observer.handle_event(&common::created(2, 0, 8));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_change(spec).await }
    });
    tokio::task::yield_now().await;

    // Activity on windows 8 and 9 must not count for a wait filtered on window 7.
    observer.handle_event(&common::commit(3, 10, 8, 1, &[common::rect(0, 0, 5, 5)]));
    observer.handle_event(&common::title_changed(4, 20, 8, Some("other")));
    observer.handle_event(&common::commit(5, 30, 7, 1, &[common::rect(10, 10, 4, 4)]));
    observer.handle_event(&common::created(6, 40, 9));
    observer.handle_event(&common::destroyed(7, 50, 8));

    let observation = handle.await.expect("waiter task").expect("known window");
    assert!(!observation.timed_out);
    assert_eq!(observation.window_id, Some(WindowId(7)));
    assert_eq!(observation.commits, 1, "window 8's commit does not count");
    assert_eq!(
        observation.changed_regions,
        vec![common::rect(10, 10, 4, 4)],
        "damage of other windows is not reported"
    );
    assert_eq!(observation.last_commit_seq, 1, "window 7's commit watermark");
    assert!(
        observation.new_windows.is_empty(),
        "window 9 was created, but it is not the filtered window"
    );
    assert!(
        observation.destroyed_windows.is_empty(),
        "window 8 was destroyed, but it is not the filtered window"
    );
    assert!(
        !observation.title_changed,
        "window 8's title change is not the filtered window"
    );
}

/// A wait on a window the observer never saw is `Error::UnknownWindow`
/// (AGP `unknown_window`), not a timeout.
#[tokio::test(start_paused = true)]async fn unknown_window_is_error() {
    let observer = ObserverService::new();
    let spec = WaitSpec::new().window(WindowId(99)).timeout_ms(50);
    let error: Error = observer
        .wait_for_change(spec)
        .await
        .expect_err("unknown window");
    assert!(matches!(error, Error::UnknownWindow(WindowId(99))));

    // Crate-boundary mapping: an unknown window is AGP `unknown_window`.
    let mapped: adesk_core::Error = error.into();
    assert_eq!(mapped.code, ErrorCode::UnknownWindow);
    assert_eq!(mapped.code.as_str(), "unknown_window");
}

/// Damage union: commits with overlapping rects `(0,0,10,10)` and `(5,5,10,10)`
/// plus a disjoint `(50,50,4,4)`.
/// Expect: `changed_regions` is `Region::simplified()` of the union, clipped to
/// the window geometry when known (via `resync`), sorted by `(y, x, h, w)`.
#[tokio::test(start_paused = true)]async fn changed_regions_are_union_simplified_and_clipped() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    // Geometry must be known *before* the commits: damage is clipped to it.
    let geometry = common::rect(0, 0, 52, 52);
    let report = observer.resync(StateSnapshot {
        seq: 1,
        ts_ms: 0,
        windows: vec![WindowSnapshot {
            window_id: WindowId(7),
            last_commit_seq: 0,
            geometry,
            popup_count: 0,
        }],
    });
    assert_eq!(report.snapshot_seq, 1);
    assert_eq!(
        observer.window_state(WindowId(7)).unwrap().geometry,
        Some(geometry),
        "the snapshot supplies the window geometry"
    );

    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_change(spec).await }
    });
    tokio::task::yield_now().await;

    // The disjoint rect comes first so the sort is observable, not just insertion
    // order; it is also the rect that the geometry clips (4x4 -> 2x2).
    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(50, 50, 4, 4)]));
    observer.handle_event(&common::commit(3, 20, 7, 2, &[common::rect(0, 0, 10, 10)]));
    observer.handle_event(&common::commit(4, 30, 7, 3, &[common::rect(5, 5, 10, 10)]));

    let observation = handle.await.expect("waiter task").expect("known window");
    assert!(!observation.timed_out);
    assert_eq!(observation.commits, 3);

    let expected = common::region(&[
        common::rect(50, 50, 4, 4),
        common::rect(0, 0, 10, 10),
        common::rect(5, 5, 10, 10),
    ])
    .clip(&geometry)
    .simplified();
    assert_eq!(observation.changed_regions, expected);
    assert_eq!(
        observation.changed_regions,
        vec![common::rect(0, 0, 15, 15), common::rect(50, 50, 2, 2)],
        "coalesced, clipped to the geometry, sorted by (y, x, h, w)"
    );
}

/// One commit plus `title_changed`, `focus` to the window, `popup_appeared` and
/// `popup_disappeared`.
/// Expect: `title_changed == true`, `focus_changed == Some(true)`,
/// `popups_appeared == [3]`, `popups_disappeared == [3]`.
#[tokio::test(start_paused = true)]async fn title_focus_and_popup_flags_are_reported() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(100);
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_change(spec).await }
    });
    tokio::task::yield_now().await;

    observer.handle_event(&common::commit(2, 10, 7, 1, &[common::rect(0, 0, 4, 4)]));
    observer.handle_event(&common::title_changed(3, 20, 7, Some("hello")));
    observer.handle_event(&common::focus(4, 30, Some(7)));
    observer.handle_event(&common::popup_appeared(5, 40, 7, 3));
    observer.handle_event(&common::popup_disappeared(6, 50, 7, 3));

    let observation = handle.await.expect("waiter task").expect("known window");
    assert!(!observation.timed_out);
    assert!(observation.title_changed);
    assert_eq!(observation.focus_changed, Some(true));
    assert_eq!(observation.popups_appeared, vec![3]);
    assert_eq!(observation.popups_disappeared, vec![3]);
    assert_eq!(observation.commits, 1);
    assert_eq!(observation.changed_regions, vec![common::rect(0, 0, 4, 4)]);
}

/// Global (unfiltered) wait across two windows: one created, one destroyed.
/// Expect: `new_windows == [WindowId(8)]`, `destroyed_windows == [WindowId(7)]`,
/// `last_commit_seq` = global max, `window_id == None`.
#[tokio::test(start_paused = true)]async fn new_and_destroyed_windows_are_reported_globally() {
    let observer = ObserverService::new();
    observer.handle_event(&common::created(1, 0, 7));
    let spec = WaitSpec::new().timeout_ms(100);
    let handle = tokio::spawn({
        let observer = observer.clone();
        async move { observer.wait_for_change(spec).await }
    });
    tokio::task::yield_now().await;

    observer.handle_event(&common::created(2, 10, 8));
    observer.handle_event(&common::commit(3, 20, 7, 2, &[common::rect(0, 0, 2, 2)]));
    observer.handle_event(&common::commit(4, 30, 8, 5, &[common::rect(10, 10, 2, 2)]));
    observer.handle_event(&common::destroyed(5, 40, 7));

    let observation = handle.await.expect("waiter task").expect("unfiltered wait");
    assert!(!observation.timed_out);
    assert_eq!(observation.window_id, None);
    assert_eq!(observation.new_windows, vec![WindowId(8)]);
    assert_eq!(observation.destroyed_windows, vec![WindowId(7)]);
    assert_eq!(
        observation.last_commit_seq, 5,
        "the global maximum commit sequence across all windows"
    );
    assert_eq!(observation.commits, 2);
    assert_eq!(
        observation.changed_regions,
        vec![common::rect(0, 0, 2, 2), common::rect(10, 10, 2, 2)]
    );
}
