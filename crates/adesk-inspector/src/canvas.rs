//! Pixel drawing surface with clipping and documented alpha blending.
//!
//! [`Canvas`] is the only place in the crate that writes pixels. Every painter
//! draws through it, so clipping and blending semantics are uniform and
//! pixel-assertable:
//!
//! ```text
//! out = (src * src.a + dst * (255 - src.a) + 127) / 255        (per RGB channel)
//! a   = src.a + (dst.a * (255 - src.a) + 127) / 255            (alpha)
//! ```
//!
//! All arithmetic is integer, so results are exact and reproducible.
//! `src.a == 0` never changes the buffer; `src.a == 255` writes `src` exactly.

use adesk_core::{ImageBuffer, Point, Rect, Size};

use crate::color::Color;

/// Mutable drawing surface over an [`ImageBuffer`] with a clip rectangle.
///
/// Draws are confined to `clip ∩ buffer bounds`; nothing outside is touched.
pub struct Canvas<'a> {
    buffer: &'a mut ImageBuffer,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    /// Wraps `buffer`; the initial clip is the whole buffer.
    pub fn new(buffer: &'a mut ImageBuffer) -> Canvas<'a> {
        let clip = buffer.rect();
        Canvas { buffer, clip }
    }

    /// Size of the underlying buffer.
    pub fn size(&self) -> Size {
        self.buffer.size()
    }

    /// Current clip rectangle.
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Replaces the clip rectangle; an empty clip discards every draw.
    pub fn set_clip(&mut self, clip: Rect) {
        self.clip = clip;
    }

    /// Runs `f` with the clip narrowed to `clip`, restoring the previous clip
    /// afterwards.
    pub fn with_clip<R>(&mut self, clip: Rect, f: impl FnOnce(&mut Canvas<'_>) -> R) -> R {
        let previous = self.clip;
        self.clip = previous.intersect(&clip).unwrap_or(Rect::EMPTY);
        let result = f(self);
        self.clip = previous;
        result
    }

    /// Alpha-blends `color` over the pixel at `p`; no-op outside the clip.
    pub fn pixel(&mut self, p: Point, color: Color) {
        let _ = (p, color);
        todo!("Phase 2: bounds + clip check, then blend_over")
    }

    /// Alpha-blends `color` over every pixel of `rect` inside the clip.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let _ = (rect, color);
        todo!("Phase 2: clip rect, then blend each pixel")
    }

    /// Draws a 1 px outline on the boundary pixels of `rect` (inside its own
    /// bounds: top row, bottom row, left column, right column).
    pub fn outline_rect(&mut self, rect: Rect, color: Color) {
        let _ = (rect, color);
        todo!("Phase 2: hline/fill top and bottom rows, vline left and right columns")
    }

    /// Draws a horizontal line from `x0` to `x1` inclusive at row `y`.
    pub fn hline(&mut self, y: i32, x0: i32, x1: i32, color: Color) {
        let _ = (y, x0, x1, color);
        todo!("Phase 2: normalize x0 <= x1 and blend each pixel in the clip")
    }

    /// Draws a vertical line from `y0` to `y1` inclusive at column `x`.
    pub fn vline(&mut self, x: i32, y0: i32, y1: i32, color: Color) {
        let _ = (x, y0, y1, color);
        todo!("Phase 2: normalize y0 <= y1 and blend each pixel in the clip")
    }

    /// Draws a 1 px Bresenham line from `from` to `to`, both endpoints included.
    pub fn line(&mut self, from: Point, to: Point, color: Color) {
        let _ = (from, to, color);
        todo!("Phase 2: integer Bresenham through Canvas::pixel")
    }
}

/// Alpha-blends `src` over the destination pixel `dst` using the documented
/// integer formula. Public so tests and callers can compute expected pixels.
pub fn blend_over(src: Color, dst: [u8; 4]) -> [u8; 4] {
    let _ = (src, dst);
    todo!("Phase 2: (src * src.a + dst * (255 - src.a) + 127) / 255 per channel")
}
