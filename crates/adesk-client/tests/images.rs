//! `decode_image` (src/image.rs): PNG and RGBA8 payloads, stride repacking,
//! and failure modes — all offline, with no display, GPU or network.

use adesk_client::{decode_image, ClientError, ImagePayload};
use adesk_core::PixelFormat;
use adesk_proto::ImageFormat as WireFormat;
use base64::Engine as _;
use image::ImageEncoder as _;

/// Four distinct 2x2 source pixels, so repacking or channel-order bugs show up.
const PIXELS: [[u8; 4]; 4] = [
    [255, 0, 0, 255],     // (0,0) red
    [0, 255, 0, 255],     // (1,0) green
    [0, 0, 255, 255],     // (0,1) blue
    [255, 255, 255, 128], // (1,1) translucent white
];

/// Padding written after every row of the padded-stride fixture.
const PADDING: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];

/// The 2x2 source pixels in tight row-major RGBA8 order.
fn rgba_bytes() -> Vec<u8> {
    PIXELS.iter().flatten().copied().collect()
}

/// Encode RGBA8 bytes as a real PNG (same encoder the renderer uses).
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .expect("encode test PNG");
    out
}

/// Assert `error` is `ClientError::Image` with a non-empty message and return it.
fn assert_is_image_error(error: ClientError) -> String {
    match error {
        ClientError::Image { message } => {
            assert!(
                !message.trim().is_empty(),
                "image error message must not be empty"
            );
            message
        }
        other => panic!("expected ClientError::Image, got {other:?}"),
    }
}

/// A PNG payload decodes to an `ImageBuffer`.
///
/// Build an `ImagePayload` with format 'png', the width/height of a tiny
/// (for example 2x2) PNG whose pixels are known, and base64 'data'; assert
/// `decode_image` returns an `ImageBuffer` with the same width/height,
/// `PixelFormat::Rgba8` and the expected pixel values.
#[tokio::test]
async fn decode_png_payload() {
    let png = encode_png(2, 2, &rgba_bytes());
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "fixture is a real PNG");
    let payload = ImagePayload::from_png(2, 2, &png, 1.0);

    let buffer = decode_image(&payload).expect("decode a valid PNG payload");

    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 2);
    assert_eq!(buffer.format, PixelFormat::Rgba8);
    assert_eq!(buffer.stride, 8, "decoded PNG rows are tightly packed");
    assert_eq!(buffer.data.len(), 16);
    for (index, expected) in PIXELS.iter().enumerate() {
        let (x, y) = ((index % 2) as u32, (index / 2) as u32);
        assert_eq!(buffer.pixel(x, y), Some(*expected), "pixel ({x},{y})");
    }
}

/// A tightly packed RGBA8 payload decodes unchanged.
///
/// Build an 'rgba8' payload with 'stride' equal to width * 4 and base64 raw
/// RGBA bytes; assert the decoded `ImageBuffer` has the same bytes
/// (`data.len()` equals width * height * 4) and matching pixels.
#[tokio::test]
async fn decode_rgba8_tight() {
    let bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    let payload = ImagePayload::from_rgba8(2, 1, &bytes, 1.0).expect("build tight rgba8 payload");
    assert_eq!(payload.stride, Some(8));

    let buffer = decode_image(&payload).expect("decode a tight rgba8 payload");

    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 1);
    assert_eq!(buffer.format, PixelFormat::Rgba8);
    assert_eq!(buffer.stride, 8);
    assert_eq!(buffer.data, bytes, "tight data is passed through unchanged");
    assert_eq!(buffer.data.len(), 8, "2x1 pixels at 4 bytes each");
    assert_eq!(buffer.pixel(0, 0), Some([1, 2, 3, 4]));
    assert_eq!(buffer.pixel(1, 0), Some([5, 6, 7, 8]));
}

