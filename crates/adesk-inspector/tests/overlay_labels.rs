//! Pixel behaviour of the label-based overlays: `window_ids`, `app_ids`,
//! `focus` (see `src/paint/labels.rs` for the slot layout).

mod common;

use adesk_core::{OverlayKind, WindowId};
use adesk_inspector::Inspector;

#[test]
fn window_ids_draws_label_in_slot_zero() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(7, common::rect(0, 0, 32, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let _out = inspector.render(&input);
    todo!("assert the 'win 7' plate is drawn with its top-left at (2,2)");
}

#[test]
fn window_ids_elides_long_text_to_window_width() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(123456, common::rect(0, 0, 16, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let _out = inspector.render(&input);
    todo!("assert the label is elided to window.w - 2*pad (16 - 4 = 12 px) with a trailing '..'");
}

#[test]
fn window_ids_skips_empty_window_geometry() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 0, 0))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
}

#[test]
fn app_ids_uses_slot_one() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window_with_app(
            7,
            common::rect(0, 0, 48, 32),
            "org.example.app",
        )])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::AppIds]);
    let _out = inspector.render(&input);
    todo!("assert the 'app org.example.app' plate starts one slot below window_ids' slot");
}

#[test]
fn app_ids_labels_missing_app_id_as_question_mark() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(7, common::rect(0, 0, 48, 32))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::AppIds]);
    let _out = inspector.render(&input);
    todo!("assert the label text is 'app ?' for a window with app_id == None");
}

#[test]
fn focus_draws_outline_and_label_for_active_window() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![
            common::window(1, common::rect(0, 0, 32, 24)),
            common::window(2, common::rect(0, 0, 64, 48)),
        ])
        .active(Some(WindowId(2)))
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Focus]);
    let _out = inspector.render(&input);
    todo!("assert the focus outline traces window 2's geometry in style.focus and 'focus' is in slot 2");
}

#[test]
fn focus_draws_nothing_without_active_window() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 32, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Focus]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
}

#[test]
fn focus_ignores_active_id_that_matches_no_window() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 32, 24))])
        .active(Some(WindowId(99)))
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Focus]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
}

#[test]
fn labels_are_clipped_at_frame_edges() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(9, common::rect(60, 44, 32, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let _out = inspector.render(&input);
    todo!("assert no pixel outside the 64x48 frame is written and the visible part of the label is intact");
}

#[test]
fn labels_are_clipped_to_window_geometry() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(123456, common::rect(4, 4, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let _out = inspector.render(&input);
    todo!("assert no label pixel falls outside (4,4,8,8)");
}

#[test]
fn labels_use_fixed_slots_regardless_of_enabled_subset() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window_with_app(
            7,
            common::rect(0, 0, 48, 32),
            "org.example.app",
        )])
        .build();
    let only_app_ids = Inspector::new(vec![OverlayKind::AppIds]).render(&input);
    let _ = only_app_ids;
    todo!("assert app_ids still draws in slot 1 when window_ids is disabled (no reflow)");
}
