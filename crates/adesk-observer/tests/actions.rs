//! Action registry: monotonic ids, causal anchors, and `last_input_at`.
//!
//! Phase 1: bodies stop at `todo!()`; the doc comment is the scenario.

mod common;

use adesk_core::{ActionId, Position, WindowId};
use adesk_observer::{ActionKind, ObserverService};

/// Ids start at 1 and increase by one per record, never reused — the
/// `after_action` contract depends on it.
#[test]
#[ignore = "Phase 2: implement ActionRegistry, then rewrite this body"]
fn action_ids_are_monotonic_and_start_at_one() {
    let registry = adesk_observer::ActionRegistry::new();
    let first: ActionId = registry.record(ActionKind::Click, None, None, 0, 0);
    let second: ActionId = registry.record(ActionKind::Keypress, None, None, 1, 1);
    assert_eq!(first, ActionId(1));
    assert_eq!(second, ActionId(2));
    todo!("Phase 2: assert monotonic allocation and len()/last()")
}

/// `record_action` captures the current watermark as `seq` and the clock as
/// `ts_ms`, plus kind/window/position, so `after_action` means "events after the
/// action was accepted".
#[test]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
fn record_action_captures_watermark_and_clock() {
    let observer = ObserverService::new();
    let id = observer.record_action(
        ActionKind::Click,
        Some(WindowId(7)),
        Some(Position::Normalized { x: 0.25, y: 0.75 }),
    );
    let record = observer.action(id).expect("recorded");
    assert_eq!(record.id, id);
    assert_eq!(record.kind, ActionKind::Click);
    assert_eq!(record.window_id, Some(WindowId(7)));
    todo!("Phase 2: assert record.seq == watermark at record time and ts_ms == clock")
}

/// Lookup, `seq_of`, `contains` and `records()` agree with the recorded data.
#[test]
#[ignore = "Phase 2: implement ActionRegistry, then rewrite this body"]
fn action_lookup_and_seq() {
    let registry = adesk_observer::ActionRegistry::new();
    let id = registry.record(ActionKind::Scroll, Some(WindowId(3)), None, 42, 7);
    assert_eq!(registry.seq_of(id), Some(42));
    assert!(registry.contains(id));
    assert_eq!(registry.get(id).map(|record| record.ts_ms), Some(7));
    todo!("Phase 2: assert records() ordering and unknown-id behaviour")
}

/// Input actions stamp `last_input_at` on the target window (and never on other
/// windows), which the inspector's `commit_timing` overlay uses.
#[test]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
fn input_actions_update_last_input_at() {
    let observer = ObserverService::new();
    let _id = observer.record_action(ActionKind::TypeText, Some(WindowId(7)), None);
    let state = observer.window_state(WindowId(7));
    let _ = state;
    todo!("Phase 2: assert last_input_at == record ts_ms for window 7 only")
}

/// Runtime-native operations are not input (design invariant 3): recording
/// `ActivateWindow`/`CloseWindow` must not touch `last_input_at`.
#[test]
#[ignore = "Phase 2: implement ObserverService, then rewrite this body"]
fn runtime_native_actions_do_not_touch_last_input_at() {
    let observer = ObserverService::new();
    let _id = observer.record_action(ActionKind::ActivateWindow, Some(WindowId(7)), None);
    let state = observer.window_state(WindowId(7));
    let _ = state;
    todo!("Phase 2: assert last_input_at stays None after activate_window")
}

/// The registry is shareable across tasks: cloning it keeps one id space.
#[test]
#[ignore = "Phase 2: implement ActionRegistry, then rewrite this body"]
fn registry_clones_share_one_id_space() {
    let registry = adesk_observer::ActionRegistry::new();
    let clone = registry.clone();
    let _first = registry.record(ActionKind::Click, None, None, 0, 0);
    let second = clone.record(ActionKind::Click, None, None, 0, 0);
    assert_eq!(second, ActionId(2));
    todo!("Phase 2: assert both clones observe the same records")
}
