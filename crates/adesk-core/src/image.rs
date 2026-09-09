//! Pixel buffers produced by the render pipeline.
//!
//! [`ImageBuffer`] is deliberately **not** wire-facing: the protocol carries
//! base64 image payloads defined in `adesk-proto`. Crop, downscale, PNG
//! encoding and damage-based re-render live in `adesk-render`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::geometry::{Rect, Size};

/// Pixel layout of an [`ImageBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    /// 8 bits per channel, red-green-blue-alpha order, straight alpha.
    Rgba8,
}

impl PixelFormat {
    /// Bytes per pixel for this format.
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::Rgba8 => 4,
        }
    }
}

/// A CPU-side pixel buffer.
///
/// `stride` is the number of bytes per row and is always at least
/// `width * bytes_per_pixel`; rows may be padded. The data is row-major,
/// starting at the top-left pixel.
///
/// `Debug` intentionally omits the pixel payload (it can be megabytes and
/// pixel data must never be logged).
#[derive(Clone, PartialEq, Eq)]
pub struct ImageBuffer {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row (`>= width * 4` for RGBA8).
    pub stride: u32,
    /// Pixel layout of `data`.
    pub format: PixelFormat,
    /// Row-major pixel data.
    pub data: Vec<u8>,
}

impl fmt::Debug for ImageBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageBuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("format", &self.format)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .finish()
    }
}

impl ImageBuffer {
    /// Creates a zeroed RGBA8 buffer filled with opaque black.
    ///
    /// Infallible by contract; dimensions whose byte size cannot be represented
    /// are saturated rather than wrapped (they are far beyond any real output).
    pub fn new_rgba(width: u32, height: u32) -> ImageBuffer {
        let stride = width.saturating_mul(4);
        let len = (stride as usize).saturating_mul(height as usize);
        let mut data = vec![0u8; len];
        for pixel in data.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        ImageBuffer {
            width,
            height,
            stride,
            format: PixelFormat::Rgba8,
            data,
        }
    }

    /// Creates an RGBA8 buffer from tightly packed data (`stride == width * 4`).
    ///
    /// Returns [`ErrorCode::InvalidRequest`](crate::ErrorCode::InvalidRequest)
    /// when the data length does not match `width * height * 4` or when the
    /// implied stride does not fit in `u32`.
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<ImageBuffer> {
        let stride = u64::from(width) * 4;
        if stride > u64::from(u32::MAX) {
            return Err(Error::invalid_request(format!(
                "image width {width} exceeds the maximum RGBA stride"
            )));
        }
        let expected = stride * u64::from(height);
        if expected != data.len() as u64 {
            return Err(Error::invalid_request(format!(
                "RGBA data length {} does not match {width}x{height} (expected {expected} bytes)",
                data.len()
            )));
        }
        Ok(ImageBuffer {
            width,
            height,
            stride: stride as u32,
            format: PixelFormat::Rgba8,
            data,
        })
    }

    /// Returns the RGBA value at `(x, y)`, or `None` when out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize)
            .checked_mul(self.stride as usize)?
            .checked_add((x as usize).checked_mul(4)?)?;
        let bytes = self.data.get(offset..offset.checked_add(4)?)?;
        Some([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Returns the buffer size in pixels.
    pub const fn size(&self) -> Size {
        Size {
            w: self.width,
            h: self.height,
        }
    }

    /// Returns the buffer bounds as a rect at the origin.
    pub const fn rect(&self) -> Rect {
        Rect::from_size(self.size())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn new_rgba_is_opaque_black() {
        let buf = ImageBuffer::new_rgba(2, 2);
        assert_eq!(buf.width, 2);
        assert_eq!(buf.height, 2);
        assert_eq!(buf.stride, 8);
        assert_eq!(buf.data.len(), 16);
        assert_eq!(buf.format, PixelFormat::Rgba8);
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(buf.pixel(x, y), Some([0, 0, 0, 255]));
            }
        }
    }

    #[test]
    fn new_rgba_zero_size() {
        let buf = ImageBuffer::new_rgba(0, 0);
        assert_eq!(buf.stride, 0);
        assert!(buf.data.is_empty());
        assert_eq!(buf.pixel(0, 0), None);
        assert_eq!(buf.size(), Size::ZERO);
        assert_eq!(buf.rect(), Rect::EMPTY);
    }

    #[test]
    fn new_rgba_does_not_allocate_for_zero_height() {
        let buf = ImageBuffer::new_rgba(u32::MAX, 0);
        assert!(buf.data.is_empty());
        assert_eq!(buf.stride, u32::MAX);
        assert_eq!(buf.pixel(0, 0), None);
    }

    #[test]
    fn from_rgba_accepts_tightly_packed_data() {
        let buf = ImageBuffer::from_rgba(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(buf.stride, 8);
        assert_eq!(buf.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(buf.pixel(1, 0), Some([5, 6, 7, 8]));
    }

    #[test]
    fn from_rgba_rejects_wrong_length() {
        let err = ImageBuffer::from_rgba(2, 1, vec![0; 7]).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(err.message.contains("does not match 2x1"));

        let err = ImageBuffer::from_rgba(1, 1, vec![]).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn from_rgba_rejects_stride_overflow() {
        let err = ImageBuffer::from_rgba(u32::MAX, 0, vec![]).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(err.message.contains("stride"));
    }

    #[test]
    fn from_rgba_accepts_empty_buffer() {
        let buf = ImageBuffer::from_rgba(0, 0, vec![]).unwrap();
        assert_eq!(buf.size(), Size::ZERO);
        assert!(buf.data.is_empty());
    }

    #[test]
    fn pixel_bounds_are_checked() {
        let buf = ImageBuffer::new_rgba(3, 2);
        assert!(buf.pixel(2, 1).is_some());
        assert_eq!(buf.pixel(3, 1), None);
        assert_eq!(buf.pixel(2, 2), None);
        assert_eq!(buf.pixel(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn pixel_reads_rows_via_stride() {
        // Hand-built buffer with padded rows: stride 12 for 2 pixels wide.
        let mut data = vec![0u8; 24];
        data[12] = 9; // first pixel of row 1
        data[13] = 8;
        data[14] = 7;
        data[15] = 6;
        let buf = ImageBuffer {
            width: 2,
            height: 2,
            stride: 12,
            format: PixelFormat::Rgba8,
            data,
        };
        assert_eq!(buf.pixel(0, 1), Some([9, 8, 7, 6]));
        assert_eq!(buf.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn size_and_rect_match_dimensions() {
        let buf = ImageBuffer::from_rgba(3, 2, vec![0; 24]).unwrap();
        assert_eq!(buf.size(), Size { w: 3, h: 2 });
        assert_eq!(
            buf.rect(),
            Rect {
                x: 0,
                y: 0,
                w: 3,
                h: 2
            }
        );
    }

    #[test]
    fn debug_omits_pixel_payload() {
        let buf = ImageBuffer::new_rgba(2, 2);
        let debug = format!("{buf:?}");
        assert!(debug.contains("width: 2"));
        assert!(debug.contains("stride: 8"));
        assert!(debug.contains("data: 16 bytes"));
    }

    #[test]
    fn pixel_format_bytes_per_pixel() {
        assert_eq!(PixelFormat::Rgba8.bytes_per_pixel(), 4);
        assert_eq!(
            serde_json::to_string(&PixelFormat::Rgba8).unwrap(),
            "\"rgba8\""
        );
    }
}
