//! Public API smoke test: the surface is constructible and documented.
//!
//! Unlike the behaviour tests (which are `#[ignore]`d until Phase 2), this test
//! runs today: it pins the public API shape — spec builders, enum mappings,
//! snapshot structs — and keeps `cargo check --all-targets` honest.

mod common;

use adesk_core::{Position, Rect, WindowId};
use adesk_observer::{
    ActionKind, ActionRecord, ActionRegistry, Condition, ObserveSpec, ObserverConfig,
    ObserverService, PendingObservation, QuietSpec, StateSnapshot, WaitSpec, WindowSnapshot,
    WindowTemporalState, DEFAULT_JOURNAL_CAPACITY, DEFAULT_QUIET_MS, DEFAULT_TIMEOUT_MS,
};

#[test]
fn wait_spec_builders_and_defaults() {
    let spec = WaitSpec::new();
    assert_eq!(spec.window_id, None);
    assert_eq!(spec.since_commit, None);
    assert_eq!(spec.timeout_ms, DEFAULT_TIMEOUT_MS);

    let spec = WaitSpec::new()
        .window(WindowId(7))
        .since_commit(42)
        .timeout_ms(1234);
    assert_eq!(spec.window_id, Some(WindowId(7)));
    assert_eq!(spec.since_commit, Some(42));
    assert_eq!(spec.timeout_ms, 1234);
}

#[test]
fn quiet_spec_builders_and_defaults() {
    let spec = QuietSpec::new();
    assert_eq!(spec.quiet_ms, DEFAULT_QUIET_MS);
    assert_eq!(spec.timeout_ms, DEFAULT_TIMEOUT_MS);
    assert_eq!(spec.after_action, None);

    let spec = QuietSpec::new()
        .window(WindowId(3))
        .quiet_ms(80)
        .timeout_ms(900)
        .after_action(adesk_core::ActionId(5));
    assert_eq!(spec.window_id, Some(WindowId(3)));
    assert_eq!(spec.quiet_ms, 80);
    assert_eq!(spec.timeout_ms, 900);
    assert_eq!(spec.after_action, Some(adesk_core::ActionId(5)));
}

#[test]
fn observe_spec_builders_and_conditions() {
    let spec = ObserveSpec::new(Condition::Change);
    assert_eq!(spec.until, Condition::Change);
    assert_eq!(spec.timeout_ms, DEFAULT_TIMEOUT_MS);

    let spec = ObserveSpec::new(Condition::Quiet { quiet_ms: 120 })
        .window(WindowId(1))
        .timeout_ms(2000);
    assert_eq!(spec.until, Condition::Quiet { quiet_ms: 120 });
    assert_eq!(spec.window_id, Some(WindowId(1)));
    assert_eq!(spec.timeout_ms, 2000);

    assert_eq!(Condition::Change.quiet_threshold_ms(), DEFAULT_QUIET_MS);
    assert_eq!(Condition::Timeout.quiet_threshold_ms(), DEFAULT_QUIET_MS);
    assert_eq!(
        Condition::Quiet { quiet_ms: 33 }.quiet_threshold_ms(),
        33
    );
}

#[test]
fn action_kind_vocabulary_matches_agp() {
    assert_eq!(ActionKind::ALL.len(), 13);
    assert_eq!(ActionKind::Click.as_str(), "click");
    assert_eq!(ActionKind::TypeText.as_str(), "type_text");
    assert_eq!(ActionKind::ActivateWindow.as_str(), "activate_window");
    assert!(ActionKind::Click.is_input());
    assert!(!ActionKind::ActivateWindow.is_input());
    assert!(!ActionKind::CloseWindow.is_input());
    assert!(ActionKind::ALL.iter().all(|kind| !kind.as_str().is_empty()));
}

#[test]
fn registry_and_snapshot_types_are_constructible() {
    // Construction only: method bodies are `todo!()` until Phase 2, so this test
    // must not call them (behaviour lives in the `#[ignore]`d test files).
    let _registry = ActionRegistry::new();
    let _registry_clone = _registry.clone();

    let record = ActionRecord {
        id: adesk_core::ActionId(1),
        kind: ActionKind::Click,
        window_id: Some(WindowId(7)),
        position: Some(Position::Normalized { x: 0.5, y: 0.5 }),
        seq: 100,
        ts_ms: 10,
    };
    assert_eq!(record.kind, ActionKind::Click);

    let state = WindowTemporalState {
        window_id: WindowId(7),
        last_commit_seq: 3,
        last_commit_at: 20,
        last_damage: adesk_core::Region::from_rect(common::rect(0, 0, 4, 4)),
        last_meaningful_change_at: 20,
        last_input_at: Some(15),
        quiet_since: Some(20),
        commit_count: 2,
        pending_observation: Some(PendingObservation {
            since_seq: 90,
            started_at: 15,
        }),
        geometry: Some(Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        }),
        state_uncertain: false,
    };
    assert!(state.is_quiet(300, 250));
    assert!(!state.is_quiet(100, 250));

    let snapshot = StateSnapshot {
        seq: 120,
        ts_ms: 25,
        windows: vec![WindowSnapshot {
            window_id: WindowId(7),
            last_commit_seq: 3,
            geometry: common::rect(0, 0, 1280, 800),
            popup_count: 0,
        }],
    };
    assert_eq!(snapshot.windows.len(), 1);

    let config = ObserverConfig::default();
    assert_eq!(config.journal_capacity, DEFAULT_JOURNAL_CAPACITY);
    assert_eq!(config.default_quiet_ms, DEFAULT_QUIET_MS);
    // Reference the constructors without invoking the `todo!()` bodies.
    let _constructors: [fn() -> ObserverService; 1] = [ObserverService::new];
    let _with_config: fn(ObserverConfig) -> ObserverService = ObserverService::with_config;
}
