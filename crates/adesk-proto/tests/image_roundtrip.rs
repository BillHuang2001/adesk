//! Image layer acceptance tests (`docs/protocol.md` §4).
//!
//! `image_payload_base64_helpers` mirrors the frozen acceptance test in
//! `tests/codec.rs`; the remaining tests pin the edge cases of the same four
//! bodies.

use adesk_core::PixelFormat;
use adesk_proto::{ImageFormat, ImagePayload, ProtoError};
use serde_json::json;

/// Verbatim mirror of the frozen acceptance test `image_payload_base64_helpers`.
#[test]
fn image_payload_base64_helpers() {
    let rgba: Vec<u8> = (0..32).collect(); // 4x2 pixels
    let payload = ImagePayload::from_rgba8(4, 2, &rgba, 1.0).unwrap();
    assert_eq!(payload.format, ImageFormat::Rgba8);
    assert_eq!(payload.stride, Some(16));
    assert_eq!(payload.decode_data().unwrap(), rgba);

    let buffer = payload.to_rgba8_buffer().unwrap();
    assert_eq!(buffer.width, 4);
    assert_eq!(buffer.pixel(1, 0), Some([4, 5, 6, 7]));

    let png = ImagePayload::from_png(4, 2, &[0x89, b'P', b'N', b'G'], 1.0);
    assert_eq!(png.format, ImageFormat::Png);
    assert_eq!(png.stride, None);
    assert_eq!(png.decode_data().unwrap(), vec![0x89, b'P', b'N', b'G']);
    assert!(png.to_rgba8_buffer().is_err());

    // Wrong length is rejected, never silently truncated.
    assert!(ImagePayload::from_rgba8(4, 2, &rgba[..10], 1.0).is_err());
}

#[test]
fn from_rgba8_uses_standard_base64_with_padding() {
    // 4x1 pixels = 16 bytes → 24 base64 chars with one '=' of padding.
    let rgba: Vec<u8> = (0..16).collect();
    let payload = ImagePayload::from_rgba8(4, 1, &rgba, 1.0).unwrap();
    assert_eq!(payload.data.len(), 24);
    assert!(payload.data.ends_with('='));
    assert_eq!(payload.stride, Some(16));
    assert_eq!(payload.decode_data().unwrap(), rgba);

    // 1x1 pixel = 4 bytes → 8 chars, "AAECAw==".
    let payload = ImagePayload::from_rgba8(1, 1, &[0, 1, 2, 3], 1.0).unwrap();
    assert_eq!(payload.data, "AAECAw==");
}

#[test]
fn from_rgba8_empty_image_round_trips() {
    let payload = ImagePayload::from_rgba8(0, 0, &[], 1.0).unwrap();
    assert_eq!(payload.width, 0);
    assert_eq!(payload.height, 0);
    assert_eq!(payload.stride, Some(0));
    assert_eq!(payload.format, ImageFormat::Rgba8);
    assert_eq!(payload.data, "");
    assert_eq!(payload.decode_data().unwrap(), Vec::<u8>::new());

    let buffer = payload.to_rgba8_buffer().unwrap();
    assert_eq!(buffer.width, 0);
    assert_eq!(buffer.height, 0);
    assert_eq!(buffer.stride, 0);
    assert!(buffer.data.is_empty());
    assert_eq!(buffer.pixel(0, 0), None);
}

#[test]
fn from_rgba8_rejects_oversized_dimensions_without_panicking() {
    // width * 4 overflows u32.
    let err = ImagePayload::from_rgba8(u32::MAX, 0, &[], 1.0).unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)));
    assert!(err.to_string().contains("stride"), "{err}");

    // Same rejection for a non-empty payload: never silently truncated.
    let err = ImagePayload::from_rgba8(u32::MAX, 1, &[0; 4], 1.0).unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)));

    // Huge but stride-representable dimensions only fail the length check.
    let err = ImagePayload::from_rgba8(u32::MAX / 4, u32::MAX, &[], 1.0).unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)));
    assert!(err.to_string().contains("does not match"), "{err}");
}

#[test]
fn from_rgba8_rejects_every_wrong_length() {
    for len in [0usize, 15, 17, 31, 33] {
        let data = vec![0u8; len];
        assert!(
            ImagePayload::from_rgba8(4, 2, &data, 1.0).is_err(),
            "{len} bytes must be rejected for 4x2"
        );
    }
    // Height 0 accepts only an empty payload.
    assert!(ImagePayload::from_rgba8(4, 0, &[], 1.0).is_ok());
    assert!(ImagePayload::from_rgba8(4, 0, &[0; 16], 1.0).is_err());
}

#[test]
fn scale_is_preserved_by_both_constructors() {
    let rgba = ImagePayload::from_rgba8(1, 1, &[1, 2, 3, 4], 0.5).unwrap();
    assert_eq!(rgba.scale, 0.5);
    let png = ImagePayload::from_png(1, 1, &[0x89], 0.25);
    assert_eq!(png.scale, 0.25);
}

