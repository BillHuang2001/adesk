//! The object-safe [`Encoder`] trait implemented by every backend.
//!
//! The recorder is generic over this trait, so the software Motion-JPEG backend
//! ([`crate::mjpeg`]) and the GPU-accelerated `ffmpeg` backend
//! ([`crate::ffmpeg`]) are interchangeable at runtime.

use adesk_core::ImageBuffer;

use crate::error::Result;

/// A single-stream frame encoder that writes encoded video to a container.
///
/// Implementations are object-safe, so the [`Recorder`](crate::Recorder) holds a
/// `Box<dyn Encoder>`. Frames always arrive as RGBA8 [`ImageBuffer`]s whose rows
/// may be padded (`stride >= width * 4`); an implementation must honour `stride`.
pub trait Encoder: Send {
    /// Human-readable backend name (e.g. `"mjpeg"` or `"ffmpeg/h264_vaapi"`).
    fn name(&self) -> &str;

    /// Encodes one RGBA8 frame stamped with the monotonic `ts_ms`.
    ///
    /// The timestamp is metadata only (the caller paces frames); the backend
    /// must never sleep.
    fn encode_frame(&mut self, frame: &ImageBuffer, ts_ms: u64) -> Result<()>;

    /// Number of frames encoded so far.
    fn frames(&self) -> u64;

    /// Finalizes the output: patches container sizes, flushes the sink and
    /// (for child-process backends) waits for the encoder to exit.
    fn finish(&mut self) -> Result<()>;
}
