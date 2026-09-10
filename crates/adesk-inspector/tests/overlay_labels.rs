//! Pixel behaviour of the label-based overlays: `window_ids`, `app_ids`,
//! `focus` (see `src/paint/labels.rs` for the slot layout).

mod common;

use adesk_core::{ImageBuffer, OverlayKind, Point, WindowId};
use adesk_inspector::{font, Inspector, OverlayStyle};

/// Opaque black: the base frame (`ImageBuffer::new_rgba`) and, because the
/// default plate is `rgba(0, 0, 0, 160)` blended over black, every pixel that
/// only carries plate background.
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Asserts every ink pixel of `label` at scale 1 (ink origin `origin`, advance
/// [`font::FONT_ADVANCE`]) equals `color`.
fn assert_ink(out: &ImageBuffer, origin: Point, label: &str, color: [u8; 4]) {
    let mut pen_x = origin.x;
    for c in label.chars() {
        let glyph = font::glyph(c).unwrap_or_else(font::fallback);
        for gy in 0..usize::from(font::FONT_HEIGHT) {
            for gx in 0..usize::from(font::FONT_WIDTH) {
                if !glyph.ink(gx, gy) {
                    continue;
                }
                let (x, y) = (pen_x + gx as i32, origin.y + gy as i32);
                assert_eq!(
                    common::px(out, x as u32, y as u32),
                    color,
                    "ink pixel of {c:?} at ({x}, {y})"
                );
            }
        }
        pen_x += font::advance(1);
    }
}

/// Number of ink pixels `label` occupies at scale 1.
fn ink_count(label: &str) -> usize {
    label
        .chars()
        .map(|c| {
            let glyph = font::glyph(c).unwrap_or_else(font::fallback);
            (0..usize::from(font::FONT_HEIGHT))
                .map(|gy| {
                    (0..usize::from(font::FONT_WIDTH))
                        .filter(|gx| glyph.ink(*gx, gy))
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

#[test]
fn window_ids_draws_label_in_slot_zero() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(7, common::rect(0, 0, 32, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();

    // Inner width = 32 - 2*2 = 28 px, "win 7" is 29 px, so the label elides to
    // "wi.." (23 px). Plate = ink rect (4, 4, 23, 7) inflated by pad 2:
    // (2, 2, 27, 11) -> right border column x = 28, bottom border row y = 12.
    assert_eq!(
        common::px(&out, 2, 2),
        style.outline.to_array(),
        "plate top-left border"
    );
    for y in 2..=12 {
        assert_eq!(
            common::px(&out, 28, y),
            style.outline.to_array(),
            "plate right border column, y = {y}"
        );
    }
    for x in 2..=28 {
        assert_eq!(
            common::px(&out, x, 12),
            style.outline.to_array(),
            "plate bottom border row, x = {x}"
        );
    }
    assert_eq!(common::px(&out, 29, 2), BLACK, "nothing right of the plate");
    assert_eq!(
        common::px(&out, 29, 12),
        BLACK,
        "nothing right of the plate"
    );

    // Text ink of the elided label at the ink origin (4, 4).
    assert_ink(&out, Point { x: 4, y: 4 }, "wi..", style.text.to_array());
    assert_eq!(
        common::px(&out, 27, 8),
        BLACK,
        "plate interior carries background only"
    );

    // Exactly the plate border plus the glyph ink is drawn; the translucent
    // plate over black is black, so those pixels are the only changes.
    let border_pixels = 2 * 27 + 2 * 11 - 4;
    let white = border_pixels + ink_count("wi..");
    assert_eq!(common::count_pixels(&out, style.outline.to_array()), white);
    assert_eq!(common::diff_bytes(&out, &input.frame), 3 * white);
}

#[test]
fn window_ids_elides_long_text_to_window_width() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(123456, common::rect(0, 0, 16, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();
    let window = common::rect(0, 0, 16, 24);

    // Inner width = 16 - 4 = 12 px: even "wi.." (23 px) does not fit, so the
    // text elides to ".." (11 px). Plate = (2, 2, 15, 11); its right border
    // column (x = 16) is outside the window clip and is not drawn.
    assert_eq!(
        common::px(&out, 2, 2),
        style.outline.to_array(),
        "plate top-left border"
    );
    for x in 2..=15 {
        assert_eq!(
            common::px(&out, x, 2),
            style.outline.to_array(),
            "top border row, x = {x}"
        );
        assert_eq!(
            common::px(&out, x, 12),
            style.outline.to_array(),
            "bottom border row, x = {x}"
        );
    }
    for x in 16..64 {
        assert_eq!(
            common::px(&out, x, 2),
            BLACK,
            "no label pixel right of the window clip, x = {x}"
        );
        assert_eq!(
            common::px(&out, x, 12),
            BLACK,
            "no label pixel right of the window clip, x = {x}"
        );
    }
    assert_eq!(
        common::px(&out, 16, 5),
        BLACK,
        "the clipped right border column leaves x = 16 untouched"
    );
    assert_ink(&out, Point { x: 4, y: 4 }, "..", style.text.to_array());

    // Byte-identical to a reference render of ".." clipped to the window.
    let reference = common::reference_label(&input.frame, window, 0, "..", &style);
    assert_eq!(out.data, reference.data);
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
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();

    // Slot 1 sits one slot below slot 0: line_height(1) + pad = 10 px, so the
    // plate top-left is (2, 12). Inner width = 48 - 4 = 44 px, "app
    // org.example.app" is 113 px, so it elides to "app o.." (41 px):
    // plate = (2, 12, 45, 11).
    assert_eq!(
        common::px(&out, 2, 12),
        style.outline.to_array(),
        "app_ids plate top-left"
    );
    assert_eq!(
        common::px(&out, 2, 2),
        BLACK,
        "slot 0 stays empty when only app_ids is enabled"
    );
    for x in 2..=46 {
        assert_eq!(
            common::px(&out, x, 22),
            style.outline.to_array(),
            "plate bottom border row, x = {x}"
        );
    }
    assert_eq!(
        common::px(&out, 46, 12),
        style.outline.to_array(),
        "plate right border column"
    );
    assert_eq!(
        common::px(&out, 47, 12),
        BLACK,
        "nothing right of the plate"
    );
    assert_ink(
        &out,
        Point { x: 4, y: 14 },
        "app o..",
        style.text.to_array(),
    );

    let reference = common::reference_label(
        &input.frame,
        common::rect(0, 0, 48, 32),
        1,
        "app o..",
        &style,
    );
    assert_eq!(out.data, reference.data, "app_ids draws in slot 1");
}

#[test]
fn app_ids_labels_missing_app_id_as_question_mark() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(7, common::rect(0, 0, 48, 32))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::AppIds]);
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();

    // No app id: the literal text "app ?" (29 px, unelided) -> plate
    // (2, 12, 33, 11). A guessed id would be wider and would move the border.
    assert_eq!(
        common::px(&out, 2, 12),
        style.outline.to_array(),
        "plate top-left"
    );
    assert_eq!(
        common::px(&out, 34, 12),
        style.outline.to_array(),
        "plate right border column"
    );
    assert_eq!(
        common::px(&out, 35, 12),
        BLACK,
        "nothing right of the plate"
    );
    assert_eq!(
        common::px(&out, 2, 22),
        style.outline.to_array(),
        "plate bottom-left corner"
    );
    assert_ink(&out, Point { x: 4, y: 14 }, "app ?", style.text.to_array());

    // Byte-identical to a reference render of the literal "app ?".
    let reference =
        common::reference_label(&input.frame, common::rect(0, 0, 48, 32), 1, "app ?", &style);
    assert_eq!(
        out.data, reference.data,
        "missing app id renders as 'app ?'"
    );
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
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();
    let focus = style.focus.to_array();

    // Window 2 fills the frame, so its outline traces the frame border.
    assert_eq!(common::px(&out, 0, 0), focus, "focus outline top-left");
    assert_eq!(common::px(&out, 63, 0), focus, "focus outline top-right");
    assert_eq!(common::px(&out, 0, 47), focus, "focus outline bottom-left");
    assert_eq!(
        common::px(&out, 63, 47),
        focus,
        "focus outline bottom-right"
    );
    assert_eq!(
        common::px(&out, 1, 1),
        BLACK,
        "the outline is 1 px; the interior is untouched"
    );
    // Window 1 (32x24) is not the active window and must not be outlined: its
    // right border column (x = 31) stays black outside the focus label plate.
    assert_eq!(
        common::px(&out, 31, 10),
        BLACK,
        "inactive window is not outlined"
    );
    assert_eq!(
        common::px(&out, 31, 20),
        BLACK,
        "inactive window is not outlined"
    );

    // "focus" label in slot 2: plate top-left = (2, 2 + 2 * 10) = (2, 22),
    // plate (2, 22, 33, 11), ink origin (4, 24).
    assert_eq!(
        common::px(&out, 2, 22),
        style.outline.to_array(),
        "focus label plate top-left"
    );
    assert_eq!(
        common::px(&out, 34, 22),
        style.outline.to_array(),
        "focus label plate right border"
    );
    assert_eq!(
        common::px(&out, 3, 23),
        BLACK,
        "plate interior carries background only"
    );
    assert_ink(&out, Point { x: 4, y: 24 }, "focus", focus);

    // Green pixels are the window outline plus the label ink; white pixels are
    // the plate border only (the focus label text is the accent colour).
    assert_ne!(focus, style.text.to_array());
    let outline_pixels = 2 * 64 + 2 * 48 - 4;
    assert_eq!(
        common::count_pixels(&out, focus),
        outline_pixels + ink_count("focus")
    );
    assert_eq!(
        common::count_pixels(&out, style.outline.to_array()),
        2 * 33 + 2 * 11 - 4
    );
}

#[test]
fn labels_are_clipped_at_frame_edges() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(9, common::rect(60, 44, 32, 24))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let out = inspector.render(&input).expect("render");
    let style = OverlayStyle::default();

    // Inner width = 28 px -> "win 9" (29 px) elides to "wi.." (23 px) and the
    // plate is (62, 46, 27, 11). Clipped to the 64x48 frame only its top-left
    // corner survives: the top border row (62..=63, y = 46) and the left
    // column's first pixel (62, 47). The plate fill is black over black, so
    // exactly those three pixels change.
    let changed = common::changed_pixels(&out, &input.frame);
    assert_eq!(changed, vec![(62, 46), (63, 46), (62, 47)]);
    for (x, y) in changed {
        assert_eq!(
            common::px(&out, x, y),
            style.outline.to_array(),
            "visible label pixel ({x}, {y})"
        );
    }
    assert_eq!(
        common::px(&out, 63, 47),
        BLACK,
        "plate background over black changes nothing"
    );
    assert_eq!(
        common::diff_bytes(&out, &input.frame),
        3 * 3,
        "three white pixels, three differing bytes each"
    );
}

#[test]
fn labels_are_clipped_to_window_geometry() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(123456, common::rect(4, 4, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::WindowIds]);
    let out = inspector.render(&input).expect("render");

    // Spec observation: the window's inner width is 8 - 2*2 = 4 px, so
    // `text::elide` returns `None` (".." alone is 11 px) and the painter draws
    // nothing at all. The "clipped to the window geometry" property therefore
    // holds vacuously here: elision is what keeps a label inside its window,
    // and the window clip is the second line of defence (exercised by
    // `labels_are_clipped_at_frame_edges`).
    assert_eq!(
        out.data, input.frame.data,
        "nothing fits, so no label pixel exists outside (4, 4, 8, 8)"
    );
    assert_eq!(common::diff_bytes(&out, &input.frame), 0);
    assert!(common::changed_pixels(&out, &input.frame).is_empty());
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
    let only_app_ids = Inspector::new(vec![OverlayKind::AppIds])
        .render(&input)
        .expect("render");
    let with_window_ids = Inspector::new(vec![OverlayKind::WindowIds, OverlayKind::AppIds])
        .render(&input)
        .expect("render");
    let style = OverlayStyle::default();

    // Slot 1 is fixed: the app plate is (2, 12, 45, 11) whether or not
    // window_ids is enabled, and the whole plate region is byte-identical.
    assert_eq!(
        common::px(&only_app_ids, 2, 12),
        style.outline.to_array(),
        "app plate top-left without window_ids"
    );
    assert_eq!(
        common::px(&with_window_ids, 2, 12),
        style.outline.to_array(),
        "app plate top-left with window_ids"
    );
    common::assert_region_equal(&only_app_ids, &with_window_ids, common::rect(2, 12, 45, 11));
    // window_ids still occupies slot 0, so the two renders do differ.
    assert_ne!(only_app_ids.data, with_window_ids.data);
    assert_eq!(
        common::px(&with_window_ids, 2, 2),
        style.outline.to_array(),
        "window_ids occupies slot 0"
    );
}
