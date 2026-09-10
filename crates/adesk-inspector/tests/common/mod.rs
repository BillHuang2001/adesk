//! Shared fixtures and pixel helpers for the inspector integration tests.
//!
//! Every helper here is exercised by at least one test binary; the module is
//! compiled once per binary, so `allow(dead_code)` silences the per-binary
//! "unused" warnings for the subset each one ignores.

#![allow(dead_code)]

use adesk_core::{AppId, ImageBuffer, OverlayKind, Point, Rect, WindowId, WindowInfo, WindowState};
use adesk_inspector::{font, text, Canvas, InspectionInput, InspectionInputBuilder, OverlayStyle};

/// Every overlay kind (core's `OverlayKind` has no `ALL` constant).
pub const ALL_OVERLAYS: [OverlayKind; 8] = [
    OverlayKind::WindowIds,
    OverlayKind::AppIds,
    OverlayKind::Focus,
    OverlayKind::Damage,
    OverlayKind::SurfaceBounds,
    OverlayKind::Cursor,
    OverlayKind::Actions,
    OverlayKind::CommitTiming,
];

/// Rectangle shorthand.
pub fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

/// Opaque-black RGBA frame (`ImageBuffer::new_rgba`).
pub fn frame(width: u32, height: u32) -> ImageBuffer {
    ImageBuffer::new_rgba(width, height)
}

/// Frame where every pixel is `color`.
pub fn filled_frame(width: u32, height: u32, color: [u8; 4]) -> ImageBuffer {
    let mut frame = ImageBuffer::new_rgba(width, height);
    for pixel in frame.data.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    frame
}

/// A mapped, inactive window without an app id.
pub fn window(id: u64, geometry: Rect) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: None,
        title: None,
        geometry,
        state: WindowState::Inactive,
        mapped: true,
        pid: None,
        created_seq: 1,
        last_commit_seq: 1,
        popup_count: 0,
    }
}

/// A mapped window carrying `app_id`.
pub fn window_with_app(id: u64, geometry: Rect, app_id: &str) -> WindowInfo {
    WindowInfo {
        app_id: Some(AppId::from(app_id)),
        ..window(id, geometry)
    }
}

/// Starts an input builder on `frame`.
pub fn input(frame: ImageBuffer) -> InspectionInputBuilder {
    InspectionInput::builder(frame)
}

/// Pixel value at `(x, y)`; panics when out of bounds.
pub fn px(frame: &ImageBuffer, x: u32, y: u32) -> [u8; 4] {
    frame.pixel(x, y).expect("pixel within bounds")
}

/// Number of `color` pixels inside `rect`, clipped to the frame.
pub fn count_in(frame: &ImageBuffer, rect: Rect, color: [u8; 4]) -> usize {
    let mut count = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            if x < 0 || y < 0 || x as u32 >= frame.width || y as u32 >= frame.height {
                continue;
            }
            if px(frame, x as u32, y as u32) == color {
                count += 1;
            }
        }
    }
    count
}

/// Number of pixels exactly equal to `color`.
pub fn count_pixels(frame: &ImageBuffer, color: [u8; 4]) -> usize {
    count_in(
        frame,
        Rect {
            x: 0,
            y: 0,
            w: frame.width,
            h: frame.height,
        },
        color,
    )
}

/// Coordinates of every pixel where `a` and `b` differ, in row-major order.
pub fn changed_pixels(a: &ImageBuffer, b: &ImageBuffer) -> Vec<(u32, u32)> {
    assert_eq!(a.size(), b.size(), "frames must have the same size");
    let mut changed = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            if px(a, x, y) != px(b, x, y) {
                changed.push((x, y));
            }
        }
    }
    changed
}

/// Asserts `a` and `b` are pixel-identical inside `region`.
pub fn assert_region_equal(a: &ImageBuffer, b: &ImageBuffer, region: Rect) {
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            assert_eq!(
                px(a, x as u32, y as u32),
                px(b, x as u32, y as u32),
                "pixel ({x}, {y}) must match"
            );
        }
    }
}

/// Reference composition of one label: the documented layout
/// (`slot_origin` = plate top-left, ink origin = plate + `pad`) drawn directly
/// through `text::draw_label`, clipped to the window.
///
/// A label painter must produce the same bytes; a mismatch means it deviated
/// from the layout spec, not from `draw_label`.
pub fn reference_label(
    base: &ImageBuffer,
    window: Rect,
    slot: u8,
    label: &str,
    style: &OverlayStyle,
) -> ImageBuffer {
    let pad = style.pad();
    let step = font::line_height(style.scale()) + pad;
    let ink_origin = Point {
        x: window.x + 2 * pad,
        y: window.y + 2 * pad + i32::from(slot) * step,
    };
    let mut out = base.clone();
    let mut canvas = Canvas::new(&mut out);
    canvas.with_clip(window, |clipped| {
        text::draw_label(clipped, ink_origin, label, style);
    });
    out
}

/// Number of differing bytes between two equally sized frames.
pub fn diff_bytes(a: &ImageBuffer, b: &ImageBuffer) -> usize {
    assert_eq!(a.size(), b.size(), "frames must have the same size");
    a.data
        .iter()
        .zip(&b.data)
        .filter(|(left, right)| left != right)
        .count()
}
