//! Pixel behaviour of the HUD overlays: `actions`, `commit_timing`.

mod common;

use adesk_core::{ActionId, OverlayKind, Point};
use adesk_inspector::{ActionKind, ActionMarker, CommitInfo, Inspector};

#[test]
fn actions_draw_plus_marker_and_label_for_positioned_action() {
    let input = common::input(common::frame(64, 48))
        .actions(vec![ActionMarker {
            action_id: ActionId(3),
            kind: ActionKind::Click,
            position: Some(Point { x: 20, y: 20 }),
            age_ms: 120,
        }])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Actions]);
    let _out = inspector.render(&input);
    todo!("assert the 3x3 centre plus 3 px arms are style.action and the label reads 'click #3 +120ms'");
}

#[test]
fn actions_hud_lists_positionless_actions_top_left() {
    let input = common::input(common::frame(96, 48))
        .actions(vec![
            ActionMarker {
                action_id: ActionId(1),
                kind: ActionKind::Keypress,
                position: None,
                age_ms: 10,
            },
            ActionMarker {
                action_id: ActionId(2),
                kind: ActionKind::TypeText,
                position: None,
                age_ms: 20,
            },
        ])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Actions]);
    let _out = inspector.render(&input);
    todo!("assert two HUD lines at (pad, pad) and (pad, pad + line_height + pad)");
}

#[test]
fn actions_preserve_input_order() {
    let input = common::input(common::frame(64, 64))
        .actions(vec![
            ActionMarker {
                action_id: ActionId(1),
                kind: ActionKind::Click,
                position: Some(Point { x: 30, y: 30 }),
                age_ms: 5,
            },
            ActionMarker {
                action_id: ActionId(2),
                kind: ActionKind::PointerMove,
                position: Some(Point { x: 31, y: 30 }),
                age_ms: 1,
            },
        ])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Actions]);
    let _out = inspector.render(&input);
    todo!("assert the later marker is drawn over the earlier one in the overlap");
}

#[test]
fn commit_timing_right_aligns_hud() {
    let input = common::input(common::frame(96, 48))
        .commit(Some(CommitInfo {
            commit_seq: 8291,
            age_ms: 123,
        }))
        .build();
    let inspector = Inspector::new(vec![OverlayKind::CommitTiming]);
    let _out = inspector.render(&input);
    todo!("assert the plate's right edge is at 96 - pad and the text is 'commit 8291 +123ms'");
}

#[test]
fn commit_timing_draws_nothing_without_commit() {
    let input = common::input(common::frame(96, 48)).build();
    let inspector = Inspector::new(vec![OverlayKind::CommitTiming]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
}

#[test]
fn actions_hud_and_commit_hud_coexist_on_narrow_frames() {
    let input = common::input(common::frame(40, 16))
        .actions(vec![ActionMarker {
            action_id: ActionId(9),
            kind: ActionKind::Keypress,
            position: None,
            age_ms: 7,
        }])
        .commit(Some(CommitInfo {
            commit_seq: 1,
            age_ms: 2,
        }))
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Actions, OverlayKind::CommitTiming]);
    let _out = inspector.render(&input);
    todo!("assert both HUDs are drawn, clipped to the frame, in canonical order (actions below commit_timing)");
}
