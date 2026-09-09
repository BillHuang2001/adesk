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
        match *self {
            FillPattern::Solid(colour) => colour,
            FillPattern::Checker { size: cell, a, b } => {
                let cell = cell.max(1) as u64;
                if (x as u64 / cell + y as u64 / cell) % 2 == 0 {
                    a
                } else {
                    b
                }
            }
            FillPattern::GradientH { from, to } => gradient_at(from, to, x, size.w),
            FillPattern::GradientV { from, to } => gradient_at(from, to, y, size.h),
        }
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
        match *self {
            FillPattern::Solid(colour) => format!("solid:{}", hex_colour(colour)),
            FillPattern::Checker { size, a, b } => {
                format!("checker:{size}:{}:{}", hex_colour(a), hex_colour(b))
            }
            FillPattern::GradientH { from, to } => {
                format!("gradient-h:{}:{}", hex_colour(from), hex_colour(to))
            }
            FillPattern::GradientV { from, to } => {
                format!("gradient-v:{}:{}", hex_colour(from), hex_colour(to))
            }
        }
    }

    /// Decodes a pattern produced by [`FillPattern::to_cli_arg`].
    ///
    /// Malformed input returns [`TestkitError::Unsupported`] with the offending argument in
    /// the message, so a CLI that prints the error names the bad value.
    pub fn from_cli_arg(arg: &str) -> Result<FillPattern> {
        let mut parts = arg.split(':');
        let kind = parts.next().unwrap_or_default();
        let fields: Vec<&str> = parts.collect();
        let colour = |field: &str| parse_colour(field).map_err(|detail| cli_error(arg, detail));
        match kind {
            "solid" => match fields.as_slice() {
                [colour_arg] => Ok(FillPattern::Solid(colour(colour_arg)?)),
                _ => Err(cli_error(
                    arg,
                    format!("solid needs exactly one colour, got {}", fields.len()),
                )),
            },
            "checker" => match fields.as_slice() {
                [size, a, b] => {
                    let size = size.parse::<u32>().map_err(|error| {
                        cli_error(arg, format!("invalid checker size `{size}`: {error}"))
                    })?;
                    Ok(FillPattern::Checker {
                        size,
                        a: colour(a)?,
                        b: colour(b)?,
                    })
                }
                _ => Err(cli_error(
                    arg,
                    format!("checker needs SIZE and two colours, got {}", fields.len()),
                )),
            },
            "gradient-h" => match fields.as_slice() {
                [from, to] => Ok(FillPattern::GradientH {
                    from: colour(from)?,
                    to: colour(to)?,
                }),
                _ => Err(cli_error(
                    arg,
                    format!("gradient-h needs two colours, got {}", fields.len()),
                )),
            },
            "gradient-v" => match fields.as_slice() {
                [from, to] => Ok(FillPattern::GradientV {
                    from: colour(from)?,
                    to: colour(to)?,
                }),
                _ => Err(cli_error(
                    arg,
                    format!("gradient-v needs two colours, got {}", fields.len()),
                )),
            },
            other => Err(cli_error(arg, format!("unknown pattern kind `{other}`"))),
        }
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

/// Interpolates `from → to` at `pos` along an axis of `len` pixels.
///
/// Per channel: `from[c] + round_half_up((to[c] - from[c]) * pos / (len - 1))` for `len > 1`
/// and `from` otherwise, with `pos` clamped to `len - 1` and half-up rounding (ties toward
/// `+∞`) on the integer quotient. `i64` intermediates keep the function total for any `u32`
/// coordinate; the result is exactly the documented `i32` arithmetic for realistic sizes.
fn gradient_at(from: [u8; 4], to: [u8; 4], pos: u32, len: u32) -> [u8; 4] {
    if len <= 1 {
        return from;
    }
    let denominator = (len - 1) as i64;
    let pos = (pos as i64).min(denominator);
    let mut out = from;
    for (channel, value) in out.iter_mut().enumerate() {
        let delta = to[channel] as i64 - from[channel] as i64;
        // `round_half_up(num / denominator)` for `denominator > 0`, ties toward +∞.
        let rounded = (2 * delta * pos + denominator).div_euclid(2 * denominator);
        *value = (from[channel] as i64 + rounded).clamp(0, 255) as u8;
    }
    out
}

/// `RRGGBB` when the colour is opaque, `RRGGBBAA` otherwise (lowercase hex).
fn hex_colour(colour: [u8; 4]) -> String {
    if colour[3] == 255 {
        format!("{:02x}{:02x}{:02x}", colour[0], colour[1], colour[2])
    } else {
        format!(
            "{:02x}{:02x}{:02x}{:02x}",
            colour[0], colour[1], colour[2], colour[3]
        )
    }
}

/// Parses `RRGGBB` or `RRGGBBAA` (case-insensitive) into opaque-by-default RGBA.
fn parse_colour(field: &str) -> std::result::Result<[u8; 4], String> {
    if field.len() != 6 && field.len() != 8 {
        return Err(format!(
            "colour `{field}` must have 6 or 8 hex digits, got {}",
            field.len()
        ));
    }
    if !field.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("colour `{field}` is not hexadecimal"));
    }
    let byte = |index: usize| {
        u8::from_str_radix(&field[index * 2..index * 2 + 2], 16)
            .expect("hex digits were validated above")
    };
    let alpha = if field.len() == 8 { byte(3) } else { 255 };
    Ok([byte(0), byte(1), byte(2), alpha])
}

/// Builds the [`TestkitError::Unsupported`] for a malformed CLI pattern.
fn cli_error(arg: &str, detail: impl std::fmt::Display) -> TestkitError {
    TestkitError::Unsupported(format!("invalid fill pattern `{arg}`: {detail}"))
}
