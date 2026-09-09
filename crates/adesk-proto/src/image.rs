//! Wire form of images (§4). `adesk_core::ImageBuffer` is not wire-facing; this
//! module owns the base64-carrying payload and its conversions.

use adesk_core::ImageBuffer;
use serde::{Deserialize, Serialize};

use crate::types::ImageFormat;
use crate::Result;

/// An image on the wire (§4).
///
/// `data` is base64 (standard alphabet, with padding) over either PNG bytes
/// (`format = "png"`) or raw tightly packed RGBA8 pixels (`format = "rgba8"`,
/// where `stride` is `width * 4`).
/// `scale` is the downscale factor already applied (`1.0` = full resolution).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImagePayload {
    /// Pixel width after scaling.
    pub width: u32,
    /// Pixel height after scaling.
    pub height: u32,
    /// Encoding of `data`.
    pub format: ImageFormat,
    /// Bytes per row for `rgba8` payloads (`null`/absent for `png`).
    #[serde(default)]
    pub stride: Option<u32>,
    /// Base64-encoded image bytes.
    pub data: String,
    /// Downscale factor applied before encoding (`1.0` = unscaled).
    #[serde(default = "crate::defaults::scale")]
    pub scale: f64,
}

impl ImagePayload {
    /// Builds an `rgba8` payload, base64-encoding `data` (`stride = width * 4`).
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Malformed`](crate::ProtoError::Malformed) when
    /// `data.len() != width * height * 4`.
    pub fn from_rgba8(width: u32, height: u32, data: &[u8], scale: f64) -> Result<ImagePayload> {
        todo!()
    }

    /// Builds a `png` payload from PNG-encoded bytes.
    pub fn from_png(width: u32, height: u32, png_bytes: &[u8], scale: f64) -> ImagePayload {
        todo!()
    }

    /// Decodes `data` from base64.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Base64`](crate::ProtoError::Base64) for invalid base64.
    pub fn decode_data(&self) -> Result<Vec<u8>> {
        todo!()
    }

    /// Reconstructs an [`ImageBuffer`] from an `rgba8` payload.
    ///
    /// # Errors
    ///
    /// Returns an error when `format` is not [`ImageFormat::Rgba8`], the stride
    /// is not tightly packed, or the decoded length does not match the size.
    pub fn to_rgba8_buffer(&self) -> Result<ImageBuffer> {
        todo!()
    }
}
