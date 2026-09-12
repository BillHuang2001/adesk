//! Bridges between sibling crates' overlapping types.
//!
//! Each pair of sibling crates deliberately avoids depending on the other, so
//! the server owns the conversions. Every function here is total and lossless
//! for the fields the protocol uses; anything a bridge cannot represent is
//! documented on the function.

use adesk_compositor::{RendererName, StateSnapshot as CompositorSnapshot};
use adesk_core::{Observation, Point, Rect};
use adesk_inspector::ActionMarker;
use adesk_observer::{
    ActionKind as ObserverActionKind, ActionRecord, Condition as ObserverCondition,
    StateSnapshot as ObserverSnapshot, WindowSnapshot,
};
use adesk_proto::{Condition as ProtoCondition, ImagePayload, ObserveResult};
use adesk_recorder::EncoderKind;
use adesk_viewer_proto::RecordingEncoder;

/// `adesk_compositor::StateSnapshot` → `adesk_observer::StateSnapshot`.
///
/// Every field the observer's resync input can express is copied verbatim:
/// `seq`, `ts_ms` and, per window, `window_id`, `last_commit_seq`, `geometry`
/// and `popup_count`. Window order is preserved (the compositor's creation
/// order).
///
/// `last_commit_seq` is **not** seeded to zero: [`adesk_core::WindowInfo`]
/// carries the compositor's per-surface-tree commit counter, which is the same
/// counter the observer counts from `RuntimeEvent::SurfaceCommit`, so a resync
/// compares like with like. A window the observer missed entirely is seeded
/// with the real watermark (and flagged `state_uncertain` by
/// [`adesk_observer::ObserverService::resync`]); seeding zero would instead
/// under-report `Observation::last_commit_seq` until that window's next commit.
///
/// Documented mismatches, deliberately hidden here:
///
/// - `adesk_observer::StateSnapshot::windows` is documented as "ordered by
///   window id", while the compositor lists windows in creation order. Resync
///   is order-insensitive (covered ids in a `BTreeSet`, per-window lookups), so
///   the difference is not observable; creation order is the only order the
///   compositor can promise.
/// - `active_window_id` and `keyboard_focus` have no counterpart in the resync
///   input and are dropped: the observer learns focus from events, and a resync
///   only reconciles existence, geometry and commit watermarks.
///
/// Used only for lag resync — the observer treats every window in the snapshot
/// as uncertain until the next event for that window.
pub fn observer_snapshot(snapshot: &CompositorSnapshot) -> ObserverSnapshot {
    ObserverSnapshot {
        seq: snapshot.seq,
        ts_ms: snapshot.ts_ms,
        windows: snapshot
            .windows
            .iter()
            .map(|window| WindowSnapshot {
                window_id: window.id,
                last_commit_seq: window.last_commit_seq,
                geometry: window.geometry,
                popup_count: window.popup_count,
            })
            .collect(),
    }
}

/// `adesk_proto::Condition` → `adesk_observer::Condition` (same three
/// variants, distinct crates).
pub fn observer_condition(condition: ProtoCondition) -> ObserverCondition {
    match condition {
        ProtoCondition::Quiet { quiet_ms } => ObserverCondition::Quiet { quiet_ms },
        ProtoCondition::Change => ObserverCondition::Change,
        ProtoCondition::Timeout => ObserverCondition::Timeout,
    }
}

/// `adesk_observer::ActionKind` → `adesk_inspector::ActionKind`.
///
/// The two vocabularies are identical (11 input methods + `ActivateWindow` +
/// `CloseWindow`); this mapping exists only because neither crate depends on
/// the other.
pub fn inspector_action_kind(kind: ObserverActionKind) -> adesk_inspector::ActionKind {
    match kind {
        ObserverActionKind::PointerMove => adesk_inspector::ActionKind::PointerMove,
        ObserverActionKind::Click => adesk_inspector::ActionKind::Click,
        ObserverActionKind::DoubleClick => adesk_inspector::ActionKind::DoubleClick,
        ObserverActionKind::MouseDown => adesk_inspector::ActionKind::MouseDown,
        ObserverActionKind::MouseUp => adesk_inspector::ActionKind::MouseUp,
        ObserverActionKind::Scroll => adesk_inspector::ActionKind::Scroll,
        ObserverActionKind::Drag => adesk_inspector::ActionKind::Drag,
        ObserverActionKind::Keypress => adesk_inspector::ActionKind::Keypress,
        ObserverActionKind::KeyDown => adesk_inspector::ActionKind::KeyDown,
        ObserverActionKind::KeyUp => adesk_inspector::ActionKind::KeyUp,
        ObserverActionKind::TypeText => adesk_inspector::ActionKind::TypeText,
        ObserverActionKind::ActivateWindow => adesk_inspector::ActionKind::ActivateWindow,
        ObserverActionKind::CloseWindow => adesk_inspector::ActionKind::CloseWindow,
    }
}

