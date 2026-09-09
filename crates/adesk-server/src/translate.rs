//! Bridges between sibling crates' overlapping types.
//!
//! Each pair of sibling crates deliberately avoids depending on the other, so
//! the server owns the conversions. Every function here is total and lossless
//! for the fields the protocol uses; anything a bridge cannot represent is
//! documented on the function.

use adesk_compositor::{RendererName, StateSnapshot as CompositorSnapshot};
use adesk_core::{Observation, Rect};
use adesk_inspector::ActionMarker;
use adesk_observer::{
    ActionKind as ObserverActionKind, ActionRecord, Condition as ObserverCondition,
    StateSnapshot as ObserverSnapshot,
};
use adesk_proto::{Condition as ProtoCondition, ImagePayload, ObserveResult};

/// `adesk_compositor::StateSnapshot` → `adesk_observer::StateSnapshot`.
///
/// The observer's snapshot carries per-window `last_commit_seq`, geometry and
/// `popup_count`; the compositor snapshot has no commit history, so the
/// conversion seeds `last_commit_seq = 0` and leaves window ordering intact.
/// Used only for lag resync — the observer treats seeded windows as uncertain.
pub fn observer_snapshot(snapshot: &CompositorSnapshot) -> ObserverSnapshot {
    todo!()
}

/// `adesk_proto::Condition` → `adesk_observer::Condition` (same three
/// variants, distinct crates).
pub fn observer_condition(condition: ProtoCondition) -> ObserverCondition {
    todo!()
}

/// `adesk_observer::ActionKind` → `adesk_inspector::ActionKind`.
///
/// The two vocabularies are identical (11 input methods + `ActivateWindow` +
/// `CloseWindow`); this mapping exists only because neither crate depends on
/// the other.
pub fn inspector_action_kind(kind: ObserverActionKind) -> adesk_inspector::ActionKind {
    todo!()
}

/// `adesk_compositor::RendererName` → `adesk_proto::RendererKind` for `ping`.
pub fn proto_renderer(name: RendererName) -> adesk_proto::RendererKind {
    todo!()
}

/// Action-registry entry → inspector marker.
///
/// `geometry` is the action window's output-coordinate rect: the record stores
/// a **window-relative** [`adesk_core::Position`], which is resolved through it.
/// `now_ms` (server monotonic clock) computes `age_ms = now_ms - record.ts_ms`.
pub fn action_marker(record: &ActionRecord, geometry: Option<Rect>, now_ms: u64) -> ActionMarker {
    todo!()
}

/// `adesk_core::Observation` + optional image → the AGP result of `observe`,
/// `wait_for_change` and `wait_for_quiet` (§5.4).
pub fn observe_result(observation: Observation, image: Option<ImagePayload>) -> ObserveResult {
    todo!()
}
