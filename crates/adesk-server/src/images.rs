//! Encoding `adesk_core::ImageBuffer` into AGP `ImagePayload`s (`docs/protocol.md` §4).
//!
//! The runtime renders into RGBA8 buffers; this module is the only place that
//! knows the wire encodings (`png` default, `rgba8` opt-in). Agent-facing
//! capture paths never include overlays.

use std::borrow::Cow;

use adesk_core::ImageBuffer;
use adesk_proto::{ImageFormat, ImagePayload, ProtoError};
use adesk_render::RenderError;

use crate::error::{Result, ServerError};

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
    match format {
        ImageFormat::Rgba8 => {
            let tight = tightly_packed(image)?;
            Ok(ImagePayload::from_rgba8(
                image.width,
                image.height,
                &tight,
                scale,
            )?)
        }
        ImageFormat::Png => {
            let png = encode_png(image)?;
            Ok(ImagePayload::from_png(
                image.width,
                image.height,
                &png,
                scale,
            ))
        }
    }
}

/// Encodes an RGBA8 buffer as PNG bytes.
///
/// The encoding itself is delegated to [`adesk_render::encode_png`] — the
/// workspace's single PNG encoder — after a cheap shape check of the buffer
/// (`buffer_shape`); no row packing is done here.
///
/// # Errors
///
/// Returns [`crate::ServerError::Proto`] when the buffer cannot describe
/// `width x height` pixels, and [`crate::ServerError::Render`] if the encoder
/// rejects the image (an empty `0x0` image).
pub fn encode_png(image: &ImageBuffer) -> Result<Vec<u8>> {
    buffer_shape(image)?;
    if image.width == 0 || image.height == 0 {
        // `adesk_render::encode_png` refuses empty images with
        // `RenderError::InvalidImage`; the server reports the encoder failure
        // instead, which is what its own encoder produced before the delegation.
        return Err(RenderError::Encode {
            reason: format!(
                "cannot encode an empty {}x{} image",
                image.width, image.height
            ),
        }
        .into());
    }
    Ok(adesk_render::encode_png(image)?)
}

/// Returns `image`'s pixels with tightly packed rows (`width * bytes_per_pixel`).
///
/// [`ImageBuffer`] rows may be padded, but every wire encoding (`rgba8` and the
/// PNG encoder) needs a tight buffer, so the padding is stripped here. A buffer
/// that is already tight is borrowed (no copy); otherwise the rows are copied
/// into a fresh `Vec`. Trailing bytes beyond the last row are ignored; a buffer
/// that is too short to describe `width x height` pixels is a caller bug and
/// never panics.
///
/// # Errors
///
/// Returns [`ServerError::Proto`] when the buffer cannot describe `width x height`
/// pixels: a stride smaller than one row, or data shorter than `stride * height`.
fn tightly_packed(image: &ImageBuffer) -> Result<Cow<'_, [u8]>> {
    let (row_bytes, stride) = buffer_shape(image)?;
    if stride == row_bytes {
        // Already tight: only the (possibly present) trailing bytes are dropped.
        let len = row_bytes * image.height as usize;
        return Ok(Cow::Borrowed(&image.data[..len]));
    }
    let mut tight = Vec::with_capacity(row_bytes * image.height as usize);
    for row in 0..image.height as usize {
        let start = row * stride;
        tight.extend_from_slice(&image.data[start..start + row_bytes]);
    }
    Ok(Cow::Owned(tight))
}

/// Validates that `image`'s buffer describes `width x height` pixels and
/// returns `(row_bytes, stride)` in bytes.
///
/// This is the server's malformed-buffer guard: a stride shorter than one row,
/// or fewer data bytes than `stride * height`, is a [`ServerError::Proto`] rather
/// than `adesk-render`'s `RenderError::InvalidImage`. It is a length/stride check
/// only, so a valid buffer pays no row packing here.
///
/// # Errors
///
/// Returns [`ServerError::Proto`] when the buffer cannot describe `width x height`
/// pixels.
fn buffer_shape(image: &ImageBuffer) -> Result<(usize, usize)> {
    let row_bytes = u64::from(image.width) * image.format.bytes_per_pixel() as u64;
    let stride = u64::from(image.stride);
    if stride < row_bytes {
        return Err(malformed(
            image,
            format!("stride {stride} is shorter than one {row_bytes}-byte row"),
        ));
    }
    let required = stride * u64::from(image.height);
    if (image.data.len() as u64) < required {
        return Err(malformed(
            image,
            format!("data length {} is shorter than the {required} bytes described by stride {stride} x height {}", image.data.len(), image.height),
        ));
    }
    Ok((row_bytes as usize, stride as usize))
}

/// Builds the malformed-buffer error (dimensions only, never pixel bytes).
fn malformed(image: &ImageBuffer, reason: String) -> ServerError {
    ServerError::Proto(ProtoError::Malformed(format!(
        "cannot encode a {}x{} {:?} image (stride {}, {} data bytes): {reason}",
        image.width,
        image.height,
        image.format,
        image.stride,
        image.data.len()
    )))
}

