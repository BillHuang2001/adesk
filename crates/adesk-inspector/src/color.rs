//! Straight (non-premultiplied) 8-bit RGBA colour used by overlay painters.

/// Straight-alpha RGBA colour, matching the `ImageBuffer` pixel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel: `0` is invisible, `255` is opaque.
    pub a: u8,
}

impl Color {
    /// Opaque white.
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// Opaque colour from RGB components.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// Colour with an explicit alpha component.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// The colour as `[r, g, b, a]`, the `ImageBuffer` pixel layout.
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}
