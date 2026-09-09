//! `decode_image` (src/image.rs): PNG and RGBA8 payloads, stride repacking,
//! and failure modes — all offline, with no display, GPU or network.

mod common;

/// A PNG payload decodes to an `ImageBuffer`.
///
/// Build an `ImagePayload` with format 'png', the width/height of a tiny
/// (for example 2x2) PNG whose pixels are known, and base64 'data'; assert
/// `decode_image` returns an `ImageBuffer` with the same width/height,
/// `PixelFormat::Rgba8` and the expected pixel values.
#[tokio::test]
async fn decode_png_payload() {
    todo!(
        "build an ImagePayload with format 'png' from a 2x2 RGBA PNG encoded as base64; \
         call decode_image and assert ImageBuffer width 2, height 2, PixelFormat::Rgba8 and the four expected pixels"
    );
}

/// A tightly packed RGBA8 payload decodes unchanged.
///
/// Build an 'rgba8' payload with 'stride' equal to width * 4 and base64 raw
/// RGBA bytes; assert the decoded `ImageBuffer` has the same bytes
/// (`data.len()` equals width * height * 4) and matching pixels.
#[tokio::test]
async fn decode_rgba8_tight() {
    todo!(
        "build an ImagePayload with format 'rgba8', width 2, height 1, stride 8 and base64 of 8 raw RGBA bytes; \
         call decode_image and assert ImageBuffer stride 8, data length 8 and the two expected pixels"
    );
}

/// An RGBA8 payload with padded rows is repacked to a tight layout.
///
/// Build an 'rgba8' payload whose 'stride' is greater than width * 4 and pad
/// each row with sentinel bytes; assert the decoded buffer drops the padding
/// (`data.len()` equals width * height * 4) and every pixel matches the source.
#[tokio::test]
async fn decode_rgba8_with_stride_repacked() {
    todo!(
        "build an ImagePayload with format 'rgba8', width 2, height 2, stride 12 (4 padding bytes per row) and base64 of the padded rows; \
         call decode_image and assert the buffer is tightly packed (data length 16) and all pixels match the source, with padding dropped"
    );
}

/// Bad base64 is a `ClientError::Image`.
///
/// Use 'data' that is not valid base64; assert `decode_image` returns
/// `Err(ClientError::Image)` and never panics.
#[tokio::test]
async fn decode_bad_base64_is_image_error() {
    todo!(
        "build an ImagePayload with format 'png' and 'data' 'not base64!!'; \
         assert decode_image returns Err(ClientError::Image) with a non-empty message"
    );
}

/// An unknown format is a `ClientError::Image`.
///
/// Build a payload whose 'format' is neither 'png' nor 'rgba8' (via the raw
/// JSON payload a server could send, or the proto type's unknown-format path);
/// assert `decode_image` returns `Err(ClientError::Image)`.
#[tokio::test]
async fn decode_unknown_format_is_image_error() {
    todo!(
        "build an ImagePayload whose format is neither 'png' nor 'rgba8'; \
         assert decode_image returns Err(ClientError::Image) rather than panicking or guessing a format"
    );
}

/// A truncated PNG is a `ClientError::Image`.
///
/// Take a valid PNG, cut 'data' short (header intact, body truncated); assert
/// `decode_image` returns `Err(ClientError::Image)` rather than panicking.
#[tokio::test]
async fn decode_truncated_png_is_image_error() {
    todo!(
        "build an ImagePayload with format 'png' whose base64 data is a valid PNG truncated mid-body; \
         assert decode_image returns Err(ClientError::Image)"
    );
}