/// An RGBA8 payload with padded rows is repacked to a tight layout.
///
/// Build an 'rgba8' payload whose 'stride' is greater than width * 4 and pad
/// each row with sentinel bytes; assert the decoded buffer drops the padding
/// (`data.len()` equals width * height * 4) and every pixel matches the source.
#[tokio::test]
async fn decode_rgba8_with_stride_repacked() {
    // 2 pixels per row = 8 bytes of pixels + 4 bytes of padding per row.
    let mut padded = Vec::new();
    for row in 0..2 {
        for column in 0..2 {
            padded.extend_from_slice(&PIXELS[row * 2 + column]);
        }
        padded.extend_from_slice(&PADDING);
    }
    assert_eq!(padded.len(), 2 * 12);
    let payload = ImagePayload {
        width: 2,
        height: 2,
        format: WireFormat::Rgba8,
        stride: Some(12),
        data: base64::engine::general_purpose::STANDARD.encode(&padded),
        scale: 1.0,
    };

    let buffer = decode_image(&payload).expect("decode a padded rgba8 payload");

    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 2);
    assert_eq!(buffer.format, PixelFormat::Rgba8);
    assert_eq!(buffer.stride, 8, "repacked rows are tight");
    assert_eq!(buffer.data.len(), 2 * 2 * 4, "padding is dropped");
    assert_eq!(buffer.data, rgba_bytes(), "only the pixel bytes survive");
    assert!(
        !buffer
            .data
            .windows(PADDING.len())
            .any(|window| window == PADDING),
        "no padding sentinel survives the repack"
    );
    for (index, expected) in PIXELS.iter().enumerate() {
        let (x, y) = ((index % 2) as u32, (index / 2) as u32);
        assert_eq!(buffer.pixel(x, y), Some(*expected), "pixel ({x},{y})");
    }
}

/// Bad base64 is a `ClientError::Image`.
///
/// Use 'data' that is not valid base64; assert `decode_image` returns
/// `Err(ClientError::Image)` and never panics.
#[tokio::test]
async fn decode_bad_base64_is_image_error() {
    let payload = ImagePayload {
        width: 2,
        height: 2,
        format: WireFormat::Png,
        stride: None,
        data: "not base64!!".to_owned(),
        scale: 1.0,
    };

    let message = assert_is_image_error(decode_image(&payload).expect_err("bad base64 must fail"));

    assert!(
        message.contains("base64"),
        "message explains the failure: {message}"
    );
}

/// An unknown format is a `ClientError::Image`.
///
/// Build a payload whose 'format' is neither 'png' nor 'rgba8' (via the raw
/// JSON payload a server could send, or the proto type's unknown-format path);
/// assert `decode_image` returns `Err(ClientError::Image)`.
///
/// DISCREPANCY (reported to root): such a payload is **unconstructible in safe
/// Rust**. `ImagePayload::format` is `adesk_proto::ImageFormat`, a closed
/// two-variant enum (`Png`/`Rgba8`) with no `#[serde(other)]` catch-all, so
/// `"format": "webp"` is rejected while *decoding the frame* and never becomes a
/// typed payload; `decode_image` therefore has no reachable "unknown format"
/// arm (its match is exhaustive and has no defensive fallback). This test keeps
/// the frozen name and proves the failure mode the spec cares about: an unknown
/// format is rejected loudly and deterministically — never decoded, guessed or
/// panicked on — and `decode_image` dispatches on `format` alone, never sniffing
/// the bytes.
#[tokio::test]
async fn decode_unknown_format_is_image_error() {
    // 1. The wire cannot deliver an unknown format at all: serde rejects the
    //    frame before `decode_image` (or any other client code) can see it.
    let json = serde_json::json!({
        "width": 2,
        "height": 2,
        "format": "webp",
        "stride": 8,
        "data": base64::engine::general_purpose::STANDARD.encode(rgba_bytes()),
        "scale": 1.0,
    });
    let error = serde_json::from_value::<ImagePayload>(json)
        .expect_err("a payload with an unknown format must not deserialise");
    assert!(
        error.to_string().contains("webp"),
        "the rejection names the offending format: {error}"
    );

    // 2. Because `format` is a typed, exhaustive choice, decoding never guesses:
    //    bytes that look like a PNG header are returned verbatim as raw RGBA8
    //    when — and only when — the payload says `rgba8`.
    let png_signature = [0x89u8, b'P', b'N', b'G'];
    let payload =
        ImagePayload::from_rgba8(1, 1, &png_signature, 1.0).expect("build 1x1 rgba8 payload");
    let buffer = decode_image(&payload).expect("rgba8 payload decodes as raw pixels");
    assert_eq!(
        buffer.pixel(0, 0),
        Some(png_signature),
        "raw bytes are never sniffed for a different codec"
    );
}

