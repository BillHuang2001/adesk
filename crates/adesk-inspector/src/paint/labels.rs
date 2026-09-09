//! Shared label layout for the window-anchored overlays.
//!
//! `window_ids`, `app_ids` and `focus` each label a window. Labels stack in
//! fixed slots at the window's top-left corner so enabling a subset never
//! shifts the others:
//!
//! | slot | overlay |
//! |---|---|
//! | 0 | `window_ids` |
//! | 1 | `app_ids` |
//! | 2 | `focus` |
//!
//! Slot `n`'s plate top-left sits at
//! `window.origin + (pad, pad + n * (line_height + pad))`.
//!
//! Labels are elided to the window's inner width (`window.w - 2 * pad`) with
//! [`text::elide`](crate::text::elide) and clipped to
//! `window.geometry ∩ canvas.clip()`, so a label never leaves its window.

#![allow(dead_code)] // Phase 2: referenced by the window-anchored painter stubs.

use adesk_core::{Point, Rect};

use crate::canvas::Canvas;
use crate::style::OverlayStyle;

/// Slot of the `window_ids` label.
pub(crate) const SLOT_WINDOW_IDS: u8 = 0;
/// Slot of the `app_ids` label.
pub(crate) const SLOT_APP_IDS: u8 = 1;
/// Slot of the `focus` label.
pub(crate) const SLOT_FOCUS: u8 = 2;

/// Plate top-left of slot `slot` inside `window`.
pub(crate) fn slot_origin(window: Rect, slot: u8, style: &OverlayStyle) -> Point {
    let _ = (window, slot);
    let pad = style.pad();
    let step = crate::font::line_height(style.scale()) + pad;
    Point {
        x: window.x + pad,
        y: window.y + pad + i32::from(slot) * step,
    }
}

/// Draws `text` in `slot` of `window`: elide to the window's inner width, clip
/// to `window ∩ canvas.clip()`, draw the label plate. Returns the drawn plate
/// rect, or `None` when nothing fits (empty window, or not even `".."` fits).
pub(crate) fn draw(
    canvas: &mut Canvas<'_>,
    window: Rect,
    slot: u8,
    text: &str,
    style: &OverlayStyle,
) -> Option<Rect> {
    let _ = (canvas, window, slot, text, style);
    todo!("Phase 2: elide to window.w - 2 * pad, then draw_label inside the window clip")
}
