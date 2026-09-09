//! Exact-pixel tests for the pure image post-processing path.
//!
//! No GPU, no display, no renderer: these run everywhere (CI included).

use adesk_core::{ImageBuffer, PixelFormat, Rect, Size};
use adesk_render::{crop, downscale, encode_png, fit_dimensions, image_from_readback};

/// Builds an image whose pixels are produced by `f(x, y)`.
fn make(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> ImageBuffer {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend_from_slice(&f(x, y));
        }
    }
    ImageBuffer::from_rgba(width, height, data).unwrap()
}

/// Builds an image with a padded stride (more bytes per row than `width * 4`).
fn make_padded(width: u32, height: u32, stride: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> ImageBuffer {
    let mut data = vec![0xEEu8; (stride * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let off = (y * stride + x * 4) as usize;
            data[off..off + 4].copy_from_slice(&f(x, y));
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

fn pattern(x: u32, y: u32) -> [u8; 4] {
    [x as u8 * 10, y as u8 * 10, (x + y) as u8, 0xFF]
}

#[test]
fn crop_copies_exact_pixels() {
    let src = make(4, 4, pattern);
    let out = crop(&src, Rect::new(1, 1, 2, 2));
    assert_eq!(out.size(), Size::new(2, 2));
    assert_eq!(out.stride, 8);
    for y in 0..2 {
        for x in 0..2 {
            assert_eq!(
                out.pixel(x, y),
                src.pixel(x + 1, y + 1),
                "pixel ({x},{y}) must be copied from source ({},{})",
                x + 1,
                y + 1
            );
        }
    }
}

#[test]
fn crop_full_image_is_identity() {
    let src = make(3, 2, pattern);
    let out = crop(&src, src.rect());
    assert_eq!(out, src);
}

#[test]
fn crop_clamps_negative_and_oversized_rects() {
    let src = make(4, 4, pattern);

    let top_left = crop(&src, Rect::new(-2, -2, 4, 4));
    assert_eq!(top_left.size(), Size::new(2, 2));
    assert_eq!(top_left.pixel(0, 0), src.pixel(0, 0));

    let bottom_right = crop(&src, Rect::new(3, 3, 9, 9));
    assert_eq!(bottom_right.size(), Size::new(1, 1));
    assert_eq!(bottom_right.pixel(0, 0), src.pixel(3, 3));
}

#[test]
fn crop_disjoint_and_empty_rects_yield_empty_image() {
    let src = make(4, 4, pattern);
    let disjoint = crop(&src, Rect::new(10, 10, 2, 2));
    assert_eq!(disjoint.size(), Size::ZERO);
    assert!(disjoint.data.is_empty());

    let empty = crop(&src, Rect::new(1, 1, 0, 5));
    assert_eq!(empty.size(), Size::ZERO);
}

#[test]
fn downscale_averages_each_box() {
    // 4x4 where every pixel is (x + 16*y) in the red channel.
    let src = make(4, 4, |x, y| [(x + 16 * y) as u8, 0, 0, 0xFF]);
    let out = downscale(&src, 2);
    assert_eq!(out.size(), Size::new(2, 2));
    // Boxes: (0,0)-(1,1) => (0,1,16,17) => 8.5 => 9 (half-up)
    assert_eq!(out.pixel(0, 0), Some([9, 0, 0, 0xFF]));
    // (2,0)-(3,1) => (2,3,18,19) => 10.5 => 11
    assert_eq!(out.pixel(1, 0), Some([11, 0, 0, 0xFF]));
    // (0,2)-(1,3) => (32,33,48,49) => 40.5 => 41
    assert_eq!(out.pixel(0, 1), Some([41, 0, 0, 0xFF]));
    // (2,2)-(3,3) => (34,35,50,51) => 42.5 => 43
    assert_eq!(out.pixel(1, 1), Some([43, 0, 0, 0xFF]));
}

#[test]
fn downscale_rounds_half_up() {
    let src = make(2, 2, |x, y| {
        let v = (x + 2 * y) as u8; // 0, 1, 2, 3
        [v, v, v, 0xFF]
    });
    let out = downscale(&src, 1);
    assert_eq!(out.size(), Size::new(1, 1));
    // average of 0,1,2,3 = 1.5 => 2 (half-up)
    assert_eq!(out.pixel(0, 0), Some([2, 2, 2, 0xFF]));
}

#[test]
fn downscale_is_noop_when_image_fits_or_bound_is_zero() {
    let src = make(3, 3, pattern);
    assert_eq!(downscale(&src, 8), src);
    assert_eq!(downscale(&src, 3), src);
    assert_eq!(downscale(&src, 0), src);
}

#[test]
fn downscale_preserves_aspect_ratio() {
    let src = make(4, 2, |x, y| [x as u8, y as u8, 0, 0xFF]);
    let out = downscale(&src, 2);
    assert_eq!(out.size(), Size::new(2, 1));
    // Each output averages a 2x2 source box: x in {0,1} and {2,3}, y in {0,1}.
    assert_eq!(out.pixel(0, 0), Some([1, 1, 0, 0xFF]));
    assert_eq!(out.pixel(1, 0), Some([3, 1, 0, 0xFF]));
}

#[test]
fn downscale_handles_non_integer_ratios() {
    // 5x5 -> bound 3 -> 3x3; source boxes are [0,1) [1,3) [3,5) per axis.
    let src = make(5, 5, |x, _y| [(x * 20) as u8, 0, 0, 0xFF]);
    let out = downscale(&src, 3);
    assert_eq!(out.size(), Size::new(3, 3));
    assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 0xFF]));
    assert_eq!(out.pixel(1, 0), Some([30, 0, 0, 0xFF])); // (20 + 40) / 2
    assert_eq!(out.pixel(2, 0), Some([70, 0, 0, 0xFF])); // (60 + 80) / 2
}

#[test]
fn fit_dimensions_rounds_half_up_and_never_upscales() {
    assert_eq!(fit_dimensions(Size::new(4, 4), 2), Size::new(2, 2));
    assert_eq!(fit_dimensions(Size::new(4, 2), 2), Size::new(2, 1));
    assert_eq!(fit_dimensions(Size::new(3, 3), 2), Size::new(2, 2));
    assert_eq!(fit_dimensions(Size::new(100, 10), 10), Size::new(10, 1));
    assert_eq!(fit_dimensions(Size::new(1, 1), 8), Size::new(1, 1));
    assert_eq!(fit_dimensions(Size::new(5, 7), 0), Size::new(5, 7));
    assert_eq!(fit_dimensions(Size::new(0, 7), 3), Size::new(0, 7));
}

#[test]
fn encode_png_round_trips_exact_pixels() {
    let src = make(2, 2, pattern);
    let png = encode_png(&src).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
    let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(decoded.width(), 2);
    assert_eq!(decoded.height(), 2);
    assert_eq!(decoded.as_raw().as_slice(), src.data.as_slice());
}

#[test]
fn encode_png_repacks_padded_stride() {
    let src = make_padded(2, 2, 16, pattern);
    let png = encode_png(&src).unwrap();
    let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0).0, pattern(0, 0));
    assert_eq!(decoded.get_pixel(1, 1).0, pattern(1, 1));
}