/// A truncated PNG is a `ClientError::Image`.
///
/// Take a valid PNG, cut 'data' short (header intact, body truncated); assert
/// `decode_image` returns `Err(ClientError::Image)` rather than panicking.
#[tokio::test]
async fn decode_truncated_png_is_image_error() {
    let png = encode_png(2, 2, &rgba_bytes());
    // 8-byte signature + 25-byte IHDR chunk: keep the header, cut mid-body.
    let cut = 40;
    assert!(png.len() > cut, "fixture PNG is long enough to truncate");
    let truncated = &png[..cut];
    assert_eq!(&truncated[..8], b"\x89PNG\r\n\x1a\n", "header stays intact");
    let payload = ImagePayload {
        width: 2,
        height: 2,
        format: WireFormat::Png,
        stride: None,
        data: base64::engine::general_purpose::STANDARD.encode(truncated),
        scale: 1.0,
    };

    let message =
        assert_is_image_error(decode_image(&payload).expect_err("truncated PNG must fail"));

    assert!(
        message.starts_with("PNG decode failed"),
        "the decoder rejects the truncated body: {message}"
    );
}

// --- client-owned extras: the validation branches `decode_image` documents but
// --- the frozen six do not reach (length, stride and dimension mismatches).

/// An RGBA8 payload whose byte count does not match its stride is rejected.
#[tokio::test]
async fn decode_rgba8_length_mismatch_is_image_error() {
    // Too short for 2x1 at stride 8, and too short for 2x2 at stride 12.
    for (width, height, stride, len) in [(2u32, 1u32, 8u32, 4usize), (2, 2, 12, 20)] {
        let payload = ImagePayload {
            width,
            height,
            format: WireFormat::Rgba8,
            stride: Some(stride),
            data: base64::engine::general_purpose::STANDARD.encode(vec![0u8; len]),
            scale: 1.0,
        };
        let message =
            assert_is_image_error(decode_image(&payload).expect_err("short RGBA8 must fail"));
        assert!(
            message.contains("does not match"),
            "message explains the failure: {message}"
        );
    }
}

/// An RGBA8 stride narrower than a row is rejected instead of reading padding.
#[tokio::test]
async fn decode_rgba8_stride_below_row_width_is_image_error() {
    let payload = ImagePayload {
        width: 2,
        height: 1,
        format: WireFormat::Rgba8,
        stride: Some(4),
        data: base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]),
        scale: 1.0,
    };

    let message =
        assert_is_image_error(decode_image(&payload).expect_err("narrow stride must fail"));

    assert!(
        message.contains("smaller than"),
        "message explains the failure: {message}"
    );
}

/// An RGBA8 payload without a `stride` field is treated as tightly packed.
#[tokio::test]
async fn decode_rgba8_missing_stride_defaults_to_tight() {
    let bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    let payload = ImagePayload {
        width: 2,
        height: 1,
        format: WireFormat::Rgba8,
        stride: None,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        scale: 1.0,
    };

    let buffer = decode_image(&payload).expect("missing stride means tight rows");

    assert_eq!(buffer.stride, 8);
    assert_eq!(buffer.data, bytes);
}

/// A PNG whose dimensions disagree with the payload is rejected.
#[tokio::test]
async fn decode_png_dimension_mismatch_is_image_error() {
    let png = encode_png(2, 2, &rgba_bytes());
    let payload = ImagePayload::from_png(4, 4, &png, 1.0);

    let message =
        assert_is_image_error(decode_image(&payload).expect_err("lying dimensions must fail"));

    assert!(
        message.contains("2x2") && message.contains("4x4"),
        "message names both sizes: {message}"
    );
}
