//! Image payload decoding.
//!
//! `capture_*`, `observe` and `inspect_capture` return the wire type
//! [`ImagePayload`] (base64 data + metadata, protocol §4).
//! Agent code wants pixels, so this module decodes a payload into an
//! [`ImageBuffer`] — the shared, non-wire pixel type.
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

use crate::{ClientError, ImagePayload, Result};

/// The wire `format` of an [`ImagePayload`], aliased because this module
/// defines the client-side request enum [`ImageFormat`] as well.
///
/// `adesk-proto`'s frame/codec types are confined to `wire.rs`, but the payload
/// itself (and therefore its `format` field) is re-exported by `lib.rs`, so this
/// is the only place outside `wire.rs` that names a proto type — and it is the
/// payload type's own vocabulary.
use adesk_proto::ImageFormat as WireImageFormat;

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
/// Errors are [`ClientError::Image`] for bad base64,
/// an unsupported/unknown format, a PNG decoder failure, or pixel data whose
/// length does not match `width * height * 4`.
pub fn decode_image(payload: &ImagePayload) -> Result<ImageBuffer> {
    let bytes = payload.decode_data().map_err(|error| ClientError::Image {
        message: format!("invalid base64 image data: {error}"),
    })?;
    // `adesk_proto::ImageFormat` is a closed two-variant enum without a
    // catch-all, so this match is exhaustive: an unknown wire format is rejected
    // by serde while decoding the payload and can never reach this function
    // (see tests/images.rs::decode_unknown_format_is_image_error).
    match payload.format {
        WireImageFormat::Png => decode_png(payload, &bytes),
        WireImageFormat::Rgba8 => decode_rgba8(payload, bytes),
    }
}

/// Decode PNG bytes and convert them to RGBA8.
///
/// The payload's declared `width`/`height` must match the PNG's own dimensions;
/// a mismatch is a malformed payload, not something to guess about.
fn decode_png(payload: &ImagePayload, bytes: &[u8]) -> Result<ImageBuffer> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| ClientError::Image {
            message: format!("PNG decode failed: {error}"),
        })?
        .to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    if width != payload.width || height != payload.height {
        return Err(ClientError::Image {
            message: format!(
                "PNG is {width}x{height} but the payload declares {}x{}",
                payload.width, payload.height
            ),
        });
    }
    ImageBuffer::from_rgba(width, height, decoded.into_raw()).map_err(|error| ClientError::Image {
        message: format!("PNG pixels are not a valid RGBA8 buffer: {}", error.message),
    })
}

/// Decode raw RGBA8 bytes, honouring `stride` and re-packing rows tightly.
///
/// `stride` defaults to the tight row length `width * 4` when absent; a declared
/// stride smaller than that, or a byte count that is not exactly
/// `stride * height`, is rejected. Padding bytes are dropped.
fn decode_rgba8(payload: &ImagePayload, data: Vec<u8>) -> Result<ImageBuffer> {
    // `width * 4` cannot overflow `u64`; it must still fit the `u32` stride of
    // the returned buffer.
    let row_bytes = u64::from(payload.width) * 4;
    if row_bytes > u64::from(u32::MAX) {
        return Err(ClientError::Image {
            message: format!("RGBA width {} exceeds the maximum stride", payload.width),
        });
    }
    let stride = payload.stride.map(u64::from).unwrap_or(row_bytes);
    if stride < row_bytes {
        return Err(ClientError::Image {
            message: format!(
                "RGBA stride {stride} is smaller than the {row_bytes}-byte row width of {} pixels",
                payload.width
            ),
        });
    }
    let expected = stride
        .checked_mul(u64::from(payload.height))
        .ok_or_else(|| ClientError::Image {
            message: format!(
                "RGBA {}x{} at stride {stride} overflows the byte count",
                payload.width, payload.height
            ),
        })?;
    if expected != data.len() as u64 {
        return Err(ClientError::Image {
            message: format!(
                "RGBA data length {} does not match {}x{} at stride {stride} (expected {expected} bytes)",
                data.len(),
                payload.width,
                payload.height
            ),
        });
    }
    let stride = usize::try_from(stride).map_err(|_| ClientError::Image {
        message: format!("RGBA stride {stride} does not fit this platform"),
    })?;
    let row_bytes = usize::try_from(row_bytes).map_err(|_| ClientError::Image {
        message: format!("RGBA row width {row_bytes} does not fit this platform"),
    })?;
    let pixels = if stride == row_bytes {
        data
    } else {
        // Drop the per-row padding: `expected == data.len()` and
        // `stride >= row_bytes` make `chunks_exact` cover every row.
        let mut tight = Vec::with_capacity(row_bytes * payload.height as usize);
        for row in data.chunks_exact(stride) {
            tight.extend_from_slice(&row[..row_bytes]);
        }
        tight
    };
    ImageBuffer::from_rgba(payload.width, payload.height, pixels).map_err(|error| {
        ClientError::Image {
            message: format!("RGBA pixels are not a valid buffer: {}", error.message),
        }
    })
}