#[test]
fn encode_png_rejects_empty_images() {
    let err = encode_png(&ImageBuffer::new_rgba(0, 0)).unwrap_err();
    assert!(matches!(err, adesk_render::RenderError::InvalidImage { .. }));
}

#[test]
fn image_from_readback_packs_tight_rows() {
    let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    let image = image_from_readback(&data, 2, 1, 8, false).unwrap();
    assert_eq!(image.size(), Size::new(2, 1));
    assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
    assert_eq!(image.pixel(1, 0), Some([5, 6, 7, 8]));
}

#[test]
fn image_from_readback_strips_padding_and_unflips() {
    // Two rows, stride 12 (4 bytes of padding), stored bottom-to-top
    // (`flipped = true`; the caller states the row order).
    let row0 = [10u8, 11, 12, 13, 14, 15, 16, 17];
    let row1 = [20u8, 21, 22, 23, 24, 25, 26, 27];
    let mut data = Vec::new();
    data.extend_from_slice(&row1); // bottom row comes first when flipped
    data.extend_from_slice(&[0xAA, 0xAA, 0xAA, 0xAA]);
    data.extend_from_slice(&row0);
    data.extend_from_slice(&[0xAA, 0xAA, 0xAA, 0xAA]);

    let image = image_from_readback(&data, 2, 2, 12, true).unwrap();
    assert_eq!(image.stride, 8);
    assert_eq!(image.pixel(0, 0), Some([10, 11, 12, 13]));
    assert_eq!(image.pixel(1, 0), Some([14, 15, 16, 17]));
    assert_eq!(image.pixel(0, 1), Some([20, 21, 22, 23]));
    assert_eq!(image.pixel(1, 1), Some([24, 25, 26, 27]));
}

#[test]
fn image_from_readback_rejects_short_and_malformed_buffers() {
    let short = image_from_readback(&[0u8; 4], 2, 2, 8, false);
    assert!(matches!(
        short,
        Err(adesk_render::RenderError::InvalidImage { .. })
    ));

    let small_stride = image_from_readback(&[0u8; 64], 4, 4, 8, false);
    assert!(matches!(
        small_stride,
        Err(adesk_render::RenderError::InvalidImage { .. })
    ));

    let empty = image_from_readback(&[], 0, 0, 0, false);
    assert!(matches!(
        empty,
        Err(adesk_render::RenderError::InvalidImage { .. })
    ));
}
