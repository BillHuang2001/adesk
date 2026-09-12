//! Pure-Rust Motion-JPEG software encoder.
//!
//! Each frame is encoded as an independent baseline JPEG with `jpeg-encoder`
//! and muxed into a RIFF/AVI `'MJPG'` stream by [`AviWriter`]. This backend
//! needs no GPU, display, network access or external process, so it always
//! works and is the default fallback.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use adesk_core::ImageBuffer;

use crate::avi::AviWriter;
use crate::encoder::Encoder;
use crate::error::{RecorderError, Result};

/// JPEG quality (1-100) used for every frame of a Motion-JPEG recording.
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

/// The software Motion-JPEG / AVI encoder.
///
/// The AVI header is written lazily on the first frame, because the frame
/// dimensions are not known when the output file is created.
pub struct MjpegEncoder {
    path: PathBuf,
    fps: u32,
    quality: u8,
    /// Holds the output file until the AVI header is written.
    writer: Option<BufWriter<File>>,
    avi: Option<AviWriter<BufWriter<File>>>,
    width: u32,
    height: u32,
    /// Scratch buffer for the RGB conversion of the current frame.
    rgb: Vec<u8>,
    /// Scratch buffer for the current JPEG frame.
    jpeg: Vec<u8>,
    frames: u64,
    finished: bool,
}

impl MjpegEncoder {
    /// Creates (or truncates) `path` and prepares a Motion-JPEG/AVI writer.
    ///
    /// The file is created eagerly so an unwritable path is reported here, but
    /// the AVI header is deferred until the first
    /// [`encode_frame`](Encoder::encode_frame) call.
    pub fn create(path: impl Into<PathBuf>, fps: u32) -> Result<MjpegEncoder> {
        let path = path.into();
        let file = File::create(&path)?;
        Ok(MjpegEncoder {
            path,
            fps,
            quality: DEFAULT_JPEG_QUALITY,
            writer: Some(BufWriter::new(file)),
            avi: None,
            width: 0,
            height: 0,
            rgb: Vec::new(),
            jpeg: Vec::new(),
            frames: 0,
            finished: false,
        })
    }

