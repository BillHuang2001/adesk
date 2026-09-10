//! Helpers shared by more than one overlay painter.
//!
//! Two snippets recur across painters and live here so they have a single source
//! of truth:
//!
//! - `ink_origin`: the label "plate top-left → ink top-left" step. Every label
//!   painter positions the label's *plate*, while `text::draw_label` takes the
//!   *ink* top-left and derives the plate by inflating the ink rect by the style
//!   padding — so plate-layout painters offset by `pad`.
//! - `crosshair` (+ `ARM`): the two perpendicular `2 * ARM + 1` px arm lines
//!   shared by the `cursor` crosshair and the `actions` plus marker.

use adesk_core::Point;

use crate::canvas::Canvas;
use crate::color::Color;

/// Half-length of a crosshair arm in px: a crosshair spans `2 * ARM + 1 = 9` px
/// per axis, centre included.
pub(crate) const ARM: i32 = 4;

/// The label ink top-left `delta` px past `anchor` on both axes.
///
/// `text::draw_label` takes the label's *ink* top-left and derives the plate by
/// inflating the ink rect by the style padding. A painter that has positioned the
/// *plate* (window slots, HUDs, the right-aligned commit watermark) therefore
/// passes the plate top-left with `delta = style.pad()`; `actions` passes a marker
/// position with `delta = ARM` so the marker's label starts at its arm end.
pub(crate) fn ink_origin(anchor: Point, delta: i32) -> Point {
    Point {
        x: anchor.x.saturating_add(delta),
        y: anchor.y.saturating_add(delta),
    }
}

/// Draws a crosshair centred on `centre` in `color`: a horizontal line from
/// `centre.x - ARM` to `centre.x + ARM` at `centre.y`, and a vertical line from
/// `centre.y - ARM` to `centre.y + ARM` at `centre.x` (9 px arms, centre
/// included). Clipped to the canvas by the line primitives.
pub(crate) fn crosshair(canvas: &mut Canvas<'_>, centre: Point, color: Color) {
    canvas.hline(
        centre.y,
        centre.x.saturating_sub(ARM),
        centre.x.saturating_add(ARM),
        color,
    );
    canvas.vline(
        centre.x,
        centre.y.saturating_sub(ARM),
        centre.y.saturating_add(ARM),
        color,
    );
}
