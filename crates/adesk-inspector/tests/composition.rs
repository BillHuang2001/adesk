//! End-to-end composition behaviour: canonical order, determinism, and the
//! `region` / `max_dimension` post-processing path.

mod common;

use adesk_core::{ImageBuffer, OverlayKind};
use adesk_inspector::{InspectionInput, InspectionRequest, InspectionSource, Inspector, Result};

/// Test double for the trait `adesk-server` implements.
struct FixedSource {
    input: InspectionInput,
}

impl InspectionSource for FixedSource {
    fn inspection_input(&self, _overlays: &[OverlayKind]) -> Result<InspectionInput> {
        Ok(self.input.clone())
    }
}

#[test]
fn empty_overlays_return_the_frame_unchanged() {
    let input = common::input(common::filled_frame(16, 16, [7, 8, 9, 255]))
        .damage(vec![common::rect(0, 0, 4, 4)])
        .build();
    let inspector = Inspector::new(vec![]);
    let _out = inspector.render(&input);
    todo!("assert the output frame is byte-identical to input.frame");
}

#[test]
fn overlay_set_order_does_not_affect_output() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 64, 48))])
        .damage(vec![common::rect(8, 8, 8, 8)])
        .build();
    let forward = Inspector::new(vec![
        OverlayKind::Damage,
        OverlayKind::WindowIds,
        OverlayKind::Focus,
    ]);
    let reversed = Inspector::new(vec![
        OverlayKind::Focus,
        OverlayKind::WindowIds,
        OverlayKind::Damage,
    ]);
    let _ = (forward.render(&input), reversed.render(&input));
    todo!("assert both inspectors produce byte-identical frames (canonical order)");
}

#[test]
fn duplicate_overlays_are_deduplicated() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 64, 48))])
        .build();
    let once = Inspector::new(vec![OverlayKind::WindowIds]);
    let twice = Inspector::new(vec![OverlayKind::WindowIds, OverlayKind::WindowIds]);
    let _ = (once.render(&input), twice.render(&input));
    todo!("assert rendering twice-listed window_ids equals rendering it once (no double blend)");
}

#[test]
fn damage_is_drawn_below_geometry_overlays() {
    let input = common::input(common::frame(32, 32))
        .windows(vec![common::window(1, common::rect(0, 0, 32, 32))])
        .damage(vec![common::rect(0, 0, 32, 32)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds, OverlayKind::Damage]);
    let _out = inspector.render(&input);
    todo!("assert the bounds outline pixels keep style.outline, not a blend of fill over outline");
}

#[test]
fn render_is_deterministic_for_identical_input() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window_with_app(
            4,
            common::rect(0, 0, 64, 48),
            "org.example.app",
        )])
        .cursor_at(adesk_core::Point { x: 12, y: 9 })
        .damage(vec![common::rect(2, 2, 6, 6)])
        .build();
    let inspector = Inspector::default();
    let _ = (inspector.render(&input), inspector.render(&input));
    todo!("assert two renders of the same input are byte-identical");
}

#[test]
fn render_into_requires_matching_dimensions() {
    let input = common::input(common::frame(8, 8)).build();
    let inspector = Inspector::new(vec![]);
    let mut out = common::frame(4, 4);
    let _ = inspector.render_into(&input, &mut out);
    todo!("assert Error::InvalidFrame naming both sizes");
}

#[test]
fn render_into_overwrites_the_target() {
    let input = common::input(common::filled_frame(8, 8, [1, 2, 3, 255]))
        .windows(vec![common::window(1, common::rect(0, 0, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let mut out = common::filled_frame(8, 8, [9, 9, 9, 255]);
    let _ = inspector.render_into(&input, &mut out);
    todo!("assert out starts from input.frame and receives the bounds outline");
}

#[test]
fn render_request_crops_region() {
    let input = common::input(common::frame(16, 12)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new().region(common::rect(4, 3, 8, 6));
    let _out = inspector.render_request(&input, &request);
    todo!("assert the result is 8x6 and equals the (4,3,8,6) sub-frame of the composed frame");
}

#[test]
fn render_request_downscales_to_max_dimension() {
    let input = common::input(common::frame(64, 32)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new().max_dimension(16);
    let _out = inspector.render_request(&input, &request);
    todo!("assert the result is 16x8 via adesk-render's box filter");
}

#[test]
fn render_request_applies_region_before_max_dimension() {
    let input = common::input(common::frame(64, 64)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new()
        .region(common::rect(0, 0, 32, 16))
        .max_dimension(8);
    let _out = inspector.render_request(&input, &request);
    todo!("assert the result is 8x4 (crop first, then downscale)");
}

#[test]
fn render_request_rejects_empty_region_and_zero_max_dimension() {
    let input = common::input(common::frame(16, 16)).build();
    let inspector = Inspector::new(vec![]);
    let empty = InspectionRequest::new().region(common::rect(100, 100, 4, 4));
    let zero = InspectionRequest::new().max_dimension(0);
    let _ = (inspector.render_request(&input, &empty), inspector.render_request(&input, &zero));
    todo!("assert both are Error::InvalidRequest and map to ErrorCode::InvalidRequest");
}

#[test]
fn render_request_identity_returns_the_composed_frame() {
    let input = common::input(common::frame(16, 16))
        .windows(vec![common::window(1, common::rect(0, 0, 16, 16))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let _ = inspector.render_request(&input, &InspectionRequest::IDENTITY);
    todo!("assert render_request(IDENTITY) == render()");
}

#[test]
fn render_from_source_uses_the_source_snapshot() {
    let source = FixedSource {
        input: common::input(common::frame(32, 16))
            .windows(vec![common::window(1, common::rect(0, 0, 32, 16))])
            .build(),
    };
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let _out = inspector.render_from_source(&source);
    todo!("assert the result equals inspector.render(&source.input) and that overlays were passed through");
}

#[test]
fn composed_frame_keeps_input_dimensions_for_every_overlay_set() {
    let frame: ImageBuffer = common::frame(128, 80);
    let mut input = common::input(frame).build();
    input.cursor = Some(adesk_core::Point { x: 1, y: 1 });
    let inspector = Inspector::new(common::ALL_OVERLAYS.to_vec());
    let _out = inspector.render(&input);
    todo!("assert the output is 128x80 for the full overlay set");
}
