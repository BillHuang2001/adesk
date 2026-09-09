//! Single-line text layout and drawing on top of [`Canvas`].
//!
//! Layout is monospace and purely integer: every glyph advances
//! [`font::advance`] px, ink width is [`font::glyph_width`] px, and the ink
//! width of an `n`-character string is `n * advance - (advance - glyph_width)`
//! (the trailing inter-glyph gap is not part of the ink).
//!
//! Only printable ASCII is rendered; every other character (including `\n`)
//! draws [`font::FALLBACK_CHAR`]. Text is always a single line.

use std::borrow::Cow;

use adesk_core::{Point, Rect, Size};

use crate::canvas::Canvas;
use crate::color::Color;
use crate::style::OverlayStyle;

/// Ink size of `text` at `scale`, ignoring clipping. Zero for empty text.
pub fn measure(text: &str, scale: u8) -> Size {
    let _ = (text, scale);
    todo!("Phase 2: n * font::advance(scale) - (font::advance(scale) - font::glyph_width(scale))")
}

/// Draws `text` with its ink top-left at `origin` and returns the intended ink
/// rect (before clipping). Characters outside the font table draw `?`.
pub fn draw(canvas: &mut Canvas<'_>, origin: Point, text: &str, scale: u8, color: Color) -> Rect {
    let _ = (canvas, origin, text, scale, color);
    todo!("Phase 2: blit each glyph with nearest-neighbour scaling through Canvas::pixel")
}

/// Longest prefix of `text` that fits `max_width` px at `scale`, with `".."`
/// appended when truncation is needed. `None` when not even `".."` fits.
pub fn elide<'a>(text: &'a str, max_width: i32, scale: u8) -> Option<Cow<'a, str>> {
    let _ = (text, max_width, scale);
    todo!("Phase 2: longest prefix p with measure(p + \"..\") <= max_width, else None")
}

/// Plate rect a label occupies for `text` at `origin`: the ink rect inflated by
/// [`OverlayStyle::pad`] px on every side. Empty text yields an empty rect.
pub fn label_rect(origin: Point, text: &str, style: &OverlayStyle) -> Rect {
    let _ = (origin, text, style);
    todo!("Phase 2: text::measure inflated by style.pad()")
}

/// Draws a label: plate fill ([`OverlayStyle::plate`]), 1 px plate border
/// ([`OverlayStyle::outline`]) and text ([`OverlayStyle::text`]), with the
/// plate's top-left at `origin`. Returns the plate rect. Text is drawn exactly
/// as given (callers elide first); everything is clipped by the canvas.
pub fn draw_label(
    canvas: &mut Canvas<'_>,
    origin: Point,
    text: &str,
    style: &OverlayStyle,
) -> Rect {
    let _ = (canvas, origin, text, style);
    todo!("Phase 2: fill plate, outline plate, draw text at origin + pad")
}
