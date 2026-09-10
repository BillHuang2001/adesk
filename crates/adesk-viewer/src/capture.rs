//! Frame capture helpers: VAP wire images to PNG files on disk.
//!
//! A frame carries an AGP `ImagePayload` (`png` or `rgba8`); the headless
//! `adesk-viewer` binary and the tests use these helpers to persist frames. The
//! `image` crate is the only encoder. Every buffer is validated before it reaches
//! the encoder, because the PNG encoder panics on a length/dimension mismatch and
//! no capture path may panic (`docs/viewer.md` §3).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use adesk_core::ImageBuffer;
use adesk_proto::{ImageFormat, ImagePayload};
use adesk_viewer_proto::{ViewerFrame, ViewerProtoError};

use crate::error::{Result, ViewerError};

/// Decodes a frame's image and writes it to `path` as a PNG.
///
/// A `png` payload is written verbatim (it already is PNG bytes); an `rgba8`
/// payload is re-packed tightly and re-encoded as PNG. The parent directory must
/// exist.
///
/// # Errors
///
/// Returns [`ViewerError::Io`] if the file cannot be written, and
/// [`ViewerError::Protocol`] when the payload is malformed (invalid base64, a
/// stride shorter than one row, a byte count that does not match the dimensions)
/// or cannot be encoded.
pub fn save_frame_png(frame: &ImagePayload, path: &Path) -> Result<()> {
    let data = frame
        .decode_data()
        .map_err(|error| invalid_payload(format!("invalid base64 image data: {error}")))?;
    match frame.format {
        ImageFormat::Png => {
            std::fs::write(path, &data)?;
            Ok(())
        }
        ImageFormat::Rgba8 => {
            let rgba = tight_rgba8(frame.width, frame.height, frame.stride, Cow::Owned(data))?;
            encode_rgba8_png(&rgba, frame.width, frame.height, path)
        }
    }
}

/// Writes an RGBA8 [`ImageBuffer`] to `path` as a PNG.
///
/// Buffer rows may be padded; the padding is stripped before encoding.
///
/// # Errors
///
/// Returns [`ViewerError::Io`] if the file cannot be written, and
/// [`ViewerError::Protocol`] when the buffer is inconsistent (a stride shorter
/// than one row, a byte count that does not match the dimensions, an empty image)
/// or cannot be encoded.
pub fn write_rgba8(buffer: &ImageBuffer, path: &Path) -> Result<()> {
    let rgba = tight_rgba8(
        buffer.width,
        buffer.height,
        Some(buffer.stride),
        Cow::Borrowed(&buffer.data),
    )?;
    encode_rgba8_png(&rgba, buffer.width, buffer.height, path)
}

/// Writes successive frames into a directory as `frame-<seq>.png`.
///
/// The name uses the frame's own [`ViewerFrame::seq`], zero-padded to eight
/// digits, so a capture directory is ordered by frame rather than by wall clock.
#[derive(Debug, Clone)]
pub struct FrameWriter {
    /// Directory frames are written into.
    directory: PathBuf,
}

impl FrameWriter {
    /// Creates a writer that writes into `dir`.
    ///
    /// The directory is **not** created; it must exist by the time
    /// [`FrameWriter::write`] is called.
    pub fn new(dir: impl Into<PathBuf>) -> FrameWriter {
        FrameWriter {
            directory: dir.into(),
        }
    }

    /// The directory frames are written into.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Writes `frame` as `frame-<seq:08>.png` and returns the path written.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`save_frame_png`].
    pub fn write(&self, frame: &ViewerFrame) -> Result<PathBuf> {
        let path = self.directory.join(format!("frame-{:08}.png", frame.seq));
        save_frame_png(&frame.image, &path)?;
        Ok(path)
    }
}

/// Re-packs an RGBA8 image into tightly packed rows (`width * 4` bytes each).
///
/// `data` must describe exactly `stride * height` bytes with `stride` at least one
/// row wide; the absent stride defaults to the tight row length. Padding bytes are
/// dropped. Every check is done in `u64` and a buffer that does not match is
/// reported as a message, never a panic.
///
/// When the stride already equals the tight row length the input is returned
/// unchanged, so an owned buffer is never copied (the `--follow` hot path).
fn tight_rgba8<'a>(
    width: u32,
    height: u32,
    stride: Option<u32>,
    data: Cow<'a, [u8]>,
) -> Result<Cow<'a, [u8]>> {
    let row_bytes = u64::from(width) * 4;
    if row_bytes > u64::from(u32::MAX) {
        return Err(invalid_payload(format!(
            "RGBA width {width} exceeds the maximum stride"
        )));
    }
    // Safe: the check above bounds `row_bytes` by `u32::MAX`.
    let stride = u64::from(stride.unwrap_or(row_bytes as u32));
    if stride < row_bytes {
        return Err(invalid_payload(format!(
            "RGBA stride {stride} is shorter than the {row_bytes}-byte row width of {width} pixels"
        )));
    }
    let expected = stride.checked_mul(u64::from(height)).ok_or_else(|| {
        invalid_payload(format!(
            "RGBA {width}x{height} at stride {stride} overflows the byte count"
        ))
    })?;
    if expected != data.len() as u64 {
        return Err(invalid_payload(format!(
            "RGBA data length {} does not match {width}x{height} at stride {stride} \
             (expected {expected} bytes)",
            data.len()
        )));
    }
    if stride == row_bytes {
        return Ok(data);
    }
    // `stride > row_bytes >= 0` and `stride` divides `data.len()`, so `chunks_exact`
    // never sees a zero chunk size and covers every row.
    let stride = stride as usize;
    let row_bytes = row_bytes as usize;
    let mut tight = Vec::with_capacity(row_bytes * height as usize);
    for row in data.chunks_exact(stride) {
        tight.extend_from_slice(&row[..row_bytes]);
    }
    Ok(Cow::Owned(tight))
}

