//! Shared fixtures and pixel helpers for the inspector integration tests.
//!
//! These helpers are implemented (they are test scaffolding); the assertions
//! themselves land in Phase 2 with the painters.

#![allow(dead_code)]

use adesk_core::{AppId, ImageBuffer, OverlayKind, Rect, WindowId, WindowInfo, WindowState};
use adesk_inspector::{InspectionInput, InspectionInputBuilder};

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

/// Number of pixels exactly equal to `color`.
pub fn count_pixels(frame: &ImageBuffer, color: [u8; 4]) -> usize {
    frame
        .data
        .chunks_exact(4)
        .filter(|pixel| *pixel == color)
        .count()
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
