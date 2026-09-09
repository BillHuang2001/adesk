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
        let base = self.crop.map_or_else(|| self.target_size(), |crop| crop.size());
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
