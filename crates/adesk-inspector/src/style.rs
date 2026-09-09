//! Palette and metrics for overlay drawing.
//!
//! Every painter reads its colours from [`OverlayStyle`] so tests can pin exact
//! pixels and the server can tune the projection without touching painters.

use crate::color::Color;
use crate::font::clamp_scale;

/// Smallest padding a label keeps between its text and its plate border.
pub const MIN_PADDING: i32 = 1;
/// Largest padding accepted; larger values are clamped down.
pub const MAX_PADDING: i32 = 16;

/// Drawing style shared by every overlay painter.
///
/// [`OverlayStyle::default`] uses classic debug colours (documented in
/// `CONTEXT.md`); the values are stable and pixel-assertable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayStyle {
    /// Bitmap font scale, clamped to `1..=8` by [`OverlayStyle::scale`].
    pub font_scale: u8,
    /// Padding between label text and plate border, clamped to `1..=16` by
    /// [`OverlayStyle::pad`].
    pub padding: i32,
    /// Label text colour.
    pub text: Color,
    /// Label plate background, drawn behind text (may be translucent).
    pub plate: Color,
    /// 1 px outline colour for damage rects, surface bounds and plates.
    pub outline: Color,
    /// Fill colour for damage rects (translucent by default).
    pub fill: Color,
    /// Focus outline and focus label accent.
    pub focus: Color,
    /// Cursor crosshair colour.
    pub cursor: Color,
    /// Action marker colour.
    pub action: Color,
    /// Commit-timing HUD colour.
    pub timing: Color,
}

impl Default for OverlayStyle {
    fn default() -> OverlayStyle {
        OverlayStyle {
            font_scale: 1,
            padding: 2,
            text: Color::WHITE,
            plate: Color::rgba(0, 0, 0, 160),
            outline: Color::WHITE,
            fill: Color::rgba(255, 0, 0, 48),
            focus: Color::rgb(0, 255, 0),
            cursor: Color::rgb(255, 255, 0),
            action: Color::rgb(0, 255, 255),
            timing: Color::rgb(255, 0, 255),
        }
    }
}

impl OverlayStyle {
    /// Effective font scale: [`OverlayStyle::font_scale`] clamped to `1..=8`.
    pub const fn scale(&self) -> u8 {
        clamp_scale(self.font_scale)
    }

    /// Effective padding: [`OverlayStyle::padding`] clamped to `1..=16`.
    pub const fn pad(&self) -> i32 {
        if self.padding < MIN_PADDING {
            MIN_PADDING
        } else if self.padding > MAX_PADDING {
            MAX_PADDING
        } else {
            self.padding
        }
    }
}
