//! Pixel behaviour of the HUD overlays: `actions`, `commit_timing`.

mod common;

use adesk_core::{ActionId, ImageBuffer, OverlayKind, Point, Rect, Size};
use adesk_inspector::{
    text, ActionKind, ActionMarker, Canvas, Color, CommitInfo, Inspector, OverlayStyle,
};

/// Opaque black base pixel (`ImageBuffer::new_rgba`).
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Style with `text` replaced by an overlay accent colour, exactly what the
/// HUD painters hand to [`text::draw_label`].
fn accent(style: &OverlayStyle, color: Color) -> OverlayStyle {
    let mut copy = *style;
    copy.text = color;
    copy
}

/// Fresh frame with `labels` (ink origin, text) drawn through the public label
/// primitive — the reference render for HUD content assertions.
fn labels_frame(
    width: u32,
    height: u32,
    labels: &[(Point, &str)],
    style: &OverlayStyle,
) -> ImageBuffer {
    let mut frame = common::frame(width, height);
    let mut canvas = Canvas::new(&mut frame);
    for (origin, label) in labels {
        text::draw_label(&mut canvas, *origin, label, style);
    }
    frame
}

#[test]
fn actions_draw_plus_marker_and_label_for_positioned_action() {
    let base = common::frame(64, 48);
    let input = common::input(base)
        .actions(vec![ActionMarker {
            action_id: ActionId(3),
            kind: ActionKind::Click,
            position: Some(Point { x: 20, y: 20 }),
            age_ms: 120,
        }])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Actions]);
    let out = inspector.render(&input).expect("render");
    let style = inspector.style;
    let action = style.action.to_array();

    // Plus marker: 9 px horizontal arm, 9 px vertical arm, 3x3 centre square.
    for x in 16..=24 {
        assert_eq!(common::px(&out, x, 20), action, "horizontal arm ({x},20)");
    }
    for y in 16..=24 {
        assert_eq!(common::px(&out, 20, y), action, "vertical arm (20,{y})");
    }
    for y in 19..=21 {
        for x in 19..=21 {
            assert_eq!(common::px(&out, x, y), action, "centre ({x},{y})");
        }
    }
    // 9 arm + 8 new vertical + 4 new centre corners = 21 distinct px. The label
    // ink is action-coloured too, so count only inside the marker box.
    let marker_px = (16..=24)
        .flat_map(|y| (16..=24).map(move |x| (x, y)))
        .filter(|&(x, y)| common::px(&out, x, y) == action)
        .count();
    assert_eq!(marker_px, 21);

    // Label: ink origin is position + (4,4); "click #3 +120ms" has 15 glyphs,
    // so its ink is 15 * 6 - 1 = 89 px wide and the plate is 89 + 2 * pad = 93.
    let label = "click #3 +120ms";
    let origin = Point { x: 24, y: 24 };
    assert_eq!(text::measure(label, style.scale()), Size::new(89, 7));
    let plate = text::label_rect(origin, label, &style);
    assert_eq!(plate, Rect::new(22, 22, 93, 11));
    assert_eq!(
        common::px(&out, 22, 22),
        style.outline.to_array(),
        "plate corner"
    );

    // Content proof: the plate region is byte-identical to a reference render
    // of the same text at the same ink origin with the action accent.
    let expected = labels_frame(64, 48, &[(origin, label)], &accent(&style, style.action));
    for y in plate.y..plate.bottom().min(48) {
        for x in plate.x..plate.right().min(64) {
            assert_eq!(
                common::px(&out, x as u32, y as u32),
                common::px(&expected, x as u32, y as u32),
                "label pixel ({x},{y})"
            );
        }
    }
    // The plate carries action-coloured ink.
    assert!(common::count_in(&out, plate, action) > 0);
}

