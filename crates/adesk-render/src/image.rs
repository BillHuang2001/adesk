//! Image post-processing and read-back conversion.
//!
//! These are pure transformations over [`adesk_core::ImageBuffer`]: no
//! renderer, no GPU, no I/O. They are the deterministic tail of every capture
//! and are unit-tested with exact-pixel assertions.

use std::borrow::Cow;

use adesk_core::{ImageBuffer, Rect, Size};
use image::ImageEncoder;

use crate::error::{RenderError, Result};

/// Bytes per pixel of [`adesk_core::PixelFormat::Rgba8`].
const BPP: usize = 4;

/// Crops `image` to `rect` (image coordinates), clamping to the image bounds.
///
/// A rect that does not intersect the image (or is empty) yields an empty
/// `0x0` image, never an error. The result is always tightly packed (a
/// whole-image crop of an already tight image is returned as a plain copy).
pub fn crop(image: &ImageBuffer, rect: Rect) -> ImageBuffer {
    let bounds = image.rect();
    let Some(clipped) = rect.intersect(&bounds) else {
        return ImageBuffer::new_rgba(0, 0);
    };
    if clipped == bounds && image.stride as usize == image.width as usize * BPP {
        // Whole-image crop of an already tight buffer: the row-by-row copy would
        // reproduce `image` exactly, so skip the zero-filled allocation and take
        // the single clone instead.
        return image.clone();
    }
    let mut out = ImageBuffer::new_rgba(clipped.w, clipped.h);
    let src_stride = image.stride as usize;
    let row_bytes = clipped.w as usize * BPP;
    for row in 0..clipped.h as usize {
        let src_start = (clipped.y as usize + row) * src_stride + clipped.x as usize * BPP;
        let dst_start = row * row_bytes;
        out.data[dst_start..dst_start + row_bytes]
            .copy_from_slice(&image.data[src_start..src_start + row_bytes]);
    }
    out
}

/// Scales `size` down so its longest edge is at most `max_dimension`,
/// preserving aspect ratio (each edge at least 1 pixel).
///
/// Returns `size` unchanged when `max_dimension` is `0`, when the image already
/// fits, or when either dimension is zero. Edge lengths are rounded half-up.
pub fn fit_dimensions(size: Size, max_dimension: u32) -> Size {
    let longest = size.w.max(size.h);
    if max_dimension == 0 || size.w == 0 || size.h == 0 || longest <= max_dimension {
        return size;
    }
    let longest = longest as u64;
    let bound = max_dimension as u64;
    let scale = |value: u32| -> u32 {
        let value = value as u64;
        let scaled = (value * bound + longest / 2) / longest;
        scaled.max(1).min(value) as u32
    };
    Size::new(scale(size.w), scale(size.h))
}

/// Downscales `image` with an integer-boundary box filter.
///
/// The output size is [`fit_dimensions`]`(image.size(), max_dimension)`. Each
/// output pixel is the exact average of the source pixels in its box
/// `[dx*w/dw, (dx+1)*w/dw) x [dy*h/dh, (dy+1)*h/dh)` (integer division, so
/// every source pixel contributes to exactly one output pixel), rounded
/// half-up per channel. Returns a copy of the image when it already fits or
/// `max_dimension` is `0`.
pub fn downscale(image: &ImageBuffer, max_dimension: u32) -> ImageBuffer {
    // The no-op cases (bound disabled, degenerate image, already fits) are exactly
    // the conditions under which `fit_dimensions` returns the size unchanged, so
    // they are checked first: the only work left is the single output allocation.
    let longest = image.width.max(image.height);
    if max_dimension == 0 || image.width == 0 || image.height == 0 || longest <= max_dimension {
        return image.clone();
    }
    let target = fit_dimensions(image.size(), max_dimension);
    let mut out = ImageBuffer::new_rgba(target.w, target.h);
    let src_stride = image.stride as usize;
    let (sw, sh) = (image.width as u64, image.height as u64);
    let (dw, dh) = (target.w as u64, target.h as u64);
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = (((dy + 1) * sh / dh).max(y0 + 1)).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = (((dx + 1) * sw / dw).max(x0 + 1)).min(sw);
            let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
            let mut count = 0u64;
            for y in y0..y1 {
                let row = y as usize * src_stride;
                for x in x0..x1 {
                    let off = row + x as usize * BPP;
                    r += image.data[off] as u64;
                    g += image.data[off + 1] as u64;
                    b += image.data[off + 2] as u64;
                    a += image.data[off + 3] as u64;
                    count += 1;
                }
            }
            let round = |sum: u64| ((sum + count / 2) / count) as u8;
            let off = (dy as usize * dw as usize + dx as usize) * BPP;
            out.data[off] = round(r);
            out.data[off + 1] = round(g);
            out.data[off + 2] = round(b);
            out.data[off + 3] = round(a);
        }
    }
    out
}

