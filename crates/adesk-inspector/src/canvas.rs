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
        if let Some(offset) = self.offset(p) {
            self.blend_at(offset, color);
        }
    }

    /// Alpha-blends `color` over every pixel of `rect` inside the clip.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let Some(area) = self.visible(rect) else {
            return;
        };
        let stride = self.buffer.stride as usize;
        for y in area.y..area.bottom() {
            let row = y as usize * stride;
            for x in area.x..area.right() {
                self.blend_at(row + x as usize * 4, color);
            }
        }
    }

    /// Draws a 1 px outline on the boundary pixels of `rect` (inside its own
    /// bounds: top row, bottom row, left column, right column).
    ///
    /// Corners belong to the horizontal edges, so a `w == 1` or `h == 1` rect
    /// draws each of its pixels exactly once and translucent colours never
    /// blend a boundary pixel twice.
    pub fn outline_rect(&mut self, rect: Rect, color: Color) {
        if rect.is_empty() {
            return;
        }
        let (x0, y0) = (rect.x, rect.y);
        let x1 = rect.right() - 1;
        let y1 = rect.bottom() - 1;
        self.hline(y0, x0, x1, color);
        if y1 > y0 {
            self.hline(y1, x0, x1, color);
        }
        if y1 > y0 + 1 {
            self.vline(x0, y0 + 1, y1 - 1, color);
            if x1 > x0 {
                self.vline(x1, y0 + 1, y1 - 1, color);
            }
        }
    }

    /// Draws a horizontal line from `x0` to `x1` inclusive at row `y`.
    pub fn hline(&mut self, y: i32, x0: i32, x1: i32, color: Color) {
        let (left, right) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        self.fill_rect(span(left, y, right, y), color);
    }

    /// Draws a vertical line from `y0` to `y1` inclusive at column `x`.
    pub fn vline(&mut self, x: i32, y0: i32, y1: i32, color: Color) {
        let (top, bottom) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        self.fill_rect(span(x, top, x, bottom), color);
    }

    /// Draws a 1 px Bresenham line from `from` to `to`, both endpoints included.
    pub fn line(&mut self, from: Point, to: Point, color: Color) {
        let (x0, y0) = (i64::from(from.x), i64::from(from.y));
        let (x1, y1) = (i64::from(to.x), i64::from(to.y));
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let step_x = if x0 < x1 { 1 } else { -1 };
        let step_y = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.pixel(
                Point {
                    x: x as i32,
                    y: y as i32,
                },
                color,
            );
            if x == x1 && y == y1 {
                break;
            }
            let doubled = 2 * error;
            if doubled >= dy {
                error += dy;
                x += step_x;
            }
            if doubled <= dx {
                error += dx;
                y += step_y;
            }
        }
    }

    /// Byte offset of `p` when it lies inside `clip ∩ buffer bounds`.
    fn offset(&self, p: Point) -> Option<usize> {
        if !self.clip.contains(p) || p.x < 0 || p.y < 0 {
            return None;
        }
        let (x, y) = (p.x as usize, p.y as usize);
        if x >= self.buffer.width as usize || y >= self.buffer.height as usize {
            return None;
        }
        let offset = y * self.buffer.stride as usize + x * 4;
        if offset + 4 > self.buffer.data.len() {
            return None;
        }
        Some(offset)
    }

    /// `rect ∩ clip ∩ buffer bounds`; `None` when nothing is drawable.
    fn visible(&self, rect: Rect) -> Option<Rect> {
        rect.intersect(&self.buffer.rect())?.intersect(&self.clip)
    }

    /// Blends `color` over the pixel starting at `offset`; `offset` must be
    /// inside the buffer.
    fn blend_at(&mut self, offset: usize, color: Color) {
        let dst = [
            self.buffer.data[offset],
            self.buffer.data[offset + 1],
            self.buffer.data[offset + 2],
            self.buffer.data[offset + 3],
        ];
        let blended = blend_over(color, dst);
        self.buffer.data[offset..offset + 4].copy_from_slice(&blended);
    }
}

/// Inclusive rectangle spanned by two corner points.
fn span(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
    let width = (i64::from(x1) - i64::from(x0) + 1).min(i64::from(u32::MAX)) as u32;
    let height = (i64::from(y1) - i64::from(y0) + 1).min(i64::from(u32::MAX)) as u32;
    Rect {
        x: x0,
        y: y0,
        w: width,
        h: height,
    }
}

/// Alpha-blends `src` over the destination pixel `dst` using the documented
/// integer formula. Public so tests and callers can compute expected pixels.
pub fn blend_over(src: Color, dst: [u8; 4]) -> [u8; 4] {
    let alpha = u32::from(src.a);
    let inverse = 255 - alpha;
    let channel = |src_channel: u8, dst_channel: u8| -> u8 {
        let mixed = u32::from(src_channel) * alpha + u32::from(dst_channel) * inverse + 127;
        (mixed / 255) as u8
    };
    [
        channel(src.r, dst[0]),
        channel(src.g, dst[1]),
        channel(src.b, dst[2]),
        (alpha + (u32::from(dst[3]) * inverse + 127) / 255) as u8,
    ]
}
