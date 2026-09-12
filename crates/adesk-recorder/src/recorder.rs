//! The [`Recorder`] state machine and its [`RecorderConfig`].

use std::path::PathBuf;

use adesk_core::ImageBuffer;

use crate::detect;
use crate::encoder::Encoder;
use crate::error::{RecorderError, Result};
use crate::ffmpeg::{gpu_available, FfmpegEncoder};
use crate::mjpeg::MjpegEncoder;
use crate::EncoderKind;

/// Default frame rate (metadata only; the caller timestamps frames).
const DEFAULT_FPS: u32 = 30;

/// How a recording is created.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Output file path.
    pub path: PathBuf,
    /// Nominal frame rate. Metadata only — the recorder never sleeps; the
    /// caller is responsible for the real pacing of [`Recorder::push_frame`].
    pub fps: u32,
    /// Requested backend.
    pub encoder: EncoderKind,
    /// Optional limit on the number of frames that will be accepted.
    pub max_frames: Option<u64>,
}

impl RecorderConfig {
    /// A configuration for `path` with default settings (30 fps, [`EncoderKind::Auto`]).
    pub fn new(path: impl Into<PathBuf>) -> RecorderConfig {
        RecorderConfig {
            path: path.into(),
            fps: DEFAULT_FPS,
            encoder: EncoderKind::Auto,
            max_frames: None,
        }
    }

    /// Sets the nominal frame rate.
    pub fn with_fps(mut self, fps: u32) -> RecorderConfig {
        self.fps = fps;
        self
    }

    /// Sets the requested backend.
    pub fn with_encoder(mut self, encoder: EncoderKind) -> RecorderConfig {
        self.encoder = encoder;
        self
    }

    /// Caps the number of frames that will be accepted.
    pub fn with_max_frames(mut self, max_frames: u64) -> RecorderConfig {
        self.max_frames = Some(max_frames);
        self
    }
}

impl Default for RecorderConfig {
    fn default() -> RecorderConfig {
        RecorderConfig::new(PathBuf::new())
    }
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("path", &self.path)
            .field("encoder", &self.encoder.name())
            .field("frames", &self.frames)
            .field("dims", &self.dims)
            .finish_non_exhaustive()
    }
}

/// Summary of a completed recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSummary {
    /// Output file path.
    pub path: PathBuf,
    /// Resolved encoder backend name.
    pub encoder: String,
    /// Number of frames written.
    pub frames: u64,
    /// Span between the first and last frame timestamps (0 for < 2 frames).
    pub duration_ms: u64,
}

/// A screen recorder: pushes rendered frames into an encoder backend.
///
/// The recorder is purely a sink — it performs no scheduling and never sleeps.
/// Callers push frames as they are produced and pass the monotonic `ts_ms` of
/// each frame.
pub struct Recorder {
    path: PathBuf,
    encoder: Box<dyn Encoder>,
    max_frames: Option<u64>,
    frames: u64,
    dims: Option<(u32, u32)>,
    first_ts_ms: Option<u64>,
    last_ts_ms: Option<u64>,
    finished: bool,
}

impl Recorder {
    /// Creates a recorder, resolving the backend and opening the output.
    ///
    /// [`EncoderKind::Auto`] resolves to [`EncoderKind::Gpu`] when a hardware
    /// H.264 encoder is available and to [`EncoderKind::Software`] otherwise.
    /// An explicit [`EncoderKind::Gpu`] request without a hardware encoder is a
    /// structured [`RecorderError::Unsupported`].
    pub fn create(config: RecorderConfig) -> Result<Recorder> {
        if config.fps == 0 {
            return Err(RecorderError::Encode(
                "recording fps must be greater than zero".into(),
            ));
        }
        let encoder: Box<dyn Encoder> = match detect(config.encoder) {
            EncoderKind::Gpu => {
                if !gpu_available() {
                    return Err(RecorderError::Unsupported(
                        "no GPU H.264 encoder is available for this recording".into(),
                    ));
                }
                Box::new(FfmpegEncoder::create(&config.path, config.fps)?)
            }
            // `Software` (and the unreachable-in-practice `Auto`) use the
            // always-available pure-Rust backend.
            _ => Box::new(MjpegEncoder::create(&config.path, config.fps)?),
        };
        tracing::info!(
            path = %config.path.display(),
            encoder = encoder.name(),
            fps = config.fps,
            "recording started"
        );
        Ok(Recorder {
            path: config.path,
            encoder,
            max_frames: config.max_frames,
            frames: 0,
            dims: None,
            first_ts_ms: None,
            last_ts_ms: None,
            finished: false,
        })
    }

    /// The resolved encoder backend name.
    pub fn encoder_name(&self) -> &str {
        self.encoder.name()
    }

    /// The output path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Number of frames encoded so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Encodes one RGBA8 frame (honouring `stride`) stamped with `ts_ms`.
    ///
    /// The first frame fixes the recording dimensions; a later frame of a
    /// different size is rejected. Once [`RecorderConfig::max_frames`] is
    /// reached, further frames are rejected.
    pub fn push_frame(&mut self, frame: &ImageBuffer, ts_ms: u64) -> Result<()> {
        if self.finished {
            return Err(RecorderError::Backend("recorder already finalized".into()));
        }
        if let Some(max) = self.max_frames {
            if self.frames >= max {
                return Err(RecorderError::Encode(format!(
                    "frame limit of {max} reached"
                )));
            }
        }
        if let Some((w, h)) = self.dims {
            if (w, h) != (frame.width, frame.height) {
                return Err(RecorderError::Encode(format!(
                    "frame size changed from {w}x{h} to {}x{}",
                    frame.width, frame.height
                )));
            }
        }

        self.encoder.encode_frame(frame, ts_ms)?;
        if self.dims.is_none() {
            self.dims = Some((frame.width, frame.height));
            self.first_ts_ms = Some(ts_ms);
        }
        self.last_ts_ms = Some(ts_ms);
        self.frames += 1;
        Ok(())
    }

    /// Finalizes the recording and returns its summary.
    pub fn finish(mut self) -> Result<RecordingSummary> {
        self.finished = true;
        self.encoder.finish()?;
        let duration_ms = match (self.first_ts_ms, self.last_ts_ms) {
            (Some(first), Some(last)) => last.saturating_sub(first),
            _ => 0,
        };
        let summary = RecordingSummary {
            path: self.path.clone(),
            encoder: self.encoder.name().to_string(),
            frames: self.frames,
            duration_ms,
        };
        tracing::info!(
            path = %summary.path.display(),
            encoder = summary.encoder,
            frames = summary.frames,
            duration_ms = summary.duration_ms,
            "recording finished"
        );
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_builders() {
        let config = RecorderConfig::new("/tmp/out.avi");
        assert_eq!(config.path, PathBuf::from("/tmp/out.avi"));
        assert_eq!(config.fps, 30);
        assert_eq!(config.encoder, EncoderKind::Auto);
        assert_eq!(config.max_frames, None);

        let config = config
            .with_fps(15)
            .with_encoder(EncoderKind::Software)
            .with_max_frames(3);
        assert_eq!(config.fps, 15);
        assert_eq!(config.encoder, EncoderKind::Software);
        assert_eq!(config.max_frames, Some(3));

        let default = RecorderConfig::default();
        assert_eq!(default.fps, 30);
        assert_eq!(default.encoder, EncoderKind::Auto);
    }
}
