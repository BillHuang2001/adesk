//! Pixel behaviour of the geometry overlays: `damage`, `surface_bounds`,
//! `cursor`.

mod common;

use adesk_core::{OverlayKind, Point};
use adesk_inspector::{blend_over, Inspector, OverlayStyle};

/// Opaque black base pixel (`ImageBuffer::new_rgba`).
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// `OverlayStyle::outline` (opaque white).
const WHITE: [u8; 4] = [255, 255, 255, 255];
/// `OverlayStyle::fill` (rgba(255,0,0,48)) blended once over the black base.
const FILL: [u8; 4] = [48, 0, 0, 255];
/// The same fill blended a second time over the first result.
const FILL_TWICE: [u8; 4] = [87, 0, 0, 255];
/// `OverlayStyle::cursor` (opaque yellow).
const CURSOR: [u8; 4] = [255, 255, 0, 255];

#[test]
fn damage_fills_and_outlines_each_rect() {
    let input = common::input(common::frame(32, 32))
        .damage(vec![common::rect(4, 4, 8, 6)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Damage]);
    let out = inspector.render(&input).expect("render");

    // Anchor the expected colours in the documented blend formula rather than
    // in bare numbers.
    let style = OverlayStyle::default();
    assert_eq!(blend_over(style.fill, BLACK), FILL);
    assert_eq!(blend_over(style.fill, FILL), FILL_TWICE);

    // Rect (4,4,8,6) spans x 4..=11 and y 4..=9. The outline owns the whole
    // boundary; the interior keeps a single translucent fill.
    for x in 4..=11 {
        assert_eq!(common::px(&out, x, 4), WHITE, "top edge ({x},4)");
        assert_eq!(common::px(&out, x, 9), WHITE, "bottom edge ({x},9)");
    }
    for y in 5..=8 {
        assert_eq!(common::px(&out, 4, y), WHITE, "left edge (4,{y})");
        assert_eq!(common::px(&out, 11, y), WHITE, "right edge (11,{y})");
    }
    for y in 5..=8 {
        for x in 5..=10 {
            assert_eq!(common::px(&out, x, y), FILL, "interior ({x},{y})");
        }
    }

    // Outside the rect nothing changes.
    assert_eq!(common::px(&out, 3, 3), BLACK);
    assert_eq!(common::px(&out, 12, 10), BLACK);

    // 8 + 8 horizontal and 4 + 4 vertical boundary pixels; 6 * 4 interior.
    assert_eq!(common::count_pixels(&out, WHITE), 24);
    assert_eq!(common::count_pixels(&out, FILL), 24);
    assert_eq!(common::count_pixels(&out, BLACK), 32 * 32 - 48);
}

#[test]
fn damage_overlaps_blend_repeatedly() {
    let input = common::input(common::frame(32, 32))
        .damage(vec![common::rect(0, 0, 8, 8), common::rect(4, 4, 8, 8)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Damage]);
    let out = inspector.render(&input).expect("render");

    // (5,5) is inside both rects, so the fill is alpha-blended twice.
    assert_eq!(common::px(&out, 5, 5), FILL_TWICE);
    // (1,1) is inside the first rect only.
    assert_eq!(common::px(&out, 1, 1), FILL);
    // Opaque outlines win inside their own rect.
    assert_eq!(common::px(&out, 0, 0), WHITE);
    assert_eq!(common::px(&out, 4, 4), WHITE);
    // But the second rect's fill is painted *after* the first rect's outline,
    // so a first-rect boundary pixel inside the second fill blends fill over
    // white: (7,7) is the first rect's bottom-right corner.
    let fill_over_white = blend_over(OverlayStyle::default().fill, WHITE);
    assert_eq!(fill_over_white, [255, 207, 207, 255]);
    assert_eq!(common::px(&out, 7, 7), fill_over_white);

    // Exact multiplicities: 4 px are double-filled (interiors ∩ interiors),
    // 54 px single-filled and 49 px pure white plus the 5 blended boundary
    // pixels above.
    assert_eq!(common::count_pixels(&out, FILL_TWICE), 4);
    assert_eq!(common::count_pixels(&out, FILL), 54);
    assert_eq!(common::count_pixels(&out, WHITE), 49);
    assert_eq!(common::count_pixels(&out, fill_over_white), 5);

    // No pixel outside the union of the two rects changes: the union is
    // 64 + 64 - 16 = 112 px, so 1024 - 112 = 912 px stay black.
    assert_eq!(common::count_pixels(&out, BLACK), 912);
}

