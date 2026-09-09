//! Image payload decoding.
//!
//! `capture_*`, `observe` and `inspect_capture` return the wire type
//! [`ImagePayload`](crate::ImagePayload) (base64 data + metadata, protocol §4).
//! Agent code wants pixels, so this module decodes a payload into an
//! [`ImageBuffer`](adesk_core::ImageBuffer) — the shared, non-wire pixel type.
//!
//! Supported formats (protocol §5.4 `format`):
//!
//! - `png` — decoded with the `image` crate, converted to RGBA8.
//! - `rgba8` — raw pixels; `stride` (bytes per row) is honoured and rows are
//!   re-packed to the tight layout [`ImageBuffer::from_rgba`] requires.
//!
//! `scale` in the payload is informational (the renderer reports what
//! `max_dimension` did); the decoded buffer always has the payload's actual
//! `width`/`height`.

use adesk_core::ImageBuffer;
use serde::{Deserialize, Serialize};

use crate::{ImagePayload, Result};

/// Image format requested by `capture_*` / `observe` (protocol §5.4).
///
/// Serialises to the AGP strings `"png"` and `"rgba8"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG-encoded payload; decoded via the `image` crate.
    #[default]
    Png,
    /// Raw RGBA8 pixels, base64-encoded on the wire.
    Rgba8,
}

/// Decode an [`ImagePayload`] into an [`ImageBuffer`].
///
/// Errors are [`ClientError::Image`](crate::ClientError::Image) for bad base64,
/// an unsupported/unknown format, a PNG decoder failure, or pixel data whose
/// length does not match `width * height * 4`.
pub fn decode_image(payload: &ImagePayload) -> Result<ImageBuffer> {
    let _ = payload;
    todo!("decode base64, then PNG or stride-aware RGBA8 → ImageBuffer")
}
