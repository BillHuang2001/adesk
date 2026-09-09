//! `surface_bounds` overlay: 1 px outline around every window geometry.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Outlines every [`WindowInfo::geometry`](adesk_core::WindowInfo::geometry)
/// with [`OverlayStyle::outline`] in input order.
///
/// Empty geometries are skipped; rects are clipped to the canvas.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    for window in &input.windows {
        if window.geometry.is_empty() {
            continue;
        }
        canvas.outline_rect(window.geometry, style.outline);
    }
}
