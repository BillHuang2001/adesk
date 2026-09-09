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
use crate::font;
use crate::style::OverlayStyle;

/// Ink width in px of `glyphs` consecutive glyphs at `scale`; the trailing
/// inter-glyph gap is not part of the ink, so the last glyph contributes
/// [`font::glyph_width`] rather than [`font::advance`].
fn ink_width(glyphs: usize, scale: u8) -> i64 {
    if glyphs == 0 {
        return 0;
    }
    let advance = i64::from(font::advance(scale));
    glyphs as i64 * advance - (advance - i64::from(font::glyph_width(scale)))
}

/// Ink size of `text` at `scale`, ignoring clipping. Zero for empty text.
pub fn measure(text: &str, scale: u8) -> Size {
    let glyphs = text.chars().count();
    if glyphs == 0 {
        return Size { w: 0, h: 0 };
    }
    let width = ink_width(glyphs, scale).clamp(0, i64::from(u32::MAX)) as u32;
    let height = font::glyph_height(scale).max(0) as u32;
    Size {
        w: width,
        h: height,
    }
}

/// Draws `text` with its ink top-left at `origin` and returns the intended ink
/// rect (before clipping). Characters outside the font table draw `?`.
pub fn draw(canvas: &mut Canvas<'_>, origin: Point, text: &str, scale: u8, color: Color) -> Rect {
    if text.is_empty() {
        return Rect::EMPTY;
    }
    let size = measure(text, scale);
    let scale = font::clamp_scale(scale);
    let block = u32::from(scale);
    let advance = font::advance(scale);
    let mut pen_x = origin.x;
    for c in text.chars() {
        let glyph = font::glyph(c).unwrap_or_else(font::fallback);
        for gy in 0..usize::from(font::FONT_HEIGHT) {
            let y = origin
                .y
                .saturating_add((gy as i32).saturating_mul(i32::from(scale)));
            for gx in 0..usize::from(font::FONT_WIDTH) {
                if !glyph.ink(gx, gy) {
                    continue;
                }
                let x = pen_x.saturating_add((gx as i32).saturating_mul(i32::from(scale)));
                canvas.fill_rect(Rect::new(x, y, block, block), color);
            }
        }
        pen_x = pen_x.saturating_add(advance);
    }
    Rect {
        x: origin.x,
        y: origin.y,
        w: size.w,
        h: size.h,
    }
}

/// Longest prefix of `text` that fits `max_width` px at `scale`, with `".."`
/// appended when truncation is needed. `None` when not even `".."` fits.
pub fn elide<'a>(text: &'a str, max_width: i32, scale: u8) -> Option<Cow<'a, str>> {
    let max_width = i64::from(max_width);
    let glyphs = text.chars().count();
    if ink_width(glyphs, scale) <= max_width {
        return Some(Cow::Borrowed(text));
    }
    // `".."` alone must fit before any prefix can.
    if ink_width(2, scale) > max_width {
        return None;
    }
    let mut kept = "";
    for (prefix_glyphs, (end, _)) in text.char_indices().enumerate() {
        if ink_width(prefix_glyphs + 2, scale) > max_width {
            break;
        }
        kept = &text[..end];
    }
    Some(Cow::Owned(format!("{kept}..")))
}

/// Plate rect a label occupies for `text` at `origin`: the ink rect inflated by
/// [`OverlayStyle::pad`] px on every side. Empty text yields an empty rect.
pub fn label_rect(origin: Point, text: &str, style: &OverlayStyle) -> Rect {
    if text.is_empty() {
        return Rect::EMPTY;
    }
    let size = measure(text, style.scale());
    let pad = style.pad().max(0) as u32;
    Rect {
        x: origin.x.saturating_sub(pad as i32),
        y: origin.y.saturating_sub(pad as i32),
        w: size.w.saturating_add(2 * pad),
        h: size.h.saturating_add(2 * pad),
    }
}

/// Draws a label: plate fill ([`OverlayStyle::plate`]), 1 px plate border
/// ([`OverlayStyle::outline`]) and text ([`OverlayStyle::text`]), with `origin`
/// as the ink top-left, so the plate is the ink rect inflated by
/// [`OverlayStyle::pad`] px. Returns the plate rect. Text is drawn exactly as
/// given (callers elide first); everything is clipped by the canvas.
pub fn draw_label(
    canvas: &mut Canvas<'_>,
    origin: Point,
    text: &str,
    style: &OverlayStyle,
) -> Rect {
    let plate = label_rect(origin, text, style);
    if plate.is_empty() {
        return plate;
    }
    canvas.fill_rect(plate, style.plate);
    canvas.outline_rect(plate, style.outline);
    draw(canvas, origin, text, style.scale(), style.text);
    plate
}
