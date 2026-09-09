//! Deterministic fill patterns: the single ground truth shared by the SHM buffer writer
//! ([`crate::wayland`]) and image assertions ([`crate::ImageAssert`]).
//!
//! A pattern is *pure data*: [`FillPattern::at`] computes the expected RGBA pixel for any
//! `(x, y)` in an image of a given size. The Wayland test client fills SHM buffers by
//! evaluating the same function, so `ImageAssert::matches_pattern` can never disagree with
//! what the client drew — there is exactly one implementation of every pattern.
//!
//! All colours are opaque RGBA with alpha `255`; test buffers are committed as `Argb8888`
//! (see `wayland::shm` for the byte-order rule).

use adesk_core::Size;

use crate::error::{Result, TestkitError};

/// The default fill: an opaque slate blue that is unlikely to collide with app content.
pub const DEFAULT_FILL: [u8; 4] = [32, 64, 96, 255];

/// A deterministic pixel pattern.
///
/// Patterns are `Copy` and comparable so tests can assert which pattern a window was
/// filled with without keeping a reference to the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillPattern {
    /// Every pixel has the same colour.
    Solid([u8; 4]),
    /// Checkerboard of `size`-pixel cells; `(0, 0)` is colour `a`.
    Checker {
        /// Cell edge length in pixels (`0` is treated as `1`).
        size: u32,
        /// Colour of cells where `(x/size + y/size)` is even.
        a: [u8; 4],
        /// Colour of cells where `(x/size + y/size)` is odd.
        b: [u8; 4],
    },
    /// Horizontal linear gradient: `from` at `x = 0`, `to` at `x = width - 1`.
    GradientH {
        /// Left edge colour.
        from: [u8; 4],
        /// Right edge colour.
        to: [u8; 4],
    },
    /// Vertical linear gradient: `from` at `y = 0`, `to` at `y = height - 1`.
    GradientV {
        /// Top edge colour.
        from: [u8; 4],
        /// Bottom edge colour.
        to: [u8; 4],
    },
}

impl Default for FillPattern {
    fn default() -> Self {
        FillPattern::Solid(DEFAULT_FILL)
    }
}

impl FillPattern {
    /// A solid opaque colour from RGB (alpha `255`).
    pub const fn solid_rgb(r: u8, g: u8, b: u8) -> FillPattern {
        FillPattern::Solid([r, g, b, 255])
    }

    /// A checkerboard with `size`-pixel cells.
    pub const fn checker(size: u32, a: [u8; 4], b: [u8; 4]) -> FillPattern {
        FillPattern::Checker { size, a, b }
    }

    /// A horizontal gradient.
    pub const fn gradient_h(from: [u8; 4], to: [u8; 4]) -> FillPattern {
        FillPattern::GradientH { from, to }
    }

    /// A vertical gradient.
    pub const fn gradient_v(from: [u8; 4], to: [u8; 4]) -> FillPattern {
        FillPattern::GradientV { from, to }
    }

    /// The expected RGBA pixel at `(x, y)` for an image of `size`.
    ///
    /// Exact semantics (Phase 2 implements exactly this; do not "improve" it):
    ///
    /// - `Solid(c)` → `c` everywhere, even outside `size`.
    /// - `Checker { size: s, a, b }` → `a` when `((x / max(s,1)) + (y / max(s,1))) % 2 == 0`,
    ///   else `b`.
    /// - `GradientH { from, to }` → per channel
    ///   `from[c] + round((to[c] - from[c]) * x / (w - 1))` for `w > 1`, `from` for `w <= 1`,
    ///   where `round` is half-up on the non-negative delta product and `x` is clamped to
    ///   `w - 1`. All arithmetic in `i32`.
    /// - `GradientV { from, to }` → same with `y` and `h`.
    ///
    /// Coordinates outside the image are still evaluated (callers clamp); the function is
    /// total so tests can ask for any pixel.
    pub fn at(&self, x: u32, y: u32, size: Size) -> [u8; 4] {
        let _ = (x, y, size);
        todo!("stub: implementation phase — exact semantics documented above")
    }

    /// Encodes the pattern as one CLI argument, used to pass it to the helper binary.
    ///
    /// Grammar (hex is case-insensitive, alpha defaults to `ff` when omitted):
    ///
    /// ```text
    /// solid:RRGGBB[AA]
    /// checker:SIZE:RRGGBB[AA]:RRGGBB[AA]
    /// gradient-h:RRGGBB[AA]:RRGGBB[AA]
    /// gradient-v:RRGGBB[AA]:RRGGBB[AA]
    /// ```
    pub fn to_cli_arg(&self) -> String {
        todo!("stub: implementation phase — grammar documented above")
    }

    /// Decodes a pattern produced by [`FillPattern::to_cli_arg`].
    pub fn from_cli_arg(arg: &str) -> Result<FillPattern> {
        let _ = arg;
        todo!("stub: implementation phase — grammar documented on to_cli_arg")
    }

    /// Validates that every colour in the pattern is opaque, returning
    /// `Err(Unsupported)` otherwise.
    ///
    /// Test buffers are committed as `Argb8888`; translucent patterns would make exact
    /// pixel assertions depend on compositing, so the harness rejects them loudly.
    pub fn require_opaque(&self) -> Result<()> {
        let opaque = |c: &[u8; 4]| c[3] == 255;
        let ok = match self {
            FillPattern::Solid(c) => opaque(c),
            FillPattern::Checker { a, b, .. } => opaque(a) && opaque(b),
            FillPattern::GradientH { from, to } | FillPattern::GradientV { from, to } => {
                opaque(from) && opaque(to)
            }
        };
        if ok {
            Ok(())
        } else {
            Err(TestkitError::Unsupported(format!(
                "fill pattern {self:?} is translucent; test buffers are Argb8888 and require opaque colours"
            )))
        }
    }
}
