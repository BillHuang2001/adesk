//! Pure image decoding from a VAP `ImagePayload` to tightly packed RGBA8.
//!
//! VAP frames carry an AGP `ImagePayload` (`adesk_proto::ImagePayload`), which is
//! either base64 PNG bytes or base64 raw RGBA8 pixels
//! (`docs/viewer.md` §3, `docs/protocol.md` §4). This module normalizes both
//! encodings into one shape the GTK layer can hand to `gdk::Texture` without
//! doing any decoding itself — and, being GTK-free, the decoding is unit-testable
//! without a display.

use adesk_proto::{ImageFormat, ImagePayload};

use crate::error::{GuiError, Result};

/// A decoded image as row-major, tightly packed RGBA8 pixels.
///
/// `rgba8` always holds exactly `width * height * 4` bytes (no row padding);
/// pixel `(x, y)` starts at byte offset `(y * width + x) * 4` in R,G,B,A order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major, tightly packed RGBA8 (`width * height * 4` bytes).
    pub rgba8: Vec<u8>,
}

/// Decodes an `ImagePayload` into a [`DecodedImage`].
///
/// `ImageFormat::Rgba8` payloads are validated and unwrapped via
/// [`ImagePayload::to_rgba8_buffer`]; `ImageFormat::Png` payloads are decoded
/// with the `image` crate and converted to RGBA8.
///
/// # Errors
///
/// Returns [`GuiError::Image`] for an undecodable PNG, invalid base64, or an
/// RGBA8 payload whose stride/length does not match its declared size. It never
/// panics on malformed input.
pub fn decode(payload: &ImagePayload) -> Result<DecodedImage> {
    match payload.format {
        ImageFormat::Rgba8 => {
            let buffer = payload
                .to_rgba8_buffer()
                .map_err(|error| GuiError::Image(error.to_string()))?;
            Ok(DecodedImage {
                width: buffer.width,
                height: buffer.height,
                rgba8: buffer.data,
            })
        }
        ImageFormat::Png => {
            let bytes = payload
                .decode_data()
                .map_err(|error| GuiError::Image(error.to_string()))?;
            let image = image::load_from_memory(&bytes)
                .map_err(|error| GuiError::Image(format!("PNG decode failed: {error}")))?;
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            Ok(DecodedImage {
                width,
                height,
                rgba8: rgba.into_raw(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    /// Encodes RGBA8 bytes (2x2, four distinct colors) as a PNG.
    fn sample_png() -> Vec<u8> {
        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];
        let image = image::RgbaImage::from_raw(2, 2, pixels).expect("2x2 RGBA image");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("PNG encoding");
        png
    }

    #[test]
    fn rgba8_payloads_round_trip_exactly() {
        let pixels: Vec<u8> = (0..(3 * 2 * 4)).map(|byte| byte as u8).collect();
        let payload = ImagePayload::from_rgba8(3, 2, &pixels, 1.0).unwrap();
        let decoded = decode(&payload).unwrap();
        assert_eq!(decoded.width, 3);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba8, pixels);
    }

    #[test]
    fn png_payloads_decode_with_their_dimensions_and_pixels() {
        let png = sample_png();
        let payload = ImagePayload::from_png(2, 2, &png, 1.0);
        let decoded = decode(&payload).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba8.len(), 2 * 2 * 4);
        // The top-left pixel (red) survives the PNG round trip.
        assert_eq!(&decoded.rgba8[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn an_undecodable_png_is_an_image_error() {
        let payload = ImagePayload::from_png(2, 2, b"not a png", 1.0);
        assert!(matches!(decode(&payload).unwrap_err(), GuiError::Image(_)));
    }

    #[test]
    fn an_rgba8_payload_with_a_wrong_length_is_an_image_error() {
        // Build a valid payload, then corrupt the declared width so the byte
        // count no longer matches the (still tight) stride.
        let mut payload = ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0).unwrap();
        payload.width = 2;
        assert!(matches!(decode(&payload).unwrap_err(), GuiError::Image(_)));
    }
}
