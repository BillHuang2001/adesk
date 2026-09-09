//! Action registry: monotonic ids, causal anchors, and `last_input_at`.

mod common;

use adesk_core::{ActionId, Position, WindowId};
use adesk_observer::{ActionKind, ObserverService};

/// Ids start at 1 and increase by one per record, never reused — the
/// `after_action` contract depends on it.
#[test]
fn action_ids_are_monotonic_and_start_at_one() {
    let registry = adesk_observer::ActionRegistry::new();
    let first: ActionId = registry.record(ActionKind::Click, None, None, 0, 0);
    let second: ActionId = registry.record(ActionKind::Keypress, None, None, 1, 1);
    assert_eq!(first, ActionId(1));
    assert_eq!(second, ActionId(2));

    // Exactly one id per record, +1 each time, and ids are never reused.
    let third = registry.record(ActionKind::Scroll, None, None, 2, 2);
    assert_eq!(third, ActionId(3));
    assert_eq!(registry.len(), 3);
    assert!(!registry.is_empty());
    assert_eq!(registry.last().map(|record| record.id), Some(third));
    assert_eq!(
        registry
            .records()
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        vec![ActionId(1), ActionId(2), ActionId(3)]
    );
    assert!(
        !registry.contains(ActionId(4)),
        "the next id was already handed out"
    );
}

/// `record_action` captures the current watermark as `seq` and the clock as
/// `ts_ms`, plus kind/window/position, so `after_action` means "events after the
/// action was accepted".
#[test]
fn record_action_captures_watermark_and_clock() {
    let observer = ObserverService::new();
    let before = observer.now_ms();
    let id = observer.record_action(
        ActionKind::Click,
        Some(WindowId(7)),
        Some(Position::Normalized { x: 0.25, y: 0.75 }),
    );
    let record = observer.action(id).expect("recorded");
    assert_eq!(record.id, id);
    assert_eq!(record.kind, ActionKind::Click);
    assert_eq!(record.window_id, Some(WindowId(7)));
    assert_eq!(
        record.position,
        Some(Position::Normalized { x: 0.25, y: 0.75 })
    );

    // No event was processed, so the causal anchor is watermark 0 and the
    // observer's own clock (anchored at ts_ms = 0, never moved by an event).
    assert_eq!(observer.watermark(), 0);
    assert_eq!(
        record.seq,
        observer.watermark(),
        "seq is the watermark at record time"
    );
    assert_eq!(
        record.ts_ms,
        observer.now_ms(),
        "ts_ms is the clock at record time"
    );
    assert!(record.ts_ms >= before, "the clock never runs backwards");
    assert_eq!(observer.action_seq(id), Some(record.seq));
}

/// Lookup, `seq_of`, `contains` and `records()` agree with the recorded data.
#[test]
fn action_lookup_and_seq() {
    let registry = adesk_observer::ActionRegistry::new();
    let id = registry.record(ActionKind::Scroll, Some(WindowId(3)), None, 42, 7);
    assert_eq!(registry.seq_of(id), Some(42));
    assert!(registry.contains(id));
    assert_eq!(registry.get(id).map(|record| record.ts_ms), Some(7));

    // records() exposes every record in ascending id order, and last() is newest.
    let second = registry.record(ActionKind::Keypress, None, None, 43, 9);
    let records = registry.records();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records.iter().map(|record| record.id).collect::<Vec<_>>(),
        vec![id, second]
    );
    assert_eq!(records[0], registry.get(id).expect("recorded"));
    assert_eq!(registry.last(), Some(records[1].clone()));

    // Unknown ids are simply absent: no error, no fabricated record.
    let unknown = ActionId(second.0 + 1);
    assert!(!registry.contains(unknown));
    assert!(registry.get(unknown).is_none());
    assert!(registry.seq_of(unknown).is_none());
    assert!(!registry.contains(ActionId(0)));
}

/// Input actions stamp `last_input_at` on the target window (and never on other
/// windows), which the inspector's `commit_timing` overlay uses.
#[test]
fn input_actions_update_last_input_at() {
    let observer = ObserverService::new();
    // A second window exists so "never on other windows" is observable.
    observer.handle_event(&common::created(1, 0, 9));

    let _id = observer.record_action(ActionKind::TypeText, Some(WindowId(7)), None);
    let record = observer.action(_id).expect("recorded");
    let state = observer.window_state(WindowId(7));
    let _ = state;
    let state = state.expect("the target window is tracked");
    assert_eq!(state.last_input_at, Some(record.ts_ms));

    let other = observer
        .window_state(WindowId(9))
        .expect("the other window is tracked");
    assert_eq!(
        other.last_input_at, None,
        "input is scoped to its target window"
    );
}

/// Runtime-native operations are not input (design invariant 3): recording
/// `ActivateWindow`/`CloseWindow` must not touch `last_input_at`.
#[test]
fn runtime_native_actions_do_not_touch_last_input_at() {
    let observer = ObserverService::new();
    let _id = observer.record_action(ActionKind::ActivateWindow, Some(WindowId(7)), None);
    let state = observer.window_state(WindowId(7));
    let _ = state;
    let state = state.expect("created on demand");
    assert_eq!(
        state.last_input_at, None,
        "activate_window is a compositor state change, not synthesized input"
    );
    assert_eq!(_id, ActionId(1), "the action is still recorded");
    assert!(!ActionKind::ActivateWindow.is_input());

    let _close = observer.record_action(ActionKind::CloseWindow, Some(WindowId(7)), None);
    assert_eq!(_close, ActionId(2));
    assert!(!ActionKind::CloseWindow.is_input());
    assert_eq!(
        observer
            .window_state(WindowId(7))
            .expect("still tracked")
            .last_input_at,
        None,
        "close_window is a compositor state change, not synthesized input"
    );
}

/// The registry is shareable across tasks: cloning it keeps one id space.
#[test]
fn registry_clones_share_one_id_space() {
    let registry = adesk_observer::ActionRegistry::new();
    let clone = registry.clone();
    let _first = registry.record(ActionKind::Click, None, None, 0, 0);
    let second = clone.record(ActionKind::Click, None, None, 0, 0);
    assert_eq!(second, ActionId(2));

    // The clone continues the same sequence and sees every record.
    let third = clone.record(ActionKind::Scroll, None, None, 2, 2);
    assert_eq!(third, ActionId(3));
    assert_eq!(registry.len(), 3);
    assert_eq!(clone.len(), 3);
    assert_eq!(registry.records(), clone.records());
    assert_eq!(registry.get(_first), clone.get(_first));
    assert!(registry.contains(third));
    assert!(clone.contains(_first));
    assert_eq!(registry.last().map(|record| record.id), Some(third));
    assert_eq!(clone.last().map(|record| record.id), Some(third));
}
