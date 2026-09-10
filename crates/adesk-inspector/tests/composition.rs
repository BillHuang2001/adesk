//! End-to-end composition behaviour: canonical order, determinism, and the
//! `region` / `max_dimension` post-processing path.

mod common;

use std::sync::Mutex;

use adesk_core::{ImageBuffer, OverlayKind, Size, WindowId};
use adesk_inspector::{
    blend_over, Error, InspectionInput, InspectionRequest, InspectionSource, Inspector, Result,
};

/// Test double for the trait `adesk-server` implements.
///
/// It also records the overlay slice it was asked for, so tests can assert that
/// [`Inspector::render_from_source`] forwards its normalized overlay set.
struct FixedSource {
    input: InspectionInput,
    seen: Mutex<Vec<OverlayKind>>,
}

impl FixedSource {
    /// A source that snapshots `input` and records the requested overlays.
    fn new(input: InspectionInput) -> FixedSource {
        FixedSource {
            input,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Overlay kinds passed to the last `inspection_input` call.
    fn seen(&self) -> Vec<OverlayKind> {
        self.seen.lock().expect("recording mutex poisoned").clone()
    }
}

impl InspectionSource for FixedSource {
    fn inspection_input(&self, overlays: &[OverlayKind]) -> Result<InspectionInput> {
        *self.seen.lock().expect("recording mutex poisoned") = overlays.to_vec();
        Ok(self.input.clone())
    }
}

/// Frame whose pixels encode their own index, so crop/downscale assertions
/// cannot pass by accident on a uniform frame.
fn marked_frame(width: u32, height: u32) -> ImageBuffer {
    let mut frame = common::frame(width, height);
    for (index, pixel) in frame.data.chunks_exact_mut(4).enumerate() {
        let value = (index % 251) as u8;
        pixel.copy_from_slice(&[value, value.wrapping_add(1), value.wrapping_add(2), 255]);
    }
    frame
}

/// Every overlay must leave the frame byte-identical when the state it renders
/// is absent (no windows, no active window, no cursor, no commit, no damage),
/// and an empty overlay set must copy the frame verbatim. Each case still
/// asserts full-frame equivalence, so sharing the harness weakens nothing.
#[test]
fn overlays_draw_nothing_without_their_data() {
    let cases: Vec<(&str, Vec<OverlayKind>, InspectionInput)> = vec![
        (
            "empty overlay set",
            vec![],
            common::input(common::filled_frame(16, 16, [7, 8, 9, 255]))
                .damage(vec![common::rect(0, 0, 4, 4)])
                .build(),
        ),
        (
            "window_ids + app_ids without windows",
            vec![OverlayKind::WindowIds, OverlayKind::AppIds],
            common::input(common::frame(64, 48)).build(),
        ),
        (
            "window_ids with an empty window geometry",
            vec![OverlayKind::WindowIds],
            common::input(common::frame(64, 48))
                .windows(vec![common::window(1, common::rect(0, 0, 0, 0))])
                .build(),
        ),
        (
            "focus without an active window",
            vec![OverlayKind::Focus],
            common::input(common::frame(64, 48))
                .windows(vec![common::window(1, common::rect(0, 0, 32, 24))])
                .build(),
        ),
        (
            "focus with an active id that matches no window",
            vec![OverlayKind::Focus],
            common::input(common::frame(64, 48))
                .windows(vec![common::window(1, common::rect(0, 0, 32, 24))])
                .active(Some(WindowId(99)))
                .build(),
        ),
        (
            "damage with empty rects",
            vec![OverlayKind::Damage],
            common::input(common::frame(32, 32))
                .damage(vec![common::rect(4, 4, 0, 8), common::rect(4, 4, 8, 0)])
                .build(),
        ),
        (
            "cursor without a cursor position",
            vec![OverlayKind::Cursor],
            common::input(common::frame(16, 16)).build(),
        ),
        (
            "commit_timing without a commit",
            vec![OverlayKind::CommitTiming],
            common::input(common::frame(96, 48)).build(),
        ),
    ];

    for (name, overlays, input) in cases {
        let out = Inspector::new(overlays)
            .render(&input)
            .unwrap_or_else(|error| panic!("{name}: render failed: {error}"));
        assert_eq!(out.size(), input.frame.size(), "{name}: frame size");
        assert_eq!(
            out.data, input.frame.data,
            "{name}: the frame must be copied verbatim"
        );
        assert_eq!(
            common::diff_bytes(&out, &input.frame),
            0,
            "{name}: not a single byte may differ"
        );
    }
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
    let forward_out = forward.render(&input).expect("forward set renders");
    let reversed_out = reversed.render(&input).expect("reversed set renders");
    assert_eq!(
        forward.overlays(),
        reversed.overlays(),
        "both inspectors must normalize to the same canonical set"
    );
    assert_eq!(
        forward_out.data, reversed_out.data,
        "caller order must not change a single byte (canonical order)"
    );
    assert_eq!(common::diff_bytes(&forward_out, &reversed_out), 0);
    // Guard against a vacuous pass: this overlay set must actually paint.
    assert_ne!(
        forward_out.data, input.frame.data,
        "damage and window_ids must paint pixels for this comparison to mean anything"
    );
}

#[test]
fn duplicate_overlays_are_deduplicated() {
    let input = common::input(common::frame(64, 48))
        .windows(vec![common::window(1, common::rect(0, 0, 64, 48))])
        .build();
    let once = Inspector::new(vec![OverlayKind::WindowIds]);
    let twice = Inspector::new(vec![OverlayKind::WindowIds, OverlayKind::WindowIds]);
    let once_out = once.render(&input).expect("single overlay renders");
    let twice_out = twice.render(&input).expect("duplicated overlay renders");
    assert_eq!(
        twice.overlays(),
        once.overlays(),
        "a duplicate kind must be dropped at construction"
    );
    assert_eq!(
        twice_out.data, once_out.data,
        "listing window_ids twice must not blend it twice"
    );
    assert_eq!(common::diff_bytes(&once_out, &twice_out), 0);
    // Guard against a vacuous pass: the label must actually be painted.
    assert_ne!(
        once_out.data, input.frame.data,
        "window_ids must paint the window label for this comparison to mean anything"
    );
}

#[test]
fn damage_is_drawn_below_geometry_overlays() {
    let input = common::input(common::frame(32, 32))
        .windows(vec![common::window(1, common::rect(0, 0, 32, 32))])
        .damage(vec![common::rect(0, 0, 32, 32)])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds, OverlayKind::Damage]);
    let out = inspector.render(&input).expect("damage + bounds render");
    let style = inspector.style;
    // damage paints first, surface_bounds over it: every perimeter pixel of the
    // window (which is also the damage rect) keeps the opaque outline colour.
    for x in 0..32 {
        assert_eq!(
            common::px(&out, x, 0),
            style.outline.to_array(),
            "top edge pixel ({x},0) must keep the bounds outline"
        );
        assert_eq!(
            common::px(&out, x, 31),
            style.outline.to_array(),
            "bottom edge pixel ({x},31) must keep the bounds outline"
        );
    }
    for y in 0..32 {
        assert_eq!(
            common::px(&out, 0, y),
            style.outline.to_array(),
            "left edge pixel (0,{y}) must keep the bounds outline"
        );
        assert_eq!(
            common::px(&out, 31, y),
            style.outline.to_array(),
            "right edge pixel (31,{y}) must keep the bounds outline"
        );
    }
    // The interior keeps the translucent damage fill, so both painters ran.
    assert_ne!(
        common::px(&out, 16, 16),
        common::px(&input.frame, 16, 16),
        "the damage fill must be visible inside the window"
    );
    // The perimeter check above holds for either order (the damage rect
    // outlines the very same pixels). A window inset from the frame edge with a
    // frame-sized damage rect does discriminate: (10,4) lies on the window's
    // top edge and in the *interior* of the damage rect, so only the outline
    // colour may be visible there.
    let overlapping = common::input(common::frame(32, 32))
        .windows(vec![common::window(1, common::rect(4, 4, 24, 24))])
        .damage(vec![common::rect(0, 0, 32, 32)])
        .build();
    let over = inspector
        .render(&overlapping)
        .expect("overlapping damage render");
    for (x, y) in [(10, 4), (4, 10)] {
        assert_eq!(
            common::px(&over, x, y),
            style.outline.to_array(),
            "({x},{y}) is on the window edge but inside the damage fill: the outline wins"
        );
        assert_ne!(
            common::px(&over, x, y),
            blend_over(style.fill, style.outline.to_array()),
            "({x},{y}) must not be a blend of the damage fill over the bounds outline"
        );
    }
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
    let first = inspector.render(&input).expect("first render");
    let second = inspector.render(&input).expect("second render");
    assert_eq!(
        first.data, second.data,
        "the same input must always produce the same bytes"
    );
    assert_eq!(common::diff_bytes(&first, &second), 0);
    assert_eq!(first.size(), input.frame.size());
    // Guard against a vacuous pass: the default overlay set must paint.
    assert_ne!(
        first.data, input.frame.data,
        "window_ids/focus/damage must paint something for this comparison to mean anything"
    );
}

