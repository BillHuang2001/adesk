//! Wire form of images (§4). `adesk_core::ImageBuffer` is not wire-facing; this
//! module owns the base64-carrying payload and its conversions.

use adesk_core::{ImageBuffer, PixelFormat};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::types::ImageFormat;
use crate::{ProtoError, Result};

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
    /// Returns [`ProtoError::Malformed`] when
    /// `data.len() != width * height * 4` or when the implied stride/byte count
    /// does not fit in `u32`.
    pub fn from_rgba8(width: u32, height: u32, data: &[u8], scale: f64) -> Result<ImagePayload> {
        let stride = u64::from(width)
            .checked_mul(4)
            .filter(|s| *s <= u64::from(u32::MAX));
        let stride = stride.ok_or_else(|| {
            ProtoError::Malformed(format!(
                "image width {width} exceeds the maximum RGBA stride"
            ))
        })?;
        let expected = stride.checked_mul(u64::from(height)).ok_or_else(|| {
            ProtoError::Malformed(format!(
                "image dimensions {width}x{height} overflow the RGBA byte count"
            ))
        })?;
        if expected != data.len() as u64 {
            return Err(ProtoError::Malformed(format!(
                "RGBA data length {} does not match {width}x{height} (expected {expected} bytes)",
                data.len()
            )));
        }
        Ok(ImagePayload {
            width,
            height,
            format: ImageFormat::Rgba8,
            stride: Some(stride as u32),
            data: STANDARD.encode(data),
            scale,
        })
    }

    /// Builds a `png` payload from PNG-encoded bytes.
    pub fn from_png(width: u32, height: u32, png_bytes: &[u8], scale: f64) -> ImagePayload {
        ImagePayload {
            width,
            height,
            format: ImageFormat::Png,
            stride: None,
            data: STANDARD.encode(png_bytes),
            scale,
        }
    }

    /// Decodes `data` from base64.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Base64`] for invalid base64.
    pub fn decode_data(&self) -> Result<Vec<u8>> {
        Ok(STANDARD.decode(&self.data)?)
    }

    /// Reconstructs an [`ImageBuffer`] from an `rgba8` payload.
    ///
    /// # Errors
    ///
    /// Returns an error when `format` is not [`ImageFormat::Rgba8`], the stride
    /// is not tightly packed, or the decoded length does not match the size.
    pub fn to_rgba8_buffer(&self) -> Result<ImageBuffer> {
        if self.format != ImageFormat::Rgba8 {
            return Err(ProtoError::Malformed(format!(
                "cannot build an RGBA8 buffer from a {:?} payload",
                self.format
            )));
        }
        let stride = self.width.checked_mul(4).ok_or_else(|| {
            ProtoError::Malformed(format!(
                "image width {} exceeds the maximum RGBA stride",
                self.width
            ))
        })?;
        match self.stride {
            Some(declared) if declared == stride => {}
            Some(declared) => {
                return Err(ProtoError::Malformed(format!(
                    "RGBA stride {declared} is not tightly packed for width {} (expected {stride})",
                    self.width
                )))
            }
            None => {
                return Err(ProtoError::Malformed(format!(
                    "rgba8 payload for {}x{} is missing its stride",
                    self.width, self.height
                )))
            }
        }
        let data = self.decode_data()?;
        let expected = u64::from(stride) * u64::from(self.height);
        if expected != data.len() as u64 {
            return Err(ProtoError::Malformed(format!(
                "RGBA data length {} does not match {}x{} (expected {expected} bytes)",
                data.len(),
                self.width,
                self.height
            )));
        }
        Ok(ImageBuffer {
            width: self.width,
            height: self.height,
            stride,
            format: PixelFormat::Rgba8,
            data,
        })
    }
}