/// Encodes tightly packed RGBA8 pixels to a PNG file at `path`.
fn encode_rgba8_png(rgba: &[u8], width: u32, height: u32, path: &Path) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(invalid_payload(format!(
            "cannot encode an empty {width}x{height} image"
        )));
    }
    let expected = u64::from(width) * u64::from(height) * 4;
    if rgba.len() as u64 != expected {
        return Err(invalid_payload(format!(
            "RGBA data length {} does not match {width}x{height} (expected {expected} bytes)",
            rgba.len()
        )));
    }
    image::save_buffer_with_format(
        path,
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| match error {
        image::ImageError::IoError(io) => ViewerError::Io(io),
        other => invalid_payload(format!("PNG encode failed: {other}")),
    })
}

/// Builds the error for a malformed or unencodable image payload.
///
/// `capture` has no dedicated image variant; a payload that does not match its
/// declared shape is a protocol-level problem, so it is reported as
/// [`ViewerProtoError::InvalidParams`] (never a panic).
fn invalid_payload(message: impl Into<String>) -> ViewerError {
    ViewerError::Protocol(ViewerProtoError::InvalidParams {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_viewer_proto::CursorState;

    /// A tightly packed RGBA8 payload whose bytes are `0..w*h*4` (mod 256).
    fn rgba_payload(width: u32, height: u32) -> ImagePayload {
        let data: Vec<u8> = (0..(width * height * 4)).map(|byte| byte as u8).collect();
        ImagePayload::from_rgba8(width, height, &data, 1.0).expect("valid rgba8 payload")
    }

    fn frame(seq: u64, image: ImagePayload) -> ViewerFrame {
        ViewerFrame {
            seq,
            ts_ms: 7,
            image,
            cursor: CursorState::hidden(),
            active_window_id: None,
        }
    }

    #[test]
    fn writes_rgba8_payload_as_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.png");
        save_frame_png(&rgba_payload(2, 2), &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn writes_png_payload_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let encoded = dir.path().join("encoded.png");
        save_frame_png(&rgba_payload(2, 2), &encoded).unwrap();
        let png = std::fs::read(&encoded).unwrap();

        let path = dir.path().join("copy.png");
        save_frame_png(&ImagePayload::from_png(2, 2, &png, 1.0), &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), png);
    }

    #[test]
    fn writes_an_image_buffer_as_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("buffer.png");
        let buffer = ImageBuffer::from_rgba(2, 1, vec![0, 0, 255, 255, 0, 255, 0, 255]).unwrap();
        write_rgba8(&buffer, &path).unwrap();
        assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn write_rgba8_strips_row_padding() {
        // A 2x2 image with 12-byte rows (4 padding bytes per row).
        let mut data = vec![0u8; 24];
        data[3] = 255;
        data[15] = 255;
        let buffer = ImageBuffer {
            width: 2,
            height: 2,
            stride: 12,
            format: adesk_core::PixelFormat::Rgba8,
            data,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padded.png");
        write_rgba8(&buffer, &path).unwrap();
        assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn mismatched_rgba8_length_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        // "AQID" decodes to 3 bytes, far short of the 2x2 RGBA size.
        let payload = ImagePayload {
            width: 2,
            height: 2,
            format: ImageFormat::Rgba8,
            stride: Some(8),
            data: "AQID".to_owned(),
            scale: 1.0,
        };
        assert!(matches!(
            save_frame_png(&payload, &path).unwrap_err(),
            ViewerError::Protocol(_)
        ));
        assert!(!path.exists(), "a rejected payload writes no file");
    }

    #[test]
    fn invalid_base64_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        let payload = ImagePayload {
            width: 2,
            height: 2,
            format: ImageFormat::Png,
            stride: None,
            data: "not base64!".to_owned(),
            scale: 1.0,
        };
        assert!(matches!(
            save_frame_png(&payload, &path).unwrap_err(),
            ViewerError::Protocol(_)
        ));
    }

    #[test]
    fn empty_image_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.png");
        let buffer = ImageBuffer::from_rgba(0, 0, Vec::new()).unwrap();
        assert!(matches!(
            write_rgba8(&buffer, &path).unwrap_err(),
            ViewerError::Protocol(_)
        ));
    }

    #[test]
    fn frame_writer_names_frames_by_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FrameWriter::new(dir.path());
        assert_eq!(writer.directory(), dir.path());

        let path = writer.write(&frame(42, rgba_payload(1, 1))).unwrap();
        assert_eq!(path.file_name().unwrap(), "frame-00000042.png");
        assert!(path.exists());
    }
}