/// Encodes `image` as a PNG byte stream.
///
/// The AGP `ImagePayload.data` field is base64 of exactly these bytes
/// (`adesk-proto` performs the base64 step). Rows are re-packed to a tight
/// stride first, so images with padded strides encode correctly; an image whose
/// stride is already tight is encoded straight from its own pixel data, without
/// copying it. Empty images cannot be encoded and produce
/// [`RenderError::InvalidImage`]; so does a buffer holding fewer bytes than its
/// stride and height require.
pub fn encode_png(image: &ImageBuffer) -> Result<Vec<u8>> {
    if image.width == 0 || image.height == 0 {
        return Err(RenderError::InvalidImage {
            reason: "cannot encode an empty image".to_string(),
        });
    }
    let tight = tight_rgba(image)?;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &tight,
            image.width,
            image.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|err| RenderError::Encode {
            reason: err.to_string(),
        })?;
    Ok(out)
}

/// Converts raw read-back bytes into a tightly packed [`ImageBuffer`].
///
/// `stride` is the source row length in bytes (`>= width * 4`); `flipped` is
/// the caller's statement about the row order of `data`: `true` means the rows
/// run bottom-to-top and must be reversed, `false` means they are already
/// top-to-bottom in the caller's coordinate space. It is a general utility, not
/// a renderer-specific signal — the caller decides. Padding bytes and the row
/// order are normalized away here.
///
/// A tight, unflipped buffer (the common read-back layout) is copied in a single
/// pass over `width * height * 4` bytes; padded and/or flipped rows are written
/// into one pre-sized allocation in output order, so the result is never built
/// through a zero-filled intermediate.
pub fn image_from_readback(
    data: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    flipped: bool,
) -> Result<ImageBuffer> {
    if width == 0 || height == 0 {
        return Err(RenderError::InvalidImage {
            reason: format!("readback dimensions {width}x{height} are empty"),
        });
    }
    let row_bytes = width as usize * BPP;
    let stride = stride as usize;
    if stride < row_bytes {
        return Err(RenderError::InvalidImage {
            reason: format!("readback stride {stride} is smaller than one {width}-pixel row ({row_bytes} bytes)"),
        });
    }
    let needed = stride
        .checked_mul(height as usize)
        .ok_or_else(|| RenderError::InvalidImage {
            reason: "readback dimensions overflow".to_string(),
        })?;
    if data.len() < needed {
        return Err(RenderError::InvalidImage {
            reason: format!(
                "readback buffer holds {} bytes, {needed} required for {width}x{height} at stride {stride}",
                data.len()
            ),
        });
    }
    let out = if stride == row_bytes && !flipped {
        // Fast path: already tight and already top-to-bottom, so the payload is
        // exactly the first `width * height * 4` bytes — one copy, no zero-fill.
        data[..needed].to_vec()
    } else {
        // Padded rows and/or bottom-to-top rows: one pre-sized allocation written
        // in output order, with no zero-filled intermediate buffer.
        let mut out = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let src_row = if flipped {
                height as usize - 1 - row
            } else {
                row
            };
            let start = src_row * stride;
            out.extend_from_slice(&data[start..start + row_bytes]);
        }
        out
    };
    ImageBuffer::from_rgba(width, height, out).map_err(|err| RenderError::InvalidImage {
        reason: err.to_string(),
    })
}

/// Returns the image rows tightly packed.
///
/// Borrows `image.data` unchanged when the stride is already tight (no copy at
/// all), and allocates a repacked copy only when the stride is padded. A buffer
/// holding fewer bytes than `stride * height` requires is rejected as
/// [`RenderError::InvalidImage`] instead of panicking on a row slice.
fn tight_rgba(image: &ImageBuffer) -> Result<Cow<'_, [u8]>> {
    let row_bytes = image.width as usize * BPP;
    let stride = image.stride as usize;
    let needed =
        stride
            .checked_mul(image.height as usize)
            .ok_or_else(|| RenderError::InvalidImage {
                reason: "image dimensions overflow".to_string(),
            })?;
    if image.data.len() < needed {
        return Err(RenderError::InvalidImage {
            reason: format!(
                "image buffer holds {} bytes, {needed} required for {}x{} at stride {stride}",
                image.data.len(),
                image.width,
                image.height
            ),
        });
    }
    if stride == row_bytes {
        return Ok(Cow::Borrowed(&image.data[..needed]));
    }
    let mut out = Vec::with_capacity(row_bytes * image.height as usize);
    for row in 0..image.height as usize {
        let start = row * stride;
        out.extend_from_slice(&image.data[start..start + row_bytes]);
    }
    Ok(Cow::Owned(out))
}