#[test]
fn render_into_requires_matching_dimensions() {
    let input = common::input(common::frame(8, 8)).build();
    let inspector = Inspector::new(vec![]);
    let mut out = common::frame(4, 4);
    let error = inspector
        .render_into(&input, &mut out)
        .expect_err("a 4x4 target cannot hold an 8x8 frame");
    match &error {
        Error::InvalidFrame(message) => {
            assert!(
                message.contains("8x8"),
                "message must name the frame size, got {message:?}"
            );
            assert!(
                message.contains("4x4"),
                "message must name the target size, got {message:?}"
            );
        }
        other => panic!("expected Error::InvalidFrame, got {other:?}"),
    }
    let displayed = error.to_string();
    assert!(
        displayed.contains("8x8") && displayed.contains("4x4"),
        "Display must name both sizes, got {displayed:?}"
    );
    assert_eq!(
        adesk_core::Error::from(error).code,
        adesk_core::ErrorCode::RenderFailed,
        "InvalidFrame maps to render_failed"
    );
}

#[test]
fn render_into_overwrites_the_target() {
    let input = common::input(common::filled_frame(8, 8, [1, 2, 3, 255]))
        .windows(vec![common::window(1, common::rect(0, 0, 8, 8))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let expected = inspector.render(&input).expect("reference render");
    let mut out = common::filled_frame(8, 8, [9, 9, 9, 255]);
    inspector
        .render_into(&input, &mut out)
        .expect("matching target");
    assert_eq!(
        out.data, expected.data,
        "render_into must produce exactly render()'s bytes"
    );
    assert_eq!(
        common::px(&out, 4, 4),
        [1, 2, 3, 255],
        "interior pixels come from input.frame, not from the target"
    );
    assert_eq!(
        common::px(&out, 0, 0),
        inspector.style.outline.to_array(),
        "the window boundary carries the bounds outline"
    );
    assert_eq!(
        common::count_pixels(&out, [9, 9, 9, 255]),
        0,
        "no pre-filled target pixel may survive"
    );
}

#[test]
fn render_request_crops_region() {
    let input = common::input(common::frame(16, 12)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new().region(common::rect(4, 3, 8, 6));
    let out = inspector
        .render_request(&input, &request)
        .expect("region request");
    let expected = adesk_render::crop(&input.frame, common::rect(4, 3, 8, 6));
    assert_eq!(out.size(), Size { w: 8, h: 6 });
    assert_eq!(
        out.data, expected.data,
        "the region must crop the composed frame through adesk_render::crop"
    );
    // The frozen frame is uniform, so pin the crop origin with a marked frame.
    let marked = common::input(marked_frame(16, 12)).build();
    let marked_out = inspector
        .render_request(&marked, &request)
        .expect("region request on marked frame");
    let marked_expected = adesk_render::crop(&marked.frame, common::rect(4, 3, 8, 6));
    assert_eq!(marked_out.size(), Size { w: 8, h: 6 });
    assert_eq!(marked_out.data, marked_expected.data);
    assert_ne!(
        marked_out.data, marked.frame.data,
        "a crop must not return the whole frame"
    );
}

#[test]
fn render_request_downscales_to_max_dimension() {
    let input = common::input(common::frame(64, 32)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new().max_dimension(16);
    let out = inspector
        .render_request(&input, &request)
        .expect("max_dimension request");
    let expected = adesk_render::downscale(&input.frame, 16);
    assert_eq!(
        out.size(),
        Size { w: 16, h: 8 },
        "64x32 scaled to a 16 px longest edge is 16x8"
    );
    assert_eq!(
        out.data, expected.data,
        "the result must come from adesk_render::downscale"
    );
    // The frozen frame is uniform, so pin the scaling with a marked frame.
    let marked = common::input(marked_frame(64, 32)).build();
    let marked_out = inspector
        .render_request(&marked, &request)
        .expect("max_dimension request on marked frame");
    assert_eq!(marked_out.size(), Size { w: 16, h: 8 });
    assert_eq!(
        marked_out.data,
        adesk_render::downscale(&marked.frame, 16).data
    );
}

#[test]
fn render_request_applies_region_before_max_dimension() {
    let input = common::input(common::frame(64, 64)).build();
    let inspector = Inspector::new(vec![]);
    let request = InspectionRequest::new()
        .region(common::rect(0, 0, 32, 16))
        .max_dimension(8);
    let out = inspector
        .render_request(&input, &request)
        .expect("region + max_dimension request");
    assert_eq!(
        out.size(),
        Size { w: 8, h: 4 },
        "the region is cropped to 32x16 first, then downscaled to an 8 px longest edge"
    );
    let cropped = adesk_render::crop(&input.frame, common::rect(0, 0, 32, 16));
    assert_eq!(out.data, adesk_render::downscale(&cropped, 8).data);
    // Downscaling first would yield 8x8 and leave no 32x16 region to crop.
    assert_ne!(out.size(), adesk_render::downscale(&input.frame, 8).size());
    // Pin the order with a marked frame as well.
    let marked = common::input(marked_frame(64, 64)).build();
    let marked_out = inspector
        .render_request(&marked, &request)
        .expect("region + max_dimension request on marked frame");
    let marked_cropped = adesk_render::crop(&marked.frame, common::rect(0, 0, 32, 16));
    assert_eq!(marked_out.size(), Size { w: 8, h: 4 });
    assert_eq!(
        marked_out.data,
        adesk_render::downscale(&marked_cropped, 8).data
    );
}

#[test]
fn render_request_rejects_empty_region_and_zero_max_dimension() {
    let input = common::input(common::frame(16, 16)).build();
    let inspector = Inspector::new(vec![]);
    let empty = InspectionRequest::new().region(common::rect(100, 100, 4, 4));
    let zero = InspectionRequest::new().max_dimension(0);
    for (label, request) in [("empty region", empty), ("zero max_dimension", zero)] {
        let error = inspector
            .render_request(&input, &request)
            .expect_err("invalid request must be rejected");
        assert!(
            matches!(&error, Error::InvalidRequest(_)),
            "{label}: expected Error::InvalidRequest, got {error:?}"
        );
        let mapped = adesk_core::Error::from(error);
        assert_eq!(
            mapped.code,
            adesk_core::ErrorCode::InvalidRequest,
            "{label}: must map to invalid_request, got {mapped}"
        );
    }
}

#[test]
fn render_request_identity_returns_the_composed_frame() {
    let input = common::input(common::frame(16, 16))
        .windows(vec![common::window(1, common::rect(0, 0, 16, 16))])
        .build();
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let composed = inspector.render(&input).expect("composed frame");
    let requested = inspector
        .render_request(&input, &InspectionRequest::IDENTITY)
        .expect("identity request");
    assert!(InspectionRequest::IDENTITY.is_identity());
    assert_eq!(requested.size(), composed.size());
    assert_eq!(
        requested.data, composed.data,
        "an identity request must return the composed frame unchanged"
    );
    assert_eq!(common::diff_bytes(&requested, &composed), 0);
}

#[test]
fn render_from_source_uses_the_source_snapshot() {
    let source = FixedSource::new(
        common::input(common::frame(32, 16))
            .windows(vec![common::window(1, common::rect(0, 0, 32, 16))])
            .build(),
    );
    let inspector = Inspector::new(vec![OverlayKind::SurfaceBounds]);
    let out = inspector
        .render_from_source(&source)
        .expect("render from source");
    let expected = inspector
        .render(&source.input)
        .expect("reference render of the snapshot");
    assert_eq!(out.size(), expected.size());
    assert_eq!(
        out.data, expected.data,
        "render_from_source must render the source snapshot verbatim"
    );
    assert_eq!(
        source.seen(),
        vec![OverlayKind::SurfaceBounds],
        "the inspector must forward its overlay set to the source"
    );
    assert_eq!(source.seen(), inspector.normalized_overlays());
}

#[test]
fn composed_frame_keeps_input_dimensions_for_every_overlay_set() {
    let frame: ImageBuffer = common::frame(128, 80);
    let mut input = common::input(frame).build();
    input.cursor = Some(adesk_core::Point { x: 1, y: 1 });
    let inspector = Inspector::new(common::ALL_OVERLAYS.to_vec());
    let out = inspector.render(&input).expect("full overlay set renders");
    assert_eq!(out.size(), input.frame.size());
    assert_eq!(out.size(), Size { w: 128, h: 80 });
    assert_eq!(out.data.len(), 128 * 80 * 4);
}
