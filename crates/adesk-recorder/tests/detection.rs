//! Backend detection: `detect`/`gpu_available` never panic, `Auto` resolves to
//! `Software` in a GPU-less environment, and an explicit `Gpu` request without a
//! hardware encoder is a structured `Unsupported` error.

use adesk_core::ErrorCode;
use adesk_recorder::{
    detect, gpu_available, suggest_extension, EncoderKind, Recorder, RecorderConfig, RecorderError,
};

#[test]
fn detect_never_panics_and_resolves_auto() {
    // Probing must never panic, regardless of whether ffmpeg is installed.
    let hardware = gpu_available();
    assert_eq!(gpu_available(), hardware, "the probe result is cached");

    let auto = detect(EncoderKind::Auto);
    if hardware {
        assert_eq!(auto, EncoderKind::Gpu);
    } else {
        assert_eq!(auto, EncoderKind::Software);
    }

    // Explicit requests pass through unchanged.
    assert_eq!(detect(EncoderKind::Software), EncoderKind::Software);
    assert_eq!(detect(EncoderKind::Gpu), EncoderKind::Gpu);
}

#[test]
fn auto_is_software_without_hardware() {
    if gpu_available() {
        // A hardware encoder is present; the environment-specific assertion
        // below does not apply.
        return;
    }
    assert_eq!(detect(EncoderKind::Auto), EncoderKind::Software);
    assert_eq!(suggest_extension(EncoderKind::Auto), ".avi");
}

#[test]
fn extensions_match_backend() {
    assert_eq!(suggest_extension(EncoderKind::Gpu), ".mp4");
    assert_eq!(suggest_extension(EncoderKind::Software), ".avi");
    let expected_auto = if gpu_available() { ".mp4" } else { ".avi" };
    assert_eq!(suggest_extension(EncoderKind::Auto), expected_auto);
}

#[test]
fn explicit_gpu_without_hardware_is_unsupported() {
    if gpu_available() {
        // A hardware encoder is present, so `Gpu` is expected to succeed; skip.
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = RecorderConfig::new(dir.path().join("out.mp4")).with_encoder(EncoderKind::Gpu);
    let err = Recorder::create(config).unwrap_err();
    assert!(
        matches!(err, RecorderError::Unsupported(_)),
        "expected Unsupported, got {err:?}"
    );
    assert_eq!(err.code(), ErrorCode::NotSupported);
}

#[test]
fn software_backend_always_creates_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("soft.avi");
    let recorder =
        Recorder::create(RecorderConfig::new(&path).with_encoder(EncoderKind::Software)).unwrap();
    assert_eq!(recorder.encoder_name(), "mjpeg");
    recorder.finish().unwrap();
    assert!(path.exists());
}