#[cfg(test)]
mod tests {
    use adesk_core::PixelFormat;

    use super::*;

    /// Deterministic, non-repeating pixel for `(x, y)`.
    fn pattern(x: u32, y: u32) -> [u8; 4] {
        [
            (x * 7 + 1) as u8,
            (y * 11 + 2) as u8,
            (x + y * 3 + 3) as u8,
            0xff,
        ]
    }

    /// Tightly packed RGBA8 image filled with [`pattern`].
    fn make(width: u32, height: u32) -> ImageBuffer {
        let mut image = ImageBuffer::new_rgba(width, height);
        let stride = image.stride as usize;
        for y in 0..height {
            for x in 0..width {
                let offset = y as usize * stride + x as usize * 4;
                image.data[offset..offset + 4].copy_from_slice(&pattern(x, y));
            }
        }
        image
    }

    /// Image whose rows carry `stride - width * 4` bytes of padding (`0xEE`).
    fn make_padded(width: u32, height: u32, stride: u32) -> ImageBuffer {
        assert!(stride >= width * 4, "stride must hold one row");
        let mut data = vec![0xEEu8; stride as usize * height as usize];
        for y in 0..height {
            for x in 0..width {
                let offset = y as usize * stride as usize + x as usize * 4;
                data[offset..offset + 4].copy_from_slice(&pattern(x, y));
            }
        }
        ImageBuffer {
            width,
            height,
            stride,
            format: PixelFormat::Rgba8,
            data,
        }
    }

    /// Tightly packed pixel bytes of `image` (padding stripped).
    fn tight_bytes(image: &ImageBuffer) -> Vec<u8> {
        let row = image.width as usize * 4;
        let mut out = Vec::with_capacity(row * image.height as usize);
        for y in 0..image.height as usize {
            let start = y * image.stride as usize;
            out.extend_from_slice(&image.data[start..start + row]);
        }
        out
    }

