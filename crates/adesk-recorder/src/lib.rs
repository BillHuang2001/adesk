//! `adesk-recorder` — screen recording for the ADesk runtime.
//!
//! Turns the runtime's rendered desktop frames ([`adesk_core::ImageBuffer`],
//! RGBA8 with possibly padded rows) plus a monotonic `ts_ms` into an encoded
//! video file. The crate is deliberately free of compositor, protocol and async
//! coupling: `adesk-server` owns capture (pulling frames from the runtime) and
//! drives a [`Recorder`] or a [`RecordingSession`] here.
//!
//! # Backends
//!
//! - **[`EncoderKind::Software`]** (always available, the default fallback): a
//!   pure-Rust Motion-JPEG encoder muxed into a RIFF/AVI `'MJPG'` file. It needs
//!   no GPU, display, network access or external tool.
//! - **[`EncoderKind::Gpu`]** (used when available): an external `ffmpeg`
//!   process driving a hardware H.264 encoder, writing an `.mp4`; it falls back
//!   to `libx264` when a hardware encoder cannot start.
//!
//! [`detect`] resolves a request against the current platform, [`gpu_available`]
//! probes for a hardware H.264 encoder, and [`suggest_extension`] reports the
//! container extension a given request will produce.
//!
//! # Example
//!
//! ```
//! use adesk_core::ImageBuffer;
//! use adesk_recorder::{EncoderKind, Recorder, RecorderConfig};
//!
//! # fn main() -> adesk_recorder::Result<()> {
//! let mut recorder = Recorder::create(RecorderConfig::new("/tmp/demo.avi").with_encoder(EncoderKind::Software))?;
//! let frame = ImageBuffer::new_rgba(64, 48);
//! recorder.push_frame(&frame, 0)?;
//! recorder.push_frame(&frame, 33)?;
//! let summary = recorder.finish()?;
//! assert_eq!(summary.frames, 2);
//! # std::fs::remove_file(&summary.path).ok();
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod avi;
pub mod encoder;
pub mod error;
pub mod ffmpeg;
pub mod mjpeg;
pub mod recorder;
pub mod session;

pub use encoder::Encoder;
pub use error::{RecorderError, Result};
pub use ffmpeg::{ffmpeg_available, gpu_available, FfmpegEncoder};
pub use mjpeg::{MjpegEncoder, DEFAULT_JPEG_QUALITY};
pub use recorder::{Recorder, RecorderConfig, RecordingSummary};
pub use session::{RecordingSession, RecordingStats};

/// Which encoder backend a recording should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderKind {
    /// Use the GPU-accelerated backend when a hardware H.264 encoder is
    /// available, otherwise the software backend.
    #[default]
    Auto,
    /// Always use the pure-Rust software Motion-JPEG/AVI backend.
    Software,
    /// Require the GPU-accelerated `ffmpeg` backend.
    Gpu,
}

/// Resolves a requested [`EncoderKind`] against the current platform.
///
/// [`EncoderKind::Auto`] resolves to [`EncoderKind::Gpu`] when a hardware H.264
/// encoder is available and to [`EncoderKind::Software`] otherwise. Explicit
/// requests are returned unchanged; [`Recorder::create`] then rejects an
/// [`EncoderKind::Gpu`] request with [`RecorderError::Unsupported`] when no
/// hardware encoder is present.
pub fn detect(kind: EncoderKind) -> EncoderKind {
    match kind {
        EncoderKind::Auto => {
            if gpu_available() {
                EncoderKind::Gpu
            } else {
                EncoderKind::Software
            }
        }
        other => other,
    }
}

/// The file extension a recording with `kind` will produce.
///
/// [`EncoderKind::Gpu`] writes an `.mp4`; the software backend writes an `.avi`.
pub fn suggest_extension(kind: EncoderKind) -> &'static str {
    match detect(kind) {
        EncoderKind::Gpu => ".mp4",
        _ => ".avi",
    }
}
