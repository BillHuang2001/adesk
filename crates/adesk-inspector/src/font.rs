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

/// The built-in 5x7 glyph table: one entry per printable ASCII character in
/// [`FIRST_CHAR`]`..=`[`LAST_CHAR`] order (95 entries).
///
/// Every glyph is 5 px wide and 7 px tall; each row keeps its ink in bits
/// `4..=0` (bit 4 = leftmost column). The shapes are the classic public-domain
/// 5x7 ASCII designs, so text stays legible at scale 1 and scales by whole
/// pixels above it. Only the space glyph is blank.
#[rustfmt::skip]
const GLYPH_TABLE: [Glyph; GLYPH_COUNT] = [
    // ' '
    Glyph { rows: [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000] },
    // '!'
    Glyph { rows: [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100] },
    // '"'
    Glyph { rows: [0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000] },
    // '#'
    Glyph { rows: [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010] },
    // '$'
    Glyph { rows: [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100] },
    // '%'
    Glyph { rows: [0b11001, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b10011] },
    // '&'
    Glyph { rows: [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101] },
    // "'"
    Glyph { rows: [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000] },
    // '('
    Glyph { rows: [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010] },
    // ')'
    Glyph { rows: [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000] },
    // '*'
    Glyph { rows: [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000] },
    // '+'
    Glyph { rows: [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000] },
    // ','
    Glyph { rows: [0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000] },
    // '-'
    Glyph { rows: [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000] },
    // '.'
    Glyph { rows: [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100] },
    // '/'
    Glyph { rows: [0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000] },
    // '0'
    Glyph { rows: [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110] },
    // '1'
    Glyph { rows: [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110] },
    // '2'
    Glyph { rows: [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111] },
    // '3'
    Glyph { rows: [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110] },
    // '4'
    Glyph { rows: [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010] },
    // '5'
    Glyph { rows: [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110] },
    // '6'
    Glyph { rows: [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110] },
    // '7'
    Glyph { rows: [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000] },
    // '8'
    Glyph { rows: [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110] },
    // '9'
    Glyph { rows: [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100] },
    // ':'
    Glyph { rows: [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000] },
    // ';'
    Glyph { rows: [0b00000, 0b00100, 0b00100, 0b00000, 0b00110, 0b00100, 0b01000] },
    // '<'
    Glyph { rows: [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010] },
    // '='
    Glyph { rows: [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000] },
    // '>'
    Glyph { rows: [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000] },
    // '?'
    Glyph { rows: [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100] },
    // '@'
    Glyph { rows: [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110] },
    // 'A'
    Glyph { rows: [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001] },
    // 'B'
    Glyph { rows: [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110] },
    // 'C'
    Glyph { rows: [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110] },
    // 'D'
    Glyph { rows: [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100] },
    // 'E'
    Glyph { rows: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111] },
    // 'F'
    Glyph { rows: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000] },
    // 'G'
    Glyph { rows: [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110] },
    // 'H'
    Glyph { rows: [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001] },
    // 'I'
    Glyph { rows: [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110] },
    // 'J'
    Glyph { rows: [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100] },
    // 'K'
    Glyph { rows: [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001] },
    // 'L'
    Glyph { rows: [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111] },
    // 'M'
    Glyph { rows: [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001] },
    // 'N'
    Glyph { rows: [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001] },
    // 'O'
    Glyph { rows: [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110] },
    // 'P'
    Glyph { rows: [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000] },
    // 'Q'
    Glyph { rows: [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101] },
    // 'R'
    Glyph { rows: [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001] },
    // 'S'
    Glyph { rows: [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110] },
    // 'T'
    Glyph { rows: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100] },
    // 'U'
    Glyph { rows: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110] },
    // 'V'
    Glyph { rows: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100] },
    // 'W'
    Glyph { rows: [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001] },
    // 'X'
    Glyph { rows: [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001] },
    // 'Y'
    Glyph { rows: [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100] },
    // 'Z'
    Glyph { rows: [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111] },
    // '['
    Glyph { rows: [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110] },
    // '\'
    Glyph { rows: [0b10000, 0b10000, 0b01000, 0b00100, 0b00010, 0b00001, 0b00001] },
    // ']'
    Glyph { rows: [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110] },
    // '^'
    Glyph { rows: [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000] },
    // '_'
    Glyph { rows: [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111] },
    // '`'
    Glyph { rows: [0b01000, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000] },
    // 'a'
    Glyph { rows: [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111] },
    // 'b'
    Glyph { rows: [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110] },
    // 'c'
    Glyph { rows: [0b00000, 0b00000, 0b01110, 0b10001, 0b10000, 0b10001, 0b01110] },
    // 'd'
    Glyph { rows: [0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b10001, 0b01111] },
    // 'e'
    Glyph { rows: [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110] },
    // 'f'
    Glyph { rows: [0b00110, 0b01000, 0b01000, 0b11110, 0b01000, 0b01000, 0b01000] },
    // 'g'
    Glyph { rows: [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110] },
    // 'h'
    Glyph { rows: [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001] },
    // 'i'
    Glyph { rows: [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110] },
    // 'j'
    Glyph { rows: [0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100] },
    // 'k'
    Glyph { rows: [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010] },
    // 'l'
    Glyph { rows: [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110] },
    // 'm'
    Glyph { rows: [0b00000, 0b00000, 0b11011, 0b10101, 0b10101, 0b10101, 0b10101] },
    // 'n'
    Glyph { rows: [0b00000, 0b00000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001] },
    // 'o'
    Glyph { rows: [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110] },
    // 'p'
    Glyph { rows: [0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000] },
    // 'q'
    Glyph { rows: [0b00000, 0b00000, 0b01111, 0b10001, 0b01111, 0b00001, 0b00001] },
    // 'r'
    Glyph { rows: [0b00000, 0b00000, 0b10110, 0b11000, 0b10000, 0b10000, 0b10000] },
    // 's'
    Glyph { rows: [0b00000, 0b00000, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110] },
    // 't'
    Glyph { rows: [0b01000, 0b01000, 0b11110, 0b01000, 0b01000, 0b01001, 0b00110] },
    // 'u'
    Glyph { rows: [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10001, 0b01111] },
    // 'v'
    Glyph { rows: [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100] },
    // 'w'
    Glyph { rows: [0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010] },
    // 'x'
    Glyph { rows: [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001] },
    // 'y'
    Glyph { rows: [0b00000, 0b00000, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110] },
    // 'z'
    Glyph { rows: [0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111] },
    // '{'
    Glyph { rows: [0b00011, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00011] },
    // '|'
    Glyph { rows: [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100] },
    // '}'
    Glyph { rows: [0b11000, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b11000] },
    // '~'
    Glyph { rows: [0b00000, 0b00000, 0b01001, 0b10101, 0b10010, 0b00000, 0b00000] },
];

/// Glyph for a printable ASCII character; `None` for anything else.
pub fn glyph(c: char) -> Option<&'static Glyph> {
    if !(FIRST_CHAR..=LAST_CHAR).contains(&c) {
        return None;
    }
    GLYPH_TABLE.get(c as usize - FIRST_CHAR as usize)
}

/// Glyph used for characters outside the table ([`FALLBACK_CHAR`]).
pub fn fallback() -> &'static Glyph {
    // `FALLBACK_CHAR` is inside the table, so this index always exists.
    &GLYPH_TABLE[FALLBACK_CHAR as usize - FIRST_CHAR as usize]
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