/// `adesk_compositor::RendererName` → `adesk_proto::RendererKind` for `ping`.
pub fn proto_renderer(name: RendererName) -> adesk_proto::RendererKind {
    match name {
        RendererName::Gl => adesk_proto::RendererKind::Gl,
        RendererName::Pixman => adesk_proto::RendererKind::Pixman,
    }
}

/// `adesk_viewer_proto::RecordingEncoder` → `adesk_recorder::EncoderKind`.
///
/// Both enums name the same three choices (`Auto`/`Software`/`Gpu`); this
/// mapping exists only because neither crate depends on the other. The recording
/// backend pairs it with [`adesk_recorder::detect`] so the resolved backend — and
/// therefore the file extension [`adesk_recorder::suggest_extension`] reports —
/// matches the encoder actually used.
pub fn recorder_encoder(encoder: RecordingEncoder) -> EncoderKind {
    match encoder {
        RecordingEncoder::Auto => EncoderKind::Auto,
        RecordingEncoder::Software => EncoderKind::Software,
        RecordingEncoder::Gpu => EncoderKind::Gpu,
    }
}

/// `adesk_recorder::EncoderKind` → `adesk_viewer_proto::RecordingEncoder`.
///
/// The exact inverse of [`recorder_encoder`], kept beside it so the pairing is
/// total in both directions and one round-trip test covers the pair. The
/// recording status reports the *resolved backend's label* (`Recorder::encoder_name`)
/// rather than a re-derived enum, so this direction has no production caller
/// today.
pub fn proto_encoder(kind: EncoderKind) -> RecordingEncoder {
    match kind {
        EncoderKind::Auto => RecordingEncoder::Auto,
        EncoderKind::Software => RecordingEncoder::Software,
        EncoderKind::Gpu => RecordingEncoder::Gpu,
    }
}

/// Action-registry entry → inspector marker.
///
/// `geometry` is the action window's output-coordinate rect: the record stores
/// a **window-relative** [`adesk_core::Position`], which is resolved through it
/// with [`adesk_core::Position::resolve`] — against a rect at the origin with
/// the window's size, then translated by the geometry origin — so `Pixels` are
/// never mistaken for output coordinates and the output origin is never
/// hard-coded (the same rule `adesk-wm`'s position resolution follows).
///
/// `position` is `None` when the record carries no position (keyboard and
/// runtime-native actions) **or** when the window geometry is unknown: the
/// marker is output-relative and the overlay must not invent an origin it does
/// not have.
///
/// `now_ms` (server monotonic clock) computes `age_ms = now_ms - record.ts_ms`,
/// saturating at `0` if the record is newer than `now_ms`.
pub fn action_marker(record: &ActionRecord, geometry: Option<Rect>, now_ms: u64) -> ActionMarker {
    let position = match (record.position, geometry) {
        (Some(position), Some(geometry)) => {
            let local = position.resolve(Rect::from_size(geometry.size()));
            Some(Point::new(
                geometry.x.saturating_add(local.x),
                geometry.y.saturating_add(local.y),
            ))
        }
        _ => None,
    };
    ActionMarker {
        action_id: record.id,
        kind: inspector_action_kind(record.kind),
        position,
        age_ms: now_ms.saturating_sub(record.ts_ms),
    }
}

