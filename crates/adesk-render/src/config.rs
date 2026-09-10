//! Render configuration: what part of the scene to render and how to post-process it.

use adesk_core::{Rect, Size};
use smithay::backend::allocator::Fourcc;

use crate::error::{RenderError, Result};
use crate::image::fit_dimensions;

/// Pixel format of offscreen targets created by [`crate::create_target`].
///
/// `Abgr8888` is the DRM fourcc whose memory byte order is `R, G, B, A`,
/// i.e. exactly [`adesk_core::PixelFormat::Rgba8`]. Both renderers support it:
/// GLES maps it to `GL_RGBA`/`GL_UNSIGNED_BYTE`, pixman to `A8B8G8R8` on
/// little-endian.
pub const TARGET_FORMAT: Fourcc = Fourcc::Abgr8888;

/// Pixel format requested for readback.
///
/// Same value as [`TARGET_FORMAT`]: readback yields tightly packed `Rgba8`
/// bytes, so no channel swizzle is ever needed.
pub const READBACK_FORMAT: Fourcc = Fourcc::Abgr8888;

/// Default clear color: opaque black (`R, G, B, A`).
pub const DEFAULT_CLEAR_COLOR: [u8; 4] = [0, 0, 0, 0xff];

/// Describes one offscreen render pass.
///
/// The pipeline renders the scene region [`RenderConfig::source`] into a target
/// of exactly that size (scene coordinate `source.loc` maps to target pixel
/// `(0, 0)`), then optionally crops and downscales the read-back image.
///
/// ```text
/// scene coordinates                      target pixels
/// ┌────────────────────┐                 ┌──────────┐
/// │        source      │  ── render ──▶  │  target  │
/// │   ┌────────┐       │                 │  ┌────┐  │
/// │   │  crop  │       │  ── crop ────▶  │  │crop│  │ ── downscale ──▶ output
/// │   └────────┘       │                 │  └────┘  │
/// └────────────────────┘                 └──────────┘
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    /// Scene-coordinate rectangle rendered into the target; its size is the
    /// target size. Must be non-empty.
    pub source: Rect,
    /// Optional crop in the same coordinate space as [`RenderConfig::source`];
    /// must be fully contained in `source`. `None` keeps the whole target.
    pub crop: Option<Rect>,
    /// Optional upper bound for the longest output edge (box filter).
    /// `None` or `0` disables downscaling.
    pub max_dimension: Option<u32>,
    /// Background color the target is cleared to before drawing (`R, G, B, A`).
    pub clear_color: [u8; 4],
}

impl RenderConfig {
    /// Config that renders `source` at full size with an opaque black background.
    pub fn new(source: Rect) -> Self {
        Self {
            source,
            crop: None,
            max_dimension: None,
            clear_color: DEFAULT_CLEAR_COLOR,
        }
    }

    /// Crops the result to `crop` (scene coordinates, must be inside `source`).
    pub fn with_crop(mut self, crop: Rect) -> Self {
        self.crop = Some(crop);
        self
    }

