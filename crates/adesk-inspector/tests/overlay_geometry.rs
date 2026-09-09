//! Pixel behaviour of the geometry overlays: `damage`, `surface_bounds`,
//! `cursor`.

mod common;

use adesk_core::{OverlayKind, Point};
use adesk_inspector::Inspector;

#[test]
fn damage_fills_and_outlines_each_rect() {
    let input = common::input(common::frame(32, 32))
        .damage(vec![common::rect(4, 4, 8, 6)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Damage]);
    let _out = inspector.render(&input);
    todo!("assert fill colour inside (4,4,8,6), outline on its boundary, untouched elsewhere");
}

#[test]
fn damage_overlaps_blend_repeatedly() {
    let input = common::input(common::frame(32, 32))
        .damage(vec![common::rect(0, 0, 8, 8), common::rect(4, 4, 8, 8)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Damage]);
    let _out = inspector.render(&input);
    todo!("assert the overlap region is blended twice (exact documented integer values)");
}

#[test]
fn damage_ignores_empty_rects() {
    let input = common::input(common::frame(32, 32))
        .damage(vec![common::rect(4, 4, 0, 8), common::rect(4, 4, 8, 0)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Damage]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
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
    let _out = inspector.render(&input);
    todo!("assert both geometries are outlined in style.outline and no interior pixel is touched");
}

#[test]
fn surface_bounds_clips_at_frame_edges() {
    let input = common::input(common::frame(16, 16))
        .windows(vec![common::window(1, common::rect(-4, -4, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let _out = inspector.render(&input);
    todo!("assert only the in-frame part of the outline is drawn");
}

#[test]
fn cursor_draws_nine_pixel_crosshair() {
    let input = common::input(common::frame(32, 32))
        .cursor_at(Point { x: 10, y: 12 })
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Cursor]);
    let _out = inspector.render(&input);
    todo!("assert (6..=14,12) and (10,8..=16) are style.cursor and the centre pixel is drawn once");
}

#[test]
fn cursor_clips_near_frame_edges() {
    let input = common::input(common::frame(16, 16))
        .cursor_at(Point { x: 0, y: 0 })
        .build();
    let inspector = Inspector::new(vec![OverlayKind::Cursor]);
    let _out = inspector.render(&input);
    todo!("assert the crosshair is clipped to the frame and no out-of-bounds pixel is written");
}

#[test]
fn cursor_draws_nothing_when_unknown() {
    let input = common::input(common::frame(16, 16)).build();
    let inspector = Inspector::new(vec![OverlayKind::Cursor]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to the input frame");
}
