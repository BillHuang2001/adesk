//! Built-in 5x7 monospace bitmap font.
//!
//! No font crates: the glyph table is a `const` array inside this module.
//! Metrics are fixed and documented so overlay pixels are assertable:
//!
//! - glyph ink: `5 x 7` px at scale 1,
//! - horizontal advance: `6` px per glyph (ink + 1 px spacing),
//! - line height: `8` px (ink + 1 px leading),
//! - every glyph is exactly `5` px wide (monospace).
//!
//! The table covers printable ASCII (`0x20..=0x7E`). Any other character is
//! rendered as [`FALLBACK_CHAR`] (`?`) by [`crate::text`].

/// Glyph ink height in pixels at scale 1.
pub const FONT_HEIGHT: u8 = 7;
/// Glyph ink width in pixels at scale 1.
pub const FONT_WIDTH: u8 = 5;
/// Horizontal advance per glyph at scale 1 (ink width + 1 px spacing).
pub const FONT_ADVANCE: u8 = 6;
/// Baseline-to-baseline distance at scale 1 (ink height + 1 px leading).
pub const FONT_LINE_HEIGHT: u8 = 8;
/// Number of glyphs in the built-in table (printable ASCII).
pub const GLYPH_COUNT: usize = 95;
/// First character covered by the table.
pub const FIRST_CHAR: char = ' ';
/// Last character covered by the table.
pub const LAST_CHAR: char = '~';
/// Character drawn for code points outside the table.
pub const FALLBACK_CHAR: char = '?';
/// Largest supported font scale.
pub const MAX_SCALE: u8 = 8;

/// Clamps a font scale into `1..=MAX_SCALE`; `0` is treated as `1`.
pub const fn clamp_scale(scale: u8) -> u8 {
    if scale < 1 {
        1
    } else if scale > MAX_SCALE {
        MAX_SCALE
    } else {
        scale
    }
}

/// One monospace 5x7 bitmap glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    /// One byte per row, top to bottom. Bits `4..=0` are the five columns with
    /// bit 4 = leftmost; set bits are ink.
    pub rows: [u8; FONT_HEIGHT as usize],
}

impl Glyph {
    /// Row bits at `y`, or `None` when out of range.
    pub fn row(&self, y: usize) -> Option<u8> {
        self.rows.get(y).copied()
    }

    /// Whether column `x` of row `y` is ink.
    pub fn ink(&self, x: usize, y: usize) -> bool {
        if x >= FONT_WIDTH as usize {
            return false;
        }
        match self.rows.get(y) {
            Some(row) => row & (1 << (FONT_WIDTH as usize - 1 - x)) != 0,
            None => false,
        }
    }
}

/// Glyph for a printable ASCII character; `None` for anything else.
pub fn glyph(c: char) -> Option<&'static Glyph> {
    let _ = c;
    todo!("Phase 2: index the const glyph table by `c as usize - FIRST_CHAR as usize`")
}

/// Glyph used for characters outside the table ([`FALLBACK_CHAR`]).
pub fn fallback() -> &'static Glyph {
    todo!("Phase 2: return the glyph for FALLBACK_CHAR")
}

/// Ink width of one glyph at `scale`.
pub const fn glyph_width(scale: u8) -> i32 {
    FONT_WIDTH as i32 * clamp_scale(scale) as i32
}

/// Ink height of one glyph at `scale`.
pub const fn glyph_height(scale: u8) -> i32 {
    FONT_HEIGHT as i32 * clamp_scale(scale) as i32
}

/// Horizontal advance per glyph at `scale`.
pub const fn advance(scale: u8) -> i32 {
    FONT_ADVANCE as i32 * clamp_scale(scale) as i32
}

/// Baseline-to-baseline distance at `scale`.
pub const fn line_height(scale: u8) -> i32 {
    FONT_LINE_HEIGHT as i32 * clamp_scale(scale) as i32
}
