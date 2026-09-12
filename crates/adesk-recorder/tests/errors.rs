//! Error paths and AGP error-code mapping.

use adesk_core::{ErrorCode, ImageBuffer};
use adesk_recorder::{EncoderKind, Recorder, RecorderConfig, RecorderError};

#[test]
fn error_codes_match_the_agp_mapping() {
    assert_eq!(
        RecorderError::Unsupported("x".into()).code(),
        ErrorCode::NotSupported
    );
    assert_eq!(
        RecorderError::Encode("x".into()).code(),
        ErrorCode::CaptureFailed
    );
    assert_eq!(
        RecorderError::Mux("x".into()).code(),
        ErrorCode::CaptureFailed
    );
    assert_eq!(
        RecorderError::Backend("x".into()).code(),
        ErrorCode::RenderFailed
    );
    let io: RecorderError = std::io::Error::other("boom").into();
    assert_eq!(io.code(), ErrorCode::Internal);

    // The umbrella conversion preserves the code and the message.
    let core: adesk_core::Error = RecorderError::Unsupported("nope".into()).into();
    assert_eq!(core.code, ErrorCode::NotSupported);
    assert!(core.message.contains("nope"));
}

#[test]
fn unwritable_path_fails_at_create() {
    let dir = tempfile::tempdir().unwrap();
    // A directory is not a writable file path.
    let err = Recorder::create(RecorderConfig::new(dir.path()).with_encoder(EncoderKind::Software))
        .unwrap_err();
    assert!(matches!(err, RecorderError::Io(_)), "got {err:?}");
    assert_eq!(err.code(), ErrorCode::Internal);

    // A path whose parent does not exist also fails at create.
    let missing = dir.path().join("no-such-dir").join("out.avi");
    let err = Recorder::create(RecorderConfig::new(missing)).unwrap_err();
    assert!(matches!(err, RecorderError::Io(_)), "got {err:?}");
}

#[test]
fn zero_fps_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        Recorder::create(RecorderConfig::new(dir.path().join("x.avi")).with_fps(0)).unwrap_err();
    assert!(matches!(err, RecorderError::Encode(_)), "got {err:?}");
}

#[test]
fn max_frames_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capped.avi");
    let mut recorder = Recorder::create(
        RecorderConfig::new(&path)
            .with_encoder(EncoderKind::Software)
            .with_max_frames(2),
    )
    .unwrap();
    recorder
        .push_frame(&ImageBuffer::new_rgba(8, 8), 0)
        .unwrap();
    recorder
        .push_frame(&ImageBuffer::new_rgba(8, 8), 10)
        .unwrap();
    let err = recorder
        .push_frame(&ImageBuffer::new_rgba(8, 8), 20)
        .unwrap_err();
    assert!(matches!(err, RecorderError::Encode(_)), "got {err:?}");
    assert_eq!(recorder.frames(), 2);
    assert_eq!(recorder.finish().unwrap().frames, 2);
}

#[test]
fn frame_size_change_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resized.avi");
    let mut recorder =
        Recorder::create(RecorderConfig::new(&path).with_encoder(EncoderKind::Software)).unwrap();
    recorder
        .push_frame(&ImageBuffer::new_rgba(16, 16), 0)
        .unwrap();
    let err = recorder
        .push_frame(&ImageBuffer::new_rgba(32, 16), 1)
        .unwrap_err();
    assert!(matches!(err, RecorderError::Encode(_)), "got {err:?}");
    // The recording stays usable at its original size.
    recorder
        .push_frame(&ImageBuffer::new_rgba(16, 16), 2)
        .unwrap();
    assert_eq!(recorder.finish().unwrap().frames, 2);
}

#[test]
fn pushing_after_finish_is_impossible_by_construction() {
    // `finish` consumes the recorder, so the only way to push afterwards is via
    // a fresh recorder; this pins the consume-by-value contract.
    let dir = tempfile::tempdir().unwrap();
    let recorder = Recorder::create(
        RecorderConfig::new(dir.path().join("done.avi")).with_encoder(EncoderKind::Software),
    )
    .unwrap();
    let summary = recorder.finish().unwrap();
    assert_eq!(summary.frames, 0);
}