#[test]
fn from_png_keeps_bytes_and_has_no_stride() {
    let bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let payload = ImagePayload::from_png(2, 2, &bytes, 1.0);
    assert_eq!(payload.format, ImageFormat::Png);
    assert_eq!(payload.stride, None);
    assert_eq!(payload.width, 2);
    assert_eq!(payload.height, 2);
    assert_eq!(payload.decode_data().unwrap(), bytes);

    // Empty PNG payload is representable (the renderer decides what is valid).
    let empty = ImagePayload::from_png(0, 0, &[], 1.0);
    assert_eq!(empty.data, "");
    assert_eq!(empty.decode_data().unwrap(), Vec::<u8>::new());
}

#[test]
fn decode_data_reports_invalid_base64() {
    let mut payload = ImagePayload::from_rgba8(1, 1, &[1, 2, 3, 4], 1.0).unwrap();
    payload.data = "not base64!".to_owned();
    let err = payload.decode_data().unwrap_err();
    assert!(matches!(err, ProtoError::Base64(_)), "{err:?}");
    assert!(payload.to_rgba8_buffer().is_err());

    // Truncated base64 (missing padding) is rejected too.
    payload.data = "AAECAw".to_owned();
    assert!(matches!(
        payload.decode_data().unwrap_err(),
        ProtoError::Base64(_)
    ));
}

#[test]
fn to_rgba8_buffer_rejects_non_rgba8_format() {
    let png = ImagePayload::from_png(1, 1, &[0x89], 1.0);
    let err = png.to_rgba8_buffer().unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("Png"), "{err}");
}

#[test]
fn to_rgba8_buffer_rejects_padded_stride() {
    // 2 pixels wide, 1 row, 12 bytes of data → stride 12 is padded, not tight.
    let payload = ImagePayload {
        width: 2,
        height: 1,
        format: ImageFormat::Rgba8,
        stride: Some(2 * 4 + 1),
        data: ImagePayload::from_rgba8(3, 1, &[0; 12], 1.0).unwrap().data,
        scale: 1.0,
    };
    let err = payload.to_rgba8_buffer().unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("tightly packed"), "{err}");

    // A stride larger than width * 4 is rejected as well.
    let padded = ImagePayload {
        stride: Some(2 * 4 + 8),
        ..payload
    };
    assert!(padded.to_rgba8_buffer().is_err());
}

#[test]
fn to_rgba8_buffer_rejects_missing_stride() {
    let payload = ImagePayload {
        width: 1,
        height: 1,
        format: ImageFormat::Rgba8,
        stride: None,
        data: ImagePayload::from_rgba8(1, 1, &[1, 2, 3, 4], 1.0)
            .unwrap()
            .data,
        scale: 1.0,
    };
    let err = payload.to_rgba8_buffer().unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("missing its stride"), "{err}");
}

#[test]
fn to_rgba8_buffer_rejects_length_mismatch() {
    // Valid base64, wrong byte count for the declared dimensions.
    let payload = ImagePayload {
        width: 4,
        height: 2,
        format: ImageFormat::Rgba8,
        stride: Some(16),
        data: ImagePayload::from_rgba8(2, 1, &[0; 8], 1.0).unwrap().data,
        scale: 1.0,
    };
    let err = payload.to_rgba8_buffer().unwrap_err();
    assert!(matches!(err, ProtoError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("does not match 4x2"), "{err}");
}

#[test]
fn to_rgba8_buffer_is_tightly_packed_rgba8() {
    let rgba: Vec<u8> = (0..32).collect();
    let buffer = ImagePayload::from_rgba8(4, 2, &rgba, 1.0)
        .unwrap()
        .to_rgba8_buffer()
        .unwrap();
    assert_eq!(buffer.width, 4);
    assert_eq!(buffer.height, 2);
    assert_eq!(buffer.stride, 16);
    assert_eq!(buffer.format, PixelFormat::Rgba8);
    assert_eq!(buffer.data, rgba);
    assert_eq!(buffer.pixel(0, 0), Some([0, 1, 2, 3]));
    assert_eq!(buffer.pixel(3, 1), Some([28, 29, 30, 31]));
    assert_eq!(buffer.pixel(4, 1), None);
}

#[test]
fn rgba8_wire_shape_matches_protocol_example() {
    let rgba: Vec<u8> = vec![0, 0, 0, 0];
    let payload = ImagePayload::from_rgba8(1, 1, &rgba, 1.0).unwrap();
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        json!({
            "width": 1,
            "height": 1,
            "format": "rgba8",
            "stride": 4,
            "data": "AAAAAA==",
            "scale": 1.0
        })
    );
    assert_eq!(
        serde_json::from_value::<ImagePayload>(serde_json::to_value(&payload).unwrap()).unwrap(),
        payload
    );
}

#[test]
fn png_wire_shape_has_null_stride() {
    let payload = ImagePayload::from_png(2, 1, &[0x89, b'P'], 0.5);
    let value = serde_json::to_value(&payload).unwrap();
    assert_eq!(
        value,
        json!({
            "width": 2,
            "height": 1,
            "format": "png",
            "stride": null,
            "data": "iVA=",
            "scale": 0.5
        })
    );
    assert_eq!(
        serde_json::from_value::<ImagePayload>(value).unwrap(),
        payload
    );

    // Absent stride/scale fall back to the §4 defaults.
    let decoded: ImagePayload = serde_json::from_value(json!({
        "width": 4,
        "height": 2,
        "format": "png",
        "data": "AAAA"
    }))
    .unwrap();
    assert_eq!(decoded.stride, None);
    assert_eq!(decoded.scale, 1.0);
}