    /// The output path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for MjpegEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MjpegEncoder")
            .field("path", &self.path)
            .field("fps", &self.fps)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl Encoder for MjpegEncoder {
    fn name(&self) -> &str {
        "mjpeg"
    }

    fn frames(&self) -> u64 {
        self.frames
    }

    fn encode_frame(&mut self, frame: &ImageBuffer, _ts_ms: u64) -> Result<()> {
        if self.finished {
            return Err(RecorderError::Backend(
                "mjpeg encoder already finalized".into(),
            ));
        }
        let width = u16::try_from(frame.width).map_err(|_| {
            RecorderError::Unsupported(format!(
                "frame width {} exceeds the JPEG limit",
                frame.width
            ))
        })?;
        let height = u16::try_from(frame.height).map_err(|_| {
            RecorderError::Unsupported(format!(
                "frame height {} exceeds the JPEG limit",
                frame.height
            ))
        })?;
        if width == 0 || height == 0 {
            return Err(RecorderError::Encode(
                "cannot encode a zero-sized frame".into(),
            ));
        }

        if self.avi.is_none() {
            let writer = self
                .writer
                .take()
                .ok_or_else(|| RecorderError::Backend("mjpeg output already closed".into()))?;
            self.avi = Some(AviWriter::new(writer, frame.width, frame.height, self.fps)?);
            self.width = frame.width;
            self.height = frame.height;
        } else if (self.width, self.height) != (frame.width, frame.height) {
            return Err(RecorderError::Encode(format!(
                "frame size changed from {}x{} to {}x{}",
                self.width, self.height, frame.width, frame.height
            )));
        }

        rgba_to_rgb(frame, &mut self.rgb)?;
        self.jpeg.clear();
        {
            let encoder = jpeg_encoder::Encoder::new(&mut self.jpeg, self.quality);
            encoder
                .encode(&self.rgb, width, height, jpeg_encoder::ColorType::Rgb)
                .map_err(|err| RecorderError::Encode(err.to_string()))?;
        }

        let avi = self
            .avi
            .as_mut()
            .ok_or_else(|| RecorderError::Backend("mjpeg output missing".into()))?;
        avi.write_frame(&self.jpeg)?;
        self.frames += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        match self.avi.as_mut() {
            Some(avi) => avi.finish()?,
            // No frames were recorded: leave a valid (empty) file behind.
            None => {
                if let Some(writer) = self.writer.as_mut() {
                    writer.flush()?;
                }
            }
        }
        Ok(())
    }
}

/// Converts an RGBA8 frame (honouring `stride`) into tightly packed RGB.
fn rgba_to_rgb(frame: &ImageBuffer, out: &mut Vec<u8>) -> Result<()> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| RecorderError::Encode("frame width overflows".into()))?;
    let stride = frame.stride as usize;
    if stride < row_bytes {
        return Err(RecorderError::Encode(format!(
            "stride {stride} is smaller than the {row_bytes}-byte row"
        )));
    }
    let needed = stride
        .checked_mul(height)
        .ok_or_else(|| RecorderError::Encode("frame height overflows".into()))?;
    if frame.data.len() < needed {
        return Err(RecorderError::Encode(format!(
            "image data ({} bytes) is shorter than height * stride ({needed} bytes)",
            frame.data.len()
        )));
    }

    out.clear();
    out.reserve(width.saturating_mul(height).saturating_mul(3));
    for y in 0..height {
        let start = y * stride;
        let row = &frame.data[start..start + row_bytes];
        for pixel in row.chunks_exact(4) {
            out.push(pixel[0]);
            out.push(pixel[1]);
            out.push(pixel[2]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::PixelFormat;

    /// Row-padded RGBA frame whose padding bytes are poisoned.
    fn padded_frame() -> ImageBuffer {
        let stride = 12u32; // 2 pixels * 4 bytes + 4 padding bytes
        let mut data = vec![0u8; (stride * 2) as usize];
        data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        data[4..8].copy_from_slice(&[0, 255, 0, 255]);
        data[8..12].copy_from_slice(&[9, 9, 9, 9]);
        let row = stride as usize;
        data[row..row + 4].copy_from_slice(&[0, 0, 255, 255]);
        data[row + 4..row + 8].copy_from_slice(&[255, 255, 255, 255]);
        data[row + 8..row + 12].copy_from_slice(&[9, 9, 9, 9]);
        ImageBuffer {
            width: 2,
            height: 2,
            stride,
            format: PixelFormat::Rgba8,
            data,
        }
    }

    #[test]
    fn rgba_to_rgb_honours_stride() {
        let mut out = Vec::new();
        rgba_to_rgb(&padded_frame(), &mut out).unwrap();
        assert_eq!(out, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn rgba_to_rgb_rejects_a_stride_smaller_than_the_row() {
        let frame = ImageBuffer {
            width: 2,
            height: 1,
            stride: 4,
            format: PixelFormat::Rgba8,
            data: vec![0; 4],
        };
        assert!(matches!(
            rgba_to_rgb(&frame, &mut Vec::new()),
            Err(RecorderError::Encode(_))
        ));
    }

    #[test]
    fn rgba_to_rgb_rejects_truncated_data() {
        let mut frame = padded_frame();
        frame.data.truncate(10);
        assert!(matches!(
            rgba_to_rgb(&frame, &mut Vec::new()),
            Err(RecorderError::Encode(_))
        ));
    }

    #[test]
    fn software_backend_rejects_a_zero_sized_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut encoder = MjpegEncoder::create(dir.path().join("zero.avi"), 30).unwrap();
        let frame = ImageBuffer::new_rgba(0, 0);
        assert!(matches!(
            encoder.encode_frame(&frame, 0),
            Err(RecorderError::Encode(_))
        ));
    }
}