#[test]
fn actions_hud_lists_positionless_actions_top_left() {
    let base = common::frame(96, 48);
    let input = common::input(base)
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
    let out = inspector.render(&input).expect("render");
    let style = inspector.style;
    let action = style.action.to_array();

    // Line 0 plate at (pad, pad) = (2,2); line 1 at
    // (pad, pad + line_height + pad) = (2,12). Ink origins are the plate
    // origin + (pad, pad).
    let first = "keypress #1 +10ms";
    let second = "type_text #2 +20ms";
    let plate0 = text::label_rect(Point { x: 4, y: 4 }, first, &style);
    let plate1 = text::label_rect(Point { x: 4, y: 14 }, second, &style);
    // 17 glyphs -> 101 px ink; 18 glyphs -> 107 px ink; plate = ink + 2 * pad.
    assert_eq!(plate0, Rect::new(2, 2, 105, 11));
    assert_eq!(plate1, Rect::new(2, 12, 111, 11));

    assert_eq!(
        common::px(&out, 2, 2),
        style.outline.to_array(),
        "line 0 corner"
    );
    assert_eq!(
        common::px(&out, 2, 12),
        style.outline.to_array(),
        "line 1 corner"
    );
    assert!(common::count_in(&out, plate0, action) > 0, "line 0 ink");
    assert!(common::count_in(&out, plate1, action) > 0, "line 1 ink");

    // Content proof: the whole frame equals a reference render of the two
    // labels at the asserted ink origins (everything is clipped to the frame).
    let expected = labels_frame(
        96,
        48,
        &[
            (Point { x: 4, y: 4 }, first),
            (Point { x: 4, y: 14 }, second),
        ],
        &accent(&style, style.action),
    );
    assert_eq!(out.data, expected.data);
}

#[test]
fn actions_preserve_input_order() {
    let base = common::frame(64, 64);
    let first = ActionMarker {
        action_id: ActionId(1),
        kind: ActionKind::Click,
        position: Some(Point { x: 30, y: 30 }),
        age_ms: 5,
    };
    let second = ActionMarker {
        action_id: ActionId(2),
        kind: ActionKind::PointerMove,
        position: Some(Point { x: 31, y: 30 }),
        age_ms: 1,
    };
    let one_input = common::input(base.clone()).actions(vec![first]).build();
    let both_input = common::input(base).actions(vec![first, second]).build();
    let style = OverlayStyle::default();
    let action = style.action.to_array();

    let one = Inspector::new(vec![OverlayKind::Actions])
        .render(&one_input)
        .expect("render");
    let both = Inspector::new(vec![OverlayKind::Actions])
        .render(&both_input)
        .expect("render");

    // Both plus glyphs are opaque action colour, so they cannot reveal order.
    assert_eq!(common::px(&one, 30, 30), action);
    assert_eq!(common::px(&both, 30, 30), action);
    assert_eq!(common::px(&both, 31, 30), action);

    // Marker 1's label ink origin is (30+4, 30+4) = (34,34); "click #1 +5ms"
    // is 13 glyphs (ink 77 px), so its plate is (32,32,81,11). Marker 2's
    // label ink origin is (35,34); "pointer_move #2 +1ms" is 20 glyphs (ink
    // 119 px), so its plate is (33,32,123,11) and overlaps marker 1's.
    assert_eq!(
        text::label_rect(Point { x: 34, y: 34 }, "click #1 +5ms", &style),
        Rect::new(32, 32, 81, 11)
    );
    assert_eq!(
        text::label_rect(Point { x: 35, y: 34 }, "pointer_move #2 +1ms", &style),
        Rect::new(33, 32, 123, 11)
    );

    // (33,33) is marker 1's plate interior (invisible over black) and marker
    // 2's left plate border: white only once marker 2 is drawn.
    assert_eq!(common::px(&one, 33, 33), BLACK);
    assert_eq!(common::px(&both, 33, 33), style.outline.to_array());

    // The 'c' of "click" is the first glyph at (34,34); its row 3 (y=37) inks
    // columns 0 and 4 of the 5 px box, i.e. (34,37) and (38,37). Marker 2's
    // text starts at x=35 and its plate covers x >= 33, so (34,37) is marker
    // 2's translucent plate painted over marker 1's action ink. The documented
    // blend predicts plate over cyan.
    assert_eq!(common::px(&one, 34, 37), action);
    let plate_over_action = adesk_inspector::blend_over(style.plate, action);
    assert_eq!(plate_over_action, [0, 95, 95, 255]);
    assert_eq!(common::px(&both, 34, 37), plate_over_action);

    // Control pixel: (34,41) is inside both plates but under no ink, so the
    // translucent black plate leaves it black in both renders — proving the
    // pixel above changed because of marker 1's ink, not the plate itself.
    assert_eq!(common::px(&one, 34, 41), BLACK);
    assert_eq!(common::px(&both, 34, 41), BLACK);
}