/// `adesk_core::Observation` + optional image → the AGP result of `observe`,
/// `wait_for_change` and `wait_for_quiet` (§5.4).
///
/// `ObserveResult` owns the §4 wire shape (`{"observation": {.., "image": ..}}`);
/// this bridge only pairs the two halves, keeping "no image requested" and
/// "image attached" distinguishable (`None` vs `Some`).
pub fn observe_result(observation: Observation, image: Option<ImagePayload>) -> ObserveResult {
    ObserveResult { observation, image }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{ActionId, Observation, Position, Rect, WindowId, WindowInfo, WindowState};
    use adesk_inspector::ActionKind as InspectorActionKind;
    use adesk_observer::ActionKind as ObserverActionKind;
    use adesk_proto::RendererKind;

    fn window(id: u64, last_commit_seq: u64, geometry: Rect, popup_count: u32) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: None,
            title: None,
            geometry,
            state: WindowState::Active,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq,
            popup_count,
        }
    }

    fn record(position: Option<Position>, ts_ms: u64) -> ActionRecord {
        ActionRecord {
            id: ActionId(5),
            kind: ObserverActionKind::Click,
            window_id: Some(WindowId(7)),
            position,
            seq: 9,
            ts_ms,
        }
    }

    fn observation() -> Observation {
        Observation {
            window_id: Some(WindowId(7)),
            after_action: Some(ActionId(3)),
            commits: 2,
            changed_regions: vec![Rect::new(1, 2, 3, 4)],
            focus_changed: Some(true),
            title_changed: false,
            new_windows: vec![WindowId(8)],
            destroyed_windows: vec![],
            popups_appeared: vec![1],
            popups_disappeared: vec![],
            elapsed_ms: 120,
            quiet: true,
            timed_out: false,
            last_commit_seq: 11,
            seq: 42,
        }
    }

    // ------------------------------------------------------------ observer_snapshot

    #[test]
    fn observer_snapshot_copies_watermarks_geometry_and_order() {
        let compositor = CompositorSnapshot {
            // Deliberately not id-ordered: the bridge must preserve this order.
            windows: vec![
                window(7, 34, Rect::new(100, 50, 1280, 800), 2),
                window(3, 0, Rect::new(0, 0, 640, 480), 0),
            ],
            active_window_id: Some(WindowId(7)),
            keyboard_focus: Some(WindowId(3)),
            seq: 8291,
            ts_ms: 51_234,
        };

        let bridged = observer_snapshot(&compositor);

        assert_eq!(bridged.seq, 8291);
        assert_eq!(bridged.ts_ms, 51_234);
        assert_eq!(
            bridged.windows,
            vec![
                WindowSnapshot {
                    window_id: WindowId(7),
                    last_commit_seq: 34,
                    geometry: Rect::new(100, 50, 1280, 800),
                    popup_count: 2,
                },
                WindowSnapshot {
                    window_id: WindowId(3),
                    last_commit_seq: 0,
                    geometry: Rect::new(0, 0, 640, 480),
                    popup_count: 0,
                },
            ],
            "creation order and the compositor's commit counters are preserved"
        );
    }

    #[test]
    fn observer_snapshot_of_an_empty_runtime_is_empty() {
        let compositor = CompositorSnapshot {
            windows: vec![],
            active_window_id: None,
            keyboard_focus: None,
            seq: 0,
            ts_ms: 0,
        };

        let bridged = observer_snapshot(&compositor);

        assert_eq!(bridged, ObserverSnapshot::default());
    }

    // ------------------------------------------------------------ observer_condition

    #[test]
    fn observer_condition_maps_every_variant() {
        assert_eq!(
            observer_condition(ProtoCondition::Quiet { quiet_ms: 250 }),
            ObserverCondition::Quiet { quiet_ms: 250 }
        );
        assert_eq!(
            observer_condition(ProtoCondition::Quiet { quiet_ms: 0 }),
            ObserverCondition::Quiet { quiet_ms: 0 }
        );
        assert_eq!(
            observer_condition(ProtoCondition::Change),
            ObserverCondition::Change
        );
        assert_eq!(
            observer_condition(ProtoCondition::Timeout),
            ObserverCondition::Timeout
        );
    }

    // ------------------------------------------------------------ inspector_action_kind

    #[test]
    fn inspector_action_kind_maps_every_variant() {
        assert_eq!(ObserverActionKind::ALL.len(), 13);
        assert_eq!(
            ObserverActionKind::ALL.len(),
            InspectorActionKind::ALL.len()
        );
        for (observer, inspector) in ObserverActionKind::ALL.iter().zip(InspectorActionKind::ALL) {
            assert_eq!(
                inspector_action_kind(*observer),
                inspector,
                "{} must map 1:1",
                observer.as_str()
            );
            assert_eq!(
                inspector_action_kind(*observer).as_str(),
                observer.as_str(),
                "wire name must survive the bridge"
            );
        }
    }

    // ------------------------------------------------------------ proto_renderer

    #[test]
    fn proto_renderer_maps_every_variant() {
        assert_eq!(proto_renderer(RendererName::Gl), RendererKind::Gl);
        assert_eq!(proto_renderer(RendererName::Pixman), RendererKind::Pixman);
    }

    // ------------------------------------------------------------ action_marker

    #[test]
    fn action_marker_resolves_pixel_positions_through_the_geometry_origin() {
        let geometry = Rect::new(100, 50, 100, 50);

        let marker = action_marker(
            &record(Some(Position::pixels(10, 10)), 400),
            Some(geometry),
            500,
        );

        assert_eq!(marker.action_id, ActionId(5));
        assert_eq!(marker.kind, InspectorActionKind::Click);
        assert_eq!(marker.position, Some(Point { x: 110, y: 60 }));
        assert_eq!(marker.age_ms, 100);
    }

    #[test]
    fn action_marker_resolves_normalized_positions_to_output_pixels() {
        let geometry = Rect::new(100, 50, 100, 50);

        let origin = action_marker(
            &record(Some(Position::normalized(0.0, 0.0)), 0),
            Some(geometry),
            0,
        );
        assert_eq!(origin.position, Some(Point { x: 100, y: 50 }));

        let corner = action_marker(
            &record(Some(Position::normalized(1.0, 1.0)), 0),
            Some(geometry),
            0,
        );
        assert_eq!(corner.position, Some(Point { x: 199, y: 99 }));
    }

    #[test]
    fn action_marker_position_is_none_without_record_position_or_geometry() {
        let geometry = Rect::new(100, 50, 100, 50);

        let no_position = action_marker(&record(None, 10), Some(geometry), 20);
        assert_eq!(no_position.position, None);

        let no_geometry = action_marker(&record(Some(Position::pixels(10, 10)), 10), None, 20);
        assert_eq!(no_geometry.position, None);

        let neither = action_marker(&record(None, 10), None, 20);
        assert_eq!(neither.position, None);
    }

    #[test]
    fn action_marker_age_saturates_when_the_record_is_newer_than_now() {
        let newer = action_marker(&record(None, 300), None, 100);
        assert_eq!(newer.age_ms, 0);

        let same = action_marker(&record(None, 300), None, 300);
        assert_eq!(same.age_ms, 0);

        let older = action_marker(&record(None, 100), None, 300);
        assert_eq!(older.age_ms, 200);

        let saturated = action_marker(&record(None, u64::MAX), None, 0);
        assert_eq!(saturated.age_ms, 0);
    }

    // ------------------------------------------------------------ observe_result

    #[test]
    fn observe_result_carries_observation_without_image() {
        let observation = observation();

        let result = observe_result(observation.clone(), None);

        assert_eq!(result.observation, observation);
        assert_eq!(result.image, None);
    }

    #[test]
    fn observe_result_carries_observation_with_image() {
        let observation = observation();
        let image = ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0).expect("valid payload");

        let result = observe_result(observation.clone(), Some(image.clone()));

        assert_eq!(result.observation, observation);
        assert_eq!(result.image, Some(image));
    }

    #[test]
    fn observe_result_serializes_as_a_nested_observation_with_null_image() {
        let result = observe_result(observation(), None);

        let json = serde_json::to_value(&result).expect("serializable");

        assert_eq!(json["observation"]["commits"], 2);
        assert_eq!(json["observation"]["seq"], 42);
        assert!(
            json["observation"]["image"].is_null(),
            "image is present as null when absent: {json}"
        );
    }

    // ---------------------------------------------------------- recording encoder

    #[test]
    fn recorder_encoder_maps_every_variant() {
        assert_eq!(recorder_encoder(RecordingEncoder::Auto), EncoderKind::Auto);
        assert_eq!(
            recorder_encoder(RecordingEncoder::Software),
            EncoderKind::Software
        );
        assert_eq!(recorder_encoder(RecordingEncoder::Gpu), EncoderKind::Gpu);
    }

    #[test]
    fn proto_encoder_maps_every_variant() {
        assert_eq!(proto_encoder(EncoderKind::Auto), RecordingEncoder::Auto);
        assert_eq!(
            proto_encoder(EncoderKind::Software),
            RecordingEncoder::Software
        );
        assert_eq!(proto_encoder(EncoderKind::Gpu), RecordingEncoder::Gpu);
    }

    #[test]
    fn recording_encoder_mapping_round_trips() {
        for kind in [EncoderKind::Auto, EncoderKind::Software, EncoderKind::Gpu] {
            assert_eq!(recorder_encoder(proto_encoder(kind)), kind);
        }
    }
}
