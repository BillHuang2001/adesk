//! Encoding `adesk_core::ImageBuffer` into AGP `ImagePayload`s (`docs/protocol.md` §4).
//!
//! The runtime renders into RGBA8 buffers; this module is the only place that
//! knows the wire encodings (`png` default, `rgba8` opt-in). Agent-facing
//! capture paths never include overlays.

use adesk_core::ImageBuffer;
use adesk_proto::{ImageFormat, ImagePayload};

use crate::error::Result;

/// Encodes `image` in the requested wire format.
///
/// `scale` is the downscale factor already applied by the compositor
/// (`1.0` = full resolution) and is reported as `ImagePayload::scale`.
///
/// # Errors
///
/// Returns [`crate::ServerError::Render`] if PNG encoding fails and
/// [`crate::ServerError::Proto`] if the buffer length does not match its size.
pub fn encode(image: &ImageBuffer, format: ImageFormat, scale: f64) -> Result<ImagePayload> {
    todo!()
}

/// Encodes an RGBA8 buffer as PNG bytes (the `image` crate is the only encoder).
///
/// # Errors
///
/// Returns [`crate::ServerError::Render`] if the encoder rejects the buffer.
pub fn encode_png(image: &ImageBuffer) -> Result<Vec<u8>> {
    todo!()
}