#[test]
fn surface_bounds_outlines_every_window_geometry() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![
            common::window(1, common::rect(0, 0, 32, 24)),
            common::window(2, common::rect(32, 24, 32, 24)),
        ])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let out = inspector.render(&input).expect("render");

    // Window 1 spans x 0..=31, y 0..=23.
    for x in 0..32 {
        assert_eq!(common::px(&out, x, 0), WHITE, "window 1 top ({x},0)");
        assert_eq!(common::px(&out, x, 23), WHITE, "window 1 bottom ({x},23)");
    }
    for y in 1..=22 {
        assert_eq!(common::px(&out, 0, y), WHITE, "window 1 left (0,{y})");
        assert_eq!(common::px(&out, 31, y), WHITE, "window 1 right (31,{y})");
    }
    // Window 2 spans x 32..=63, y 24..=47.
    for x in 32..64 {
        assert_eq!(common::px(&out, x, 24), WHITE, "window 2 top ({x},24)");
        assert_eq!(common::px(&out, x, 47), WHITE, "window 2 bottom ({x},47)");
    }
    for y in 25..=46 {
        assert_eq!(common::px(&out, 32, y), WHITE, "window 2 left (32,{y})");
        assert_eq!(common::px(&out, 63, y), WHITE, "window 2 right (63,{y})");
    }

    // Interiors are untouched.
    assert_eq!(common::px(&out, 16, 12), BLACK);
    assert_eq!(common::px(&out, 48, 36), BLACK);

    // 2 * (32 + 32 + 22 + 22) boundary pixels.
    assert_eq!(common::count_pixels(&out, WHITE), 216);
    assert_eq!(common::count_pixels(&out, BLACK), 64 * 48 - 216);
}

#[test]
fn surface_bounds_clips_at_frame_edges() {
    let input = common::input(common::frame(16, 16))
        .windows(vec![common::window(1, common::rect(-4, -4, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let out = inspector.render(&input).expect("render");

    // The rect spans x -4..=3, y -4..=3: only its bottom edge (row y=3) and
    // right edge (column x=3, rows 0..=2) fall inside the 16x16 frame.
    for x in 0..=3 {
        assert_eq!(common::px(&out, x, 3), WHITE, "visible bottom edge ({x},3)");
    }
    for y in 0..=2 {
        assert_eq!(common::px(&out, 3, y), WHITE, "visible right edge (3,{y})");
    }
    assert_eq!(common::px(&out, 0, 0), BLACK);
    assert_eq!(common::px(&out, 4, 3), BLACK);
    assert_eq!(common::px(&out, 3, 4), BLACK);

    // Exactly 4 + 3 = 7 boundary pixels are visible; nothing outside the
    // outline is touched (and nothing is written outside the frame, which
    // would have been out of bounds anyway).
    assert_eq!(common::count_pixels(&out, WHITE), 7);
    assert_eq!(common::count_pixels(&out, BLACK), 16 * 16 - 7);
    assert_eq!(out.size(), input.size());
}

#[test]
fn cursor_draws_nine_pixel_crosshair() {
    let input = common::input(common::frame(32, 32))
        .cursor_at(Point { x: 10, y: 12 })
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Cursor]);
    let out = inspector.render(&input).expect("render");

    // Horizontal arm (6..=14, 12), vertical arm (10, 8..=16).
    for x in 6..=14 {
        assert_eq!(common::px(&out, x, 12), CURSOR, "horizontal arm ({x},12)");
    }
    for y in 8..=16 {
        assert_eq!(common::px(&out, 10, y), CURSOR, "vertical arm (10,{y})");
    }
    // The centre is covered by both arms; the opaque colour makes the double
    // draw invisible.
    assert_eq!(common::px(&out, 10, 12), CURSOR);
    assert_eq!(common::count_pixels(&out, CURSOR), 9 + 9 - 1);

    // Neighbours of the arms stay black.
    assert_eq!(common::px(&out, 5, 12), BLACK);
    assert_eq!(common::px(&out, 15, 12), BLACK);
    assert_eq!(common::px(&out, 10, 7), BLACK);
    assert_eq!(common::px(&out, 10, 17), BLACK);
    assert_eq!(common::count_pixels(&out, BLACK), 32 * 32 - 17);
}

#[test]
fn cursor_clips_near_frame_edges() {
    let input = common::input(common::frame(16, 16))
        .cursor_at(Point { x: 0, y: 0 })
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Cursor]);
    let out = inspector.render(&input).expect("render");

    // Only the in-frame quarter of the crosshair survives: (0..=4, 0) and
    // (0, 0..=4), the corner shared.
    for x in 0..=4 {
        assert_eq!(common::px(&out, x, 0), CURSOR, "clipped arm ({x},0)");
    }
    for y in 0..=4 {
        assert_eq!(common::px(&out, 0, y), CURSOR, "clipped arm (0,{y})");
    }
    assert_eq!(common::px(&out, 5, 0), BLACK);
    assert_eq!(common::px(&out, 0, 5), BLACK);
    assert_eq!(common::count_pixels(&out, CURSOR), 5 + 5 - 1);
    assert_eq!(common::count_pixels(&out, BLACK), 16 * 16 - 9);

    // The frame is still intact: no out-of-bounds write happened.
    assert_eq!(out.size(), input.size());
    assert_eq!(out.data.len(), 16 * 16 * 4);
}
