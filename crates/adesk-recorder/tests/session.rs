//! `RecordingSession` behaviour: frames are pushed over a channel and the final
//! `stop` reports the same stats and errors as the underlying recorder.

use std::time::Duration;

use adesk_core::ImageBuffer;
use adesk_recorder::{EncoderKind, RecorderConfig, RecorderError, RecordingSession};

#[test]
fn session_pushes_frames_then_stops() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.avi");
    let mut session =
        RecordingSession::start(RecorderConfig::new(&path).with_encoder(EncoderKind::Software))
            .unwrap();
    assert_eq!(session.encoder_name(), "mjpeg");
    assert!(session.stats().running);

    for i in 0..5u64 {
        session.push(ImageBuffer::new_rgba(32, 24), i * 33).unwrap();
    }

    let summary = session.stop().unwrap();
    assert_eq!(summary.frames, 5);
    assert_eq!(summary.path, path);
    assert_eq!(summary.encoder, "mjpeg");
    assert_eq!(summary.duration_ms, 4 * 33);
    assert!(path.exists());
    assert!(path.metadata().unwrap().len() > 0);

    let stats = session.stats();
    assert_eq!(stats.frames, 5);
    assert_eq!(stats.encoder, "mjpeg");
    assert!(!stats.running);

    // `stop` is idempotent and pushing after `stop` fails.
    let again = session.stop().unwrap();
    assert_eq!(again.frames, 5);
    let err = session
        .push(ImageBuffer::new_rgba(32, 24), 999)
        .unwrap_err();
    assert!(matches!(err, RecorderError::Backend(_)));
}

#[test]
fn session_is_finalized_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dropped.avi");
    {
        let session =
            RecordingSession::start(RecorderConfig::new(&path).with_encoder(EncoderKind::Software))
                .unwrap();
        session.push(ImageBuffer::new_rgba(16, 16), 0).unwrap();
        // Dropped without `stop`: the worker still finalizes the file.
    }
    assert!(path.exists());
    assert!(path.metadata().unwrap().len() > 0);
}

#[test]
fn session_propagates_worker_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failed.avi");
    let mut session =
        RecordingSession::start(RecorderConfig::new(&path).with_encoder(EncoderKind::Software))
            .unwrap();

    // A zero-sized frame is rejected by the software encoder on the worker.
    session.push(ImageBuffer::new_rgba(0, 0), 0).unwrap();

    // The worker will stop and close the channel; a later push must fail.
    let mut push_error = None;
    for _ in 0..200 {
        match session.push(ImageBuffer::new_rgba(4, 4), 1) {
            Ok(()) => std::thread::sleep(Duration::from_millis(5)),
            Err(err) => {
                push_error = Some(err);
                break;
            }
        }
    }
    let push_error = push_error.expect("a push after the worker failed must error");
    assert!(matches!(push_error, RecorderError::Backend(_)));

    // `stop` surfaces the precise recorder error.
    let err = session.stop().unwrap_err();
    assert!(
        matches!(err, RecorderError::Encode(_)),
        "expected Encode, got {err:?}"
    );
    assert_eq!(err.code(), adesk_core::ErrorCode::CaptureFailed);
}

#[test]
fn session_start_fails_on_unwritable_path() {
    let dir = tempfile::tempdir().unwrap();
    // A directory is not a writable file path, so `start` fails immediately.
    let err = RecordingSession::start(
        RecorderConfig::new(dir.path()).with_encoder(EncoderKind::Software),
    )
    .unwrap_err();
    assert!(matches!(err, RecorderError::Io(_)));
}