    fn decode_png(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).expect("valid png").to_rgba8()
    }

    #[test]
    fn rgba8_payload_is_tightly_packed() {
        let image = make(3, 2);
        let payload = encode(&image, ImageFormat::Rgba8, 1.0).unwrap();
        assert_eq!(payload.width, 3);
        assert_eq!(payload.height, 2);
        assert_eq!(payload.format, ImageFormat::Rgba8);
        assert_eq!(payload.stride, Some(12));
        assert_eq!(payload.scale, 1.0);
        assert_eq!(payload.decode_data().unwrap(), tight_bytes(&image));
    }

    #[test]
    fn rgba8_payload_round_trips_to_the_input_buffer() {
        let image = make(4, 3);
        let payload = encode(&image, ImageFormat::Rgba8, 1.0).unwrap();
        let decoded = payload.to_rgba8_buffer().unwrap();
        assert_eq!(decoded, image);
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(decoded.pixel(x, y), Some(pattern(x, y)));
            }
        }
    }

    #[test]
    fn rgba8_payload_keeps_the_reported_scale_without_rescaling() {
        let image = make(4, 2);
        let payload = encode(&image, ImageFormat::Rgba8, 0.5).unwrap();
        assert_eq!(payload.scale, 0.5);
        assert_eq!((payload.width, payload.height), (4, 2));
        assert_eq!(payload.decode_data().unwrap(), tight_bytes(&image));
    }

    #[test]
    fn rgba8_payload_repacks_padded_stride() {
        let image = make_padded(2, 2, 16);
        let payload = encode(&image, ImageFormat::Rgba8, 1.0).unwrap();
        assert_eq!(payload.stride, Some(8));
        assert_eq!(payload.decode_data().unwrap(), tight_bytes(&image));
        assert!(!payload.decode_data().unwrap().contains(&0xEE));
        let decoded = payload.to_rgba8_buffer().unwrap();
        assert_eq!(decoded.stride, 8);
        assert_eq!(decoded.pixel(1, 1), Some(pattern(1, 1)));
    }

    #[test]
    fn rgba8_payload_encodes_an_empty_image() {
        let payload = encode(&ImageBuffer::new_rgba(0, 0), ImageFormat::Rgba8, 1.0).unwrap();
        assert_eq!((payload.width, payload.height), (0, 0));
        assert_eq!(payload.stride, Some(0));
        assert!(payload.decode_data().unwrap().is_empty());
    }

    #[test]
    fn png_payload_carries_png_bytes() {
        let image = make(3, 2);
        let payload = encode(&image, ImageFormat::Png, 1.0).unwrap();
        assert_eq!(payload.width, 3);
        assert_eq!(payload.height, 2);
        assert_eq!(payload.format, ImageFormat::Png);
        assert_eq!(payload.stride, None);
        assert_eq!(payload.scale, 1.0);
        let png = payload.decode_data().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "png signature");
    }

    #[test]
    fn png_payload_round_trips_exact_pixels() {
        let image = make(4, 3);
        let payload = encode(&image, ImageFormat::Png, 1.0).unwrap();
        let decoded = decode_png(&payload.decode_data().unwrap());
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 3);
        assert_eq!(decoded.as_raw().as_slice(), tight_bytes(&image).as_slice());
    }

    #[test]
    fn png_payload_keeps_the_reported_scale() {
        let payload = encode(&make(2, 2), ImageFormat::Png, 0.25).unwrap();
        assert_eq!(payload.scale, 0.25);
        assert_eq!((payload.width, payload.height), (2, 2));
    }

    #[test]
    fn png_payload_repacks_padded_stride() {
        let image = make_padded(2, 2, 24);
        let payload = encode(&image, ImageFormat::Png, 1.0).unwrap();
        let decoded = decode_png(&payload.decode_data().unwrap());
        assert_eq!(decoded.get_pixel(0, 0).0, pattern(0, 0));
        assert_eq!(decoded.get_pixel(1, 1).0, pattern(1, 1));
        assert_eq!(decoded.as_raw().as_slice(), tight_bytes(&image).as_slice());
    }

    #[test]
    fn encode_png_returns_the_same_bytes_as_the_png_payload() {
        let image = make(2, 2);
        let png = encode_png(&image).unwrap();
        let payload = encode(&image, ImageFormat::Png, 1.0).unwrap();
        assert_eq!(payload.decode_data().unwrap(), png);
        assert_eq!(decode_png(&png).as_raw().as_slice(), tight_bytes(&image));
    }

    #[test]
    fn encode_png_delegates_to_adesk_render_byte_for_byte() {
        let tight = make(4, 3);
        let padded = make_padded(4, 3, 20);
        let mut with_trailing = make(3, 2);
        with_trailing.data.extend_from_slice(&[0xEE; 9]);

        for image in [&tight, &padded, &with_trailing] {
            assert_eq!(
                encode_png(image).unwrap(),
                adesk_render::encode_png(image).unwrap(),
                "delegated PNG bytes must match adesk-render for {image:?}"
            );
        }
    }
    #[test]
    fn short_data_is_rejected_without_panicking() {
        let mut image = make(2, 2);
        image.data.truncate(4); // stride 8 x height 2 needs 16 bytes
        let err = encode(&image, ImageFormat::Rgba8, 1.0).unwrap_err();
        assert!(matches!(err, ServerError::Proto(ProtoError::Malformed(_))));
        assert!(err.to_string().contains("shorter"), "{err}");

        let err = encode(&image, ImageFormat::Png, 1.0).unwrap_err();
        assert!(matches!(err, ServerError::Proto(ProtoError::Malformed(_))));
    }

    #[test]
    fn stride_shorter_than_a_row_is_rejected() {
        let image = ImageBuffer {
            width: 2,
            height: 1,
            stride: 4,
            format: PixelFormat::Rgba8,
            data: vec![0; 4],
        };
        let err = encode(&image, ImageFormat::Rgba8, 1.0).unwrap_err();
        assert!(matches!(err, ServerError::Proto(ProtoError::Malformed(_))));
        assert!(err.to_string().contains("stride 4"), "{err}");
        assert!(encode_png(&image).is_err());
    }

    #[test]
    fn trailing_bytes_beyond_the_last_row_are_ignored() {
        let mut image = make(2, 1);
        image.data.extend_from_slice(&[0xEE; 8]);
        let payload = encode(&image, ImageFormat::Rgba8, 1.0).unwrap();
        assert_eq!(payload.decode_data().unwrap(), tight_bytes(&image));
    }

    #[test]
    fn tightly_packed_borrows_a_tight_buffer_and_copies_a_padded_one() {
        let tight = make(3, 2);
        assert!(matches!(tightly_packed(&tight).unwrap(), Cow::Borrowed(_)));

        let padded = make_padded(3, 2, 16);
        assert!(matches!(tightly_packed(&padded).unwrap(), Cow::Owned(_)));
        assert_eq!(
            tightly_packed(&padded).unwrap().into_owned(),
            tight_bytes(&padded)
        );
    }

    #[test]
    fn tight_buffer_with_trailing_bytes_is_borrowed_without_the_tail() {
        let mut image = make(2, 1);
        let row = image.width as usize * 4;
        image.data.extend_from_slice(&[0xEE; 4]);
        let tight = tightly_packed(&image).unwrap();
        let Cow::Borrowed(bytes) = tight else {
            panic!("tight buffer must be borrowed");
        };
        assert_eq!(bytes, &image.data[..row]);
    }

    #[test]
    fn png_encoding_rejects_an_empty_image() {
        let err = encode_png(&ImageBuffer::new_rgba(0, 0)).unwrap_err();
        assert!(matches!(
            err,
            ServerError::Render(RenderError::Encode { .. })
        ));
    }
}
