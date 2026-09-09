//! Window-relative pointer positions and their resolution to pixels.

use serde::{Deserialize, Serialize};

use crate::geometry::{clamp_i32, Point, Rect};

/// A window-relative pointer position.
///
/// Serializes as the AGP tagged union (`docs/protocol.md` §2):
/// `{"type":"pixels","x":100,"y":50}` or
/// `{"type":"normalized","x":0.72,"y":0.41}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Position {
    /// Absolute pixels inside the window.
    Pixels(Point),
    /// Fraction of the window width/height.
    Normalized {
        /// Fraction along the window width.
        x: f64,
        /// Fraction along the window height.
        y: f64,
    },
}

impl Position {
    /// Creates a pixel position.
    pub const fn pixels(x: i32, y: i32) -> Position {
        Position::Pixels(Point { x, y })
    }

    /// Creates a normalized position.
    pub const fn normalized(x: f64, y: f64) -> Position {
        Position::Normalized { x, y }
    }

    /// Resolves window-relative coordinates to a window-relative pixel point.
    ///
    /// - `Normalized` values are clamped to `0.0..=1.0` (`NaN` becomes `0.0`,
    ///   infinities saturate) and mapped linearly so `0.0` is the first and
    ///   `1.0` the last pixel of the window: `round(n * (dim - 1))`.
    /// - `Pixels` are clamped into the window (first to last pixel inclusive).
    /// - An empty window resolves to its origin, which is the only sane point
    ///   it contains.
    ///
    /// The window rect is always the authority for bounds; callers must not
    /// re-derive coordinates with hard-coded constants.
    pub fn resolve(&self, window: Rect) -> Point {
        if window.is_empty() {
            return Point {
                x: window.x,
                y: window.y,
            };
        }
        let last_x = window.x as i64 + window.w as i64 - 1;
        let last_y = window.y as i64 + window.h as i64 - 1;
        match *self {
            Position::Pixels(p) => Point {
                x: clamp_i32((p.x as i64).clamp(window.x as i64, last_x)),
                y: clamp_i32((p.y as i64).clamp(window.y as i64, last_y)),
            },
            Position::Normalized { x, y } => Point {
                x: clamp_i32(
                    window.x as i64 + (clamp01(x) * (window.w as f64 - 1.0)).round() as i64,
                ),
                y: clamp_i32(
                    window.y as i64 + (clamp01(y) * (window.h as f64 - 1.0)).round() as i64,
                ),
            },
        }
    }
}

/// Clamps a normalized coordinate to `0.0..=1.0`; `NaN` becomes `0.0`.
fn clamp01(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect {
            x: 100,
            y: 50,
            w: 100,
            h: 50,
        }
    }

    #[test]
    fn pixels_inside_are_unchanged() {
        assert_eq!(
            Position::pixels(150, 75).resolve(window()),
            Point { x: 150, y: 75 }
        );
    }

    #[test]
    fn pixels_are_clamped_to_window() {
        let w = window();
        assert_eq!(Position::pixels(0, 0).resolve(w), Point { x: 100, y: 50 });
        assert_eq!(
            Position::pixels(-500, -500).resolve(w),
            Point { x: 100, y: 50 }
        );
        assert_eq!(
            Position::pixels(1000, 1000).resolve(w),
            Point { x: 199, y: 99 }
        );
        assert_eq!(
            Position::pixels(i32::MAX, i32::MAX).resolve(w),
            Point { x: 199, y: 99 }
        );
    }

    #[test]
    fn pixels_on_empty_window_resolve_to_origin() {
        let empty = Rect {
            x: 7,
            y: 9,
            w: 0,
            h: 0,
        };
        assert_eq!(
            Position::pixels(500, 500).resolve(empty),
            Point { x: 7, y: 9 }
        );
        assert_eq!(
            Position::normalized(0.5, 0.5).resolve(empty),
            Point { x: 7, y: 9 }
        );
    }

    #[test]
    fn normalized_corners_map_to_first_and_last_pixel() {
        let w = window();
        assert_eq!(
            Position::normalized(0.0, 0.0).resolve(w),
            Point { x: 100, y: 50 }
        );
        assert_eq!(
            Position::normalized(1.0, 1.0).resolve(w),
            Point { x: 199, y: 99 }
        );
    }

    #[test]
    fn normalized_center_rounds_to_middle() {
        assert_eq!(
            Position::normalized(0.5, 0.5).resolve(window()),
            Point { x: 150, y: 75 }
        );
        assert_eq!(
            Position::normalized(0.5, 0.5).resolve(Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10
            }),
            Point { x: 5, y: 5 }
        );
    }

    #[test]
    fn normalized_out_of_range_is_clamped() {
        let w = window();
        assert_eq!(
            Position::normalized(-0.5, 1.5).resolve(w),
            Point { x: 100, y: 99 }
        );
        assert_eq!(
            Position::normalized(f64::NEG_INFINITY, f64::INFINITY).resolve(w),
            Point { x: 100, y: 99 }
        );
    }

    #[test]
    fn normalized_nan_resolves_to_origin() {
        assert_eq!(
            Position::normalized(f64::NAN, f64::NAN).resolve(window()),
            Point { x: 100, y: 50 }
        );
    }

    #[test]
    fn normalized_single_pixel_window_always_resolves_to_that_pixel() {
        let single = Rect {
            x: 5,
            y: 5,
            w: 1,
            h: 1,
        };
        assert_eq!(
            Position::normalized(0.7, 0.7).resolve(single),
            Point { x: 5, y: 5 }
        );
        assert_eq!(
            Position::normalized(0.0, 1.0).resolve(single),
            Point { x: 5, y: 5 }
        );
    }

    #[test]
    fn normalized_examples_from_protocol() {
        let output = Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        };
        assert_eq!(
            Position::normalized(0.72, 0.41).resolve(output),
            Point { x: 921, y: 328 }
        );
    }

    #[test]
    fn constructors_match_variants() {
        assert_eq!(
            Position::pixels(1, 2),
            Position::Pixels(Point { x: 1, y: 2 })
        );
        assert_eq!(
            Position::normalized(0.25, 0.75),
            Position::Normalized { x: 0.25, y: 0.75 }
        );
    }
}
