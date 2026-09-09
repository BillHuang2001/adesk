//! Policy configuration: the virtual output geometry the tiling policy targets.

use adesk_core::{Rect, Size};

/// Configuration of the single-visible-toplevel tiling policy.
///
/// v1 has exactly one knob: the virtual output size (`--output WxH`, AGP
/// default `1280x800`). Every mapped window is tiled to
/// [`PolicyConfig::tiled_rect`]. New fields (gaps, margins, layout kind) are
/// additive and must not change the behavior of existing ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PolicyConfig {
    /// Virtual output size in pixels.
    pub output_size: Size,
}

impl PolicyConfig {
    /// Creates a configuration for the given output size.
    pub const fn new(output_size: Size) -> PolicyConfig {
        PolicyConfig { output_size }
    }

    /// The rect every mapped window is tiled to: the whole output at `(0, 0)`.
    pub const fn tiled_rect(&self) -> Rect {
        Rect::from_size(self.output_size)
    }
}

impl Default for PolicyConfig {
    /// The protocol default output size, `1280x800`.
    fn default() -> PolicyConfig {
        PolicyConfig::new(Size::new(1280, 800))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_protocol_output_size() {
        assert_eq!(PolicyConfig::default().output_size, Size::new(1280, 800));
    }

    #[test]
    fn tiled_rect_is_full_output_at_origin() {
        assert_eq!(
            PolicyConfig::new(Size::new(640, 480)).tiled_rect(),
            Rect::new(0, 0, 640, 480)
        );
        assert_eq!(
            PolicyConfig::default().tiled_rect(),
            Rect::new(0, 0, 1280, 800)
        );
    }

    #[test]
    fn empty_output_tiles_to_an_empty_rect() {
        assert!(PolicyConfig::new(Size::ZERO).tiled_rect().is_empty());
    }
}