#[test]
fn commit_timing_right_aligns_hud() {
    let base = common::frame(96, 48);
    let input = common::input(base)
        .commit(Some(CommitInfo {
            commit_seq: 8291,
            age_ms: 123,
        }))
        .build();
    let inspector = Inspector::new(vec![OverlayKind::CommitTiming]);
    let out = inspector.render(&input).expect("render");
    let style = inspector.style;

    // "commit 8291 +123ms" is 18 glyphs: ink 18 * 6 - 1 = 107 px, plate
    // 107 + 2 * pad = 111 px wide. The plate's right edge is exclusive at
    // 96 - pad = 94, so its left edge is 94 - 111 = -17 and the ink origin is
    // (-17 + pad, 0 + pad + pad) = (-15, 4).
    let label = "commit 8291 +123ms";
    assert_eq!(text::measure(label, style.scale()), Size::new(107, 7));
    let origin = Point { x: -15, y: 4 };
    let plate = text::label_rect(origin, label, &style);
    assert_eq!(plate, Rect::new(-17, 2, 111, 11));

    // The last plate border column is 93, in the top and bottom rows and the
    // right column.
    assert_eq!(
        common::px(&out, 93, 2),
        style.outline.to_array(),
        "top-right corner"
    );
    assert_eq!(
        common::px(&out, 93, 12),
        style.outline.to_array(),
        "bottom-right corner"
    );
    for y in 3..=11 {
        assert_eq!(
            common::px(&out, 93, y),
            style.outline.to_array(),
            "right border (93,{y})"
        );
    }
    // Nothing is painted at or beyond the exclusive right edge 94.
    for y in 0..48 {
        assert_eq!(common::px(&out, 94, y), BLACK);
        assert_eq!(common::px(&out, 95, y), BLACK);
    }

    // Content proof: the whole frame equals a reference render of the same
    // label at the plate's ink origin with the timing accent.
    let expected = labels_frame(96, 48, &[(origin, label)], &accent(&style, style.timing));
    assert_eq!(out.data, expected.data);
}

#[test]
fn actions_hud_and_commit_hud_coexist_on_narrow_frames() {
    let base = common::frame(40, 16);
    let input = common::input(base.clone())
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
    let style = OverlayStyle::default();
    let action = style.action.to_array();

    let actions_only = Inspector::new(vec![OverlayKind::Actions])
        .render(&input)
        .expect("render");
    let commit_only = Inspector::new(vec![OverlayKind::CommitTiming])
        .render(&input)
        .expect("render");
    let both = Inspector::new(vec![OverlayKind::Actions, OverlayKind::CommitTiming])
        .render(&input)
        .expect("render");

    // Both HUDs contribute: each differs from the base, from the other and
    // from the combined render.
    assert!(common::diff_bytes(&actions_only, &base) > 0);
    assert!(common::diff_bytes(&commit_only, &base) > 0);
    assert!(common::diff_bytes(&actions_only, &commit_only) > 0);
    assert!(common::diff_bytes(&both, &actions_only) > 0);
    assert!(common::diff_bytes(&both, &commit_only) > 0);

    // Both are clipped to the 40x16 frame: the buffer keeps its size, and the
    // canvas dropped everything outside it.
    assert_eq!(both.size(), Size::new(40, 16));
    assert_eq!(both.data.len(), 40 * 16 * 4);

    // Actions HUD: "keypress #9 +7ms" (16 glyphs, ink 95 px) at plate (2,2).
    // Commit HUD: "commit 1 +2ms" (13 glyphs, ink 77 px) right-aligned to
    // 40 - pad = 38, so its plate is (38 - 81, 2) = (-43, 2).
    let plate_actions = text::label_rect(Point { x: 4, y: 4 }, "keypress #9 +7ms", &style);
    let plate_commit = text::label_rect(Point { x: -41, y: 4 }, "commit 1 +2ms", &style);
    assert_eq!(plate_actions, Rect::new(2, 2, 99, 11));
    assert_eq!(plate_commit, Rect::new(-43, 2, 81, 11));

    // Canonical order: actions (index 6) below commit_timing (index 7). (4,4)
    // is the 'k' ink of "keypress #9 +7ms" and lies inside the commit plate,
    // where the commit text ("commit 1 +2ms": '1' inks only column x=3 of its
    // 5 px box in row y=4) has no ink. So the combined render shows exactly
    // the documented plate-over-action blend.
    assert_eq!(common::px(&actions_only, 4, 4), action);
    assert_eq!(common::px(&commit_only, 4, 4), BLACK);
    let plate_over_action = adesk_inspector::blend_over(style.plate, action);
    assert_eq!(plate_over_action, [0, 95, 95, 255]);
    assert_eq!(common::px(&both, 4, 4), plate_over_action);
}
