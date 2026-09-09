//! Errors produced by the offscreen render pipeline.
//!
//! Every failure path in this crate is a [`RenderError`]; the pipeline never
//! panics on request/event paths. [`RenderError::code`] maps a failure to the
//! AGP [`ErrorCode`] the server reports to the agent, and the `From` impl turns
//! it into the umbrella [`adesk_core::Error`] at crate boundaries.

use adesk_core::{ErrorCode, Rect, Size};

/// Errors produced by rendering, readback, image post-processing and buffer import.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The offscreen target could not be allocated by the renderer.
    #[error("failed to create offscreen render target {size:?}: {source}")]
    TargetCreation {
        /// Requested target size in pixels.
        size: Size,
        /// Renderer-reported cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The offscreen target could not be bound as a framebuffer.
    #[error("failed to bind render target {size:?}: {source}")]
    TargetBind {
        /// Target size in pixels.
        size: Size,
        /// Renderer-reported cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The renderer failed while drawing the scene (frame setup, element draw, finish).
    #[error("renderer failed while drawing the scene: {source}")]
    RenderFailed {
        /// Renderer-reported cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Reading pixels back from the rendered target failed.
    #[error("readback of region {region:?} failed: {source}")]
    Readback {
        /// Region requested for readback, in target coordinates.
        region: Rect,
        /// Renderer-reported cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The renderer cannot express the requested pixel format as `Rgba8`.
    #[error("unsupported pixel format {format}")]
    UnsupportedFormat {
        /// Human-readable pixel format description (usually a Fourcc).
        format: String,
    },
    /// Importing a wayland buffer (SHM / DMA-BUF / EGL) into the renderer failed.
    ///
    /// This is the structured error a software renderer reports for DMA-BUF
    /// windows it cannot sample; it maps to AGP `render_failed`.
    #[error("buffer import failed: {reason}")]
    ImportFailed {
        /// Why the import failed.
        reason: String,
    },
    /// The buffer has no importable texture representation (e.g. an unknown
    /// buffer type or a single-pixel buffer).
    #[error("buffer cannot be imported as a texture: {reason}")]
    UnsupportedBuffer {
        /// Why the buffer has no texture representation.
        reason: String,
    },
    /// The render configuration is invalid (empty source, crop outside source, ...).
    #[error("invalid render configuration: {reason}")]
    InvalidConfig {
        /// Why the configuration is invalid.
        reason: String,
    },
    /// Raw pixel data could not be turned into an [`adesk_core::ImageBuffer`].
    #[error("invalid image data: {reason}")]
    InvalidImage {
        /// Why the image data is invalid.
        reason: String,
    },
    /// Encoding the rendered image (PNG) failed.
    #[error("image encoding failed: {reason}")]
    Encode {
        /// Encoder-reported cause.
        reason: String,
    },
}

impl RenderError {
    /// AGP error code this failure is reported as.
    ///
    /// Renderer-side failures (target allocation, drawing, readback, import,
    /// format) map to [`ErrorCode::RenderFailed`]; caller mistakes map to
    /// [`ErrorCode::InvalidRequest`]; payload encoding maps to
    /// [`ErrorCode::CaptureFailed`].
    pub fn code(&self) -> ErrorCode {
        match self {
            RenderError::TargetCreation { .. }
            | RenderError::TargetBind { .. }
            | RenderError::RenderFailed { .. }
            | RenderError::Readback { .. }
            | RenderError::UnsupportedFormat { .. }
            | RenderError::ImportFailed { .. }
            | RenderError::UnsupportedBuffer { .. } => ErrorCode::RenderFailed,
            RenderError::InvalidConfig { .. } | RenderError::InvalidImage { .. } => {
                ErrorCode::InvalidRequest
            }
            RenderError::Encode { .. } => ErrorCode::CaptureFailed,
        }
    }
}

impl From<RenderError> for adesk_core::Error {
    fn from(err: RenderError) -> Self {
        adesk_core::Error::new(err.code(), err.to_string())
    }
}

/// Crate-local result type.
pub type Result<T> = std::result::Result<T, RenderError>;