    /// Bounds the longest output edge to `max_dimension` (box-filter downscale).
    pub fn with_max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }

    /// Sets the target clear color (`R, G, B, A`).
    pub fn with_clear_color(mut self, color: [u8; 4]) -> Self {
        self.clear_color = color;
        self
    }

    /// Size of the offscreen target that this configuration renders into.
    pub fn target_size(&self) -> Size {
        self.source.size()
    }

    /// Size of the image the pipeline returns, after crop and downscale.
    pub fn output_size(&self) -> Size {
        let base = self
            .crop
            .map_or_else(|| self.target_size(), |crop| crop.size());
        match self.max_dimension {
            Some(max_dimension) => fit_dimensions(base, max_dimension),
            None => base,
        }
    }

    /// Validates the configuration, returning a structured error for callers.
    pub fn validate(&self) -> Result<()> {
        if self.source.is_empty() {
            return Err(RenderError::InvalidConfig {
                reason: "source rect is empty".to_string(),
            });
        }
        if let Some(crop) = self.crop {
            if crop.is_empty() {
                return Err(RenderError::InvalidConfig {
                    reason: "crop rect is empty".to_string(),
                });
            }
            if crop.intersect(&self.source) != Some(crop) {
                return Err(RenderError::InvalidConfig {
                    reason: format!("crop {crop:?} is not contained in source {:?}", self.source),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Rect {
        Rect::new(10, 20, 64, 48)
    }

    #[test]
    fn new_config_has_no_post_processing_and_default_color() {
        let config = RenderConfig::new(source());
        assert_eq!(config.source, source());
        assert_eq!(config.crop, None);
        assert_eq!(config.max_dimension, None);
        assert_eq!(config.clear_color, DEFAULT_CLEAR_COLOR);
        assert_eq!(DEFAULT_CLEAR_COLOR, [0, 0, 0, 0xff]);
    }

    #[test]
    fn builders_set_fields() {
        let config = RenderConfig::new(source())
            .with_crop(Rect::new(12, 22, 8, 8))
            .with_max_dimension(4)
            .with_clear_color([1, 2, 3, 4]);
        assert_eq!(config.crop, Some(Rect::new(12, 22, 8, 8)));
        assert_eq!(config.max_dimension, Some(4));
        assert_eq!(config.clear_color, [1, 2, 3, 4]);
    }

    #[test]
    fn validate_accepts_valid_config() {
        assert!(RenderConfig::new(source()).validate().is_ok());
        assert!(RenderConfig::new(source())
            .with_crop(source())
            .with_max_dimension(16)
            .validate()
            .is_ok());
    }

    #[test]
    fn validate_rejects_empty_source() {
        let err = RenderConfig::new(Rect::new(3, 4, 0, 5)).validate();
        assert!(matches!(err, Err(RenderError::InvalidConfig { .. })));
    }

    #[test]
    fn validate_rejects_empty_crop() {
        let err = RenderConfig::new(source())
            .with_crop(Rect::new(11, 21, 0, 4))
            .validate();
        assert!(matches!(err, Err(RenderError::InvalidConfig { .. })));
    }

    #[test]
    fn validate_rejects_crop_outside_source() {
        // Starts before the source origin.
        assert!(RenderConfig::new(source())
            .with_crop(Rect::new(9, 20, 4, 4))
            .validate()
            .is_err());
        // Extends past the source's right edge.
        assert!(RenderConfig::new(source())
            .with_crop(Rect::new(70, 20, 8, 4))
            .validate()
            .is_err());
        // Disjoint from the source.
        assert!(RenderConfig::new(source())
            .with_crop(Rect::new(200, 200, 4, 4))
            .validate()
            .is_err());
    }

    #[test]
    fn target_size_is_the_source_size() {
        let config = RenderConfig::new(source());
        assert_eq!(config.target_size(), Size::new(64, 48));
        // Crop and downscale only affect the output, never the target.
        let config = config
            .with_crop(Rect::new(10, 20, 8, 8))
            .with_max_dimension(2);
        assert_eq!(config.target_size(), Size::new(64, 48));
    }

    #[test]
    fn output_size_without_post_processing_is_the_target_size() {
        let config = RenderConfig::new(source());
        assert_eq!(config.output_size(), Size::new(64, 48));
    }

    #[test]
    fn output_size_with_crop_is_the_crop_size() {
        let config = RenderConfig::new(source()).with_crop(Rect::new(12, 22, 8, 6));
        assert_eq!(config.output_size(), Size::new(8, 6));
    }

    #[test]
    fn output_size_with_max_dimension_preserves_aspect_ratio() {
        // 64x48 (4:3) -> longest edge 16.
        let config = RenderConfig::new(source()).with_max_dimension(16);
        assert_eq!(config.output_size(), Size::new(16, 12));
        // Non-integer ratio: 64x48 -> longest edge 20.
        let config = RenderConfig::new(source()).with_max_dimension(20);
        assert_eq!(config.output_size(), Size::new(20, 15));
        // Crop first (8x6), then downscale to a longest edge of 3.
        let config = RenderConfig::new(source())
            .with_crop(Rect::new(12, 22, 8, 6))
            .with_max_dimension(3);
        assert_eq!(config.output_size(), Size::new(3, 2));
    }

    #[test]
    fn output_size_never_upscales() {
        let config = RenderConfig::new(source()).with_max_dimension(1000);
        assert_eq!(config.output_size(), Size::new(64, 48));
    }

    #[test]
    fn output_size_with_max_dimension_zero_disables_scaling() {
        let config = RenderConfig::new(source()).with_max_dimension(0);
        assert_eq!(config.output_size(), Size::new(64, 48));
        let config = config.with_crop(Rect::new(12, 22, 8, 6));
        assert_eq!(config.output_size(), Size::new(8, 6));
    }

    #[test]
    fn target_and_readback_formats_are_rgba8() {
        assert_eq!(TARGET_FORMAT, Fourcc::Abgr8888);
        assert_eq!(READBACK_FORMAT, Fourcc::Abgr8888);
        assert_eq!(TARGET_FORMAT, READBACK_FORMAT);
    }
}
