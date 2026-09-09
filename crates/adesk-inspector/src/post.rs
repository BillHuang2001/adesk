//! Post-processing adapter over `adesk-render`.
//!
//! This module is the **only** place this crate touches `adesk-render`. It
//! delegates `inspect_capture`'s `region` / `max_dimension` to the same
//! crop/downscale implementation agent-facing `capture_*` uses, so both paths
//! scale identically.
//!
//! Expected `adesk-render` surface (cross-crate contract, see `CONTEXT.md`):
//!
//! ```ignore
//! adesk_render::crop(&ImageBuffer, Rect) -> adesk_render::Result<ImageBuffer>
//! adesk_render::downscale(&ImageBuffer, u32) -> adesk_render::Result<ImageBuffer>
//! ```

use adesk_core::ImageBuffer;

use crate::error::{Error, Result};
use crate::source::InspectionRequest;

/// Applies `request` to a composed frame: `region` crop first, then
/// `max_dimension` downscale (the order `docs/architecture.md` §5 uses).
///
/// The region is clipped to the frame bounds first; a region that does not
/// intersect the frame, or a `max_dimension` of `0`, is an
/// [`Error::InvalidRequest`].
pub(crate) fn apply(frame: ImageBuffer, request: &InspectionRequest) -> Result<ImageBuffer> {
    let mut frame = frame;

    if let Some(region) = request.region {
        let clipped = frame.rect().intersect(&region).ok_or_else(|| {
            Error::InvalidRequest(format!(
                "region {region:?} does not intersect the {}x{} inspection frame",
                frame.width, frame.height
            ))
        })?;
        frame = adesk_render::crop(&frame, clipped)?;
    }

    if let Some(max_dimension) = request.max_dimension {
        if max_dimension == 0 {
            return Err(Error::InvalidRequest(
                "max_dimension must be greater than zero".to_string(),
            ));
        }
        frame = adesk_render::downscale(&frame, max_dimension)?;
    }

    tracing::debug!(
        width = frame.width,
        height = frame.height,
        region = ?request.region,
        max_dimension = ?request.max_dimension,
        "post-processed inspection frame"
    );

    Ok(frame)
}
