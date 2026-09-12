//! A minimal, streaming RIFF/AVI muxer for Motion-JPEG video.
//!
//! [`AviWriter`] produces a valid AVI 1.0 file:
//!
//! ```text
//! RIFF ('AVI '
//!   LIST ('hdrl'
//!     'avih' <AVIMAINHEADER>
//!     LIST ('strl'
//!       'strh' <AVISTREAMHEADER, fccType='vids', fccHandler='MJPG'>
//!       'strf' <BITMAPINFOHEADER, biCompression='MJPG', biBitCount=24>))
//!   LIST ('movi'
//!     '00dc' <jpeg bytes> [pad]
//!     ...)
//!   'idx1' <AVIOLDINDEX>)
//! ```
//!
//! Frames are written as they arrive; the container sizes, the frame counts in
//! `avih`/`strh` and the `'movi'` list size are patched in place by
//! [`AviWriter::finish`]. A trailing `'idx1'` index makes every frame seekable.
//!
//! The index stores each chunk's offset relative to the **`'movi'` FOURCC**
//! (the first byte of the `'movi'` identifier), which is the convention used by
//! `ffmpeg` and the OpenDML/AVI spec.

use std::io::{Seek, SeekFrom, Write};

use crate::error::{RecorderError, Result};

const AVIHEADER_AVIH_SIZE: u32 = 56;
const BITMAPINFOHEADER_SIZE: u32 = 40;
const AVISTREAMHEADER_SIZE: u32 = 56;

/// `AVIF_HASINDEX`: the file carries an `'idx1'` index.
const AVIF_HASINDEX: u32 = 0x0000_0010;
/// `AVIIF_KEYFRAME`: every Motion-JPEG frame is a key frame.
const AVIIF_KEYFRAME: u32 = 0x0000_0010;
/// Quality marker written into `strh.dwQuality` (`-1`).
const AVI_DEFAULT_QUALITY: u32 = 0xFFFF_FFFF;

/// One `'idx1'` entry: the chunk's offset (relative to `'movi'`) and byte length.
#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    offset: u32,
    length: u32,
}

/// A streaming writer that muxes JPEG frames into an AVI `'MJPG'` stream.
///
/// The generic `W` must support seeking so the header sizes can be patched once
/// the last frame has been written; a `BufWriter<File>` is the intended sink.
pub struct AviWriter<W: Write + Seek> {
    inner: W,
    /// Logical write position (bytes written), kept in sync with `inner`.
    pos: u64,
    width: u32,
    height: u32,
    fps: u32,
    frames: u64,
    max_frame_bytes: u32,
    index: Vec<IndexEntry>,
    /// Offset of the RIFF chunk's size field.
    riff_size_offset: u64,
    /// Offset of `avih.dwMaxBytesPerSec`.
    avih_max_bytes_offset: u64,
    /// Offset of `avih.dwTotalFrames`.
    avih_frames_offset: u64,
    /// Offset of `avih.dwSuggestedBufferSize`.
    avih_buffer_offset: u64,
    /// Offset of `strh.dwLength`.
    strh_length_offset: u64,
    /// Offset of `strh.dwSuggestedBufferSize`.
    strh_buffer_offset: u64,
    /// Offset of the `'movi'` LIST size field.
    movi_size_offset: u64,
    /// Position of the `'movi'` FOURCC; index offsets are relative to it.
    movi_base: u64,
    /// Set once [`AviWriter::finish`] has run.
    finished: bool,
}

impl<W: Write + Seek> AviWriter<W> {
    /// Writes the RIFF/AVI headers for a `width`x`height` `'MJPG'` stream at `fps`.
    ///
    /// Returns [`RecorderError::Mux`] for a zero width, height or frame rate.
    pub fn new(inner: W, width: u32, height: u32, fps: u32) -> Result<AviWriter<W>> {
        if width == 0 || height == 0 {
            return Err(RecorderError::Mux(format!(
                "cannot mux a {width}x{height} video"
            )));
        }
        if fps == 0 {
            return Err(RecorderError::Mux("cannot mux a video with 0 fps".into()));
        }
        let mut w = AviWriter {
            inner,
            pos: 0,
            width,
            height,
            fps,
            frames: 0,
            max_frame_bytes: 0,
            index: Vec::new(),
            riff_size_offset: 0,
            avih_max_bytes_offset: 0,
            avih_frames_offset: 0,
            avih_buffer_offset: 0,
            strh_length_offset: 0,
            strh_buffer_offset: 0,
            movi_size_offset: 0,
            movi_base: 0,
            finished: false,
        };
        w.write_header()?;
        Ok(w)
    }

    /// Number of frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Appends one encoded (JPEG) frame as a `'00dc'` chunk.
    ///
    /// Odd-length payloads are padded to a word boundary, as RIFF requires.
    pub fn write_frame(&mut self, data: &[u8]) -> Result<()> {
        let offset = (self.pos - self.movi_base) as u32;
        let length = data.len() as u32;
        self.write_bytes(b"00dc")?;
        self.write_u32(length)?;
        self.write_bytes(data)?;
        if data.len() % 2 == 1 {
            self.write_bytes(&[0])?;
        }
        self.index.push(IndexEntry { offset, length });
        self.frames += 1;
        self.max_frame_bytes = self.max_frame_bytes.max(length);
        Ok(())
    }

    /// Writes the `'idx1'` index and patches every deferred header size.
    ///
    /// Safe to call more than once; subsequent calls are no-ops.
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let movi_size = (self.pos - self.movi_base) as u32;
        self.patch_u32(self.movi_size_offset, movi_size)?;

        self.write_bytes(b"idx1")?;
        self.write_u32((self.index.len() * 16) as u32)?;
        for i in 0..self.index.len() {
            let entry = self.index[i];
            self.write_bytes(b"00dc")?;
            self.write_u32(AVIIF_KEYFRAME)?;
            self.write_u32(entry.offset)?;
            self.write_u32(entry.length)?;
        }

        let frames = self.frames as u32;
        let max_bytes_per_sec = self
            .max_frame_bytes
            .saturating_add(8)
            .saturating_mul(self.fps);
        self.patch_u32(self.avih_frames_offset, frames)?;
        self.patch_u32(self.avih_max_bytes_offset, max_bytes_per_sec)?;
        self.patch_u32(self.avih_buffer_offset, self.max_frame_bytes)?;
        self.patch_u32(self.strh_length_offset, frames)?;
        self.patch_u32(self.strh_buffer_offset, self.max_frame_bytes)?;

        let total = self.pos;
        self.patch_u32(self.riff_size_offset, (total - 8) as u32)?;
        self.inner.flush()?;
        Ok(())
    }

    fn write_header(&mut self) -> Result<()> {
        self.write_bytes(b"RIFF")?;
        self.riff_size_offset = self.pos;
        self.write_u32(0)?;
        self.write_bytes(b"AVI ")?;

        // LIST hdrl
        self.write_bytes(b"LIST")?;
        let hdrl_size_offset = self.pos;
        self.write_u32(0)?;
        self.write_bytes(b"hdrl")?;

        // avih (AVIMAINHEADER, 56 bytes)
        self.write_bytes(b"avih")?;
        self.write_u32(AVIHEADER_AVIH_SIZE)?;
        let avih_start = self.pos;
        self.avih_max_bytes_offset = avih_start + 4;
        self.avih_frames_offset = avih_start + 16;
        self.avih_buffer_offset = avih_start + 28;
        self.write_u32(1_000_000 / self.fps)?; // dwMicroSecPerFrame
        self.write_u32(0)?; // dwMaxBytesPerSec (patched)
        self.write_u32(0)?; // dwPaddingGranularity
        self.write_u32(AVIF_HASINDEX)?; // dwFlags
        self.write_u32(0)?; // dwTotalFrames (patched)
        self.write_u32(0)?; // dwInitialFrames
        self.write_u32(1)?; // dwStreams
        self.write_u32(0)?; // dwSuggestedBufferSize (patched)
        self.write_u32(self.width)?; // dwWidth
        self.write_u32(self.height)?; // dwHeight
        self.write_u32(0)?; // dwReserved[0]
        self.write_u32(0)?; // dwReserved[1]
        self.write_u32(0)?; // dwReserved[2]
        self.write_u32(0)?; // dwReserved[3]

        // LIST strl
        self.write_bytes(b"LIST")?;
        let strl_size_offset = self.pos;
        self.write_u32(0)?;
        self.write_bytes(b"strl")?;

        // strh (AVISTREAMHEADER, 56 bytes)
        self.write_bytes(b"strh")?;
        self.write_u32(AVISTREAMHEADER_SIZE)?;
        let strh_start = self.pos;
        self.strh_length_offset = strh_start + 32;
        self.strh_buffer_offset = strh_start + 36;
        self.write_bytes(b"vids")?; // fccType
        self.write_bytes(b"MJPG")?; // fccHandler
        self.write_u32(0)?; // dwFlags
        self.write_u16(0)?; // wPriority
        self.write_u16(0)?; // wLanguage
        self.write_u32(0)?; // dwInitialFrames
        self.write_u32(1)?; // dwScale
        self.write_u32(self.fps)?; // dwRate  (rate/scale = fps)
        self.write_u32(0)?; // dwStart
        self.write_u32(0)?; // dwLength (patched)
        self.write_u32(0)?; // dwSuggestedBufferSize (patched)
        self.write_u32(AVI_DEFAULT_QUALITY)?; // dwQuality
        self.write_u32(0)?; // dwSampleSize (variable per frame)
        self.write_u16(0)?; // rcFrame.left
        self.write_u16(0)?; // rcFrame.top
        self.write_u16(self.width as u16)?; // rcFrame.right
        self.write_u16(self.height as u16)?; // rcFrame.bottom

        // strf (BITMAPINFOHEADER, 40 bytes)
        self.write_bytes(b"strf")?;
        self.write_u32(BITMAPINFOHEADER_SIZE)?;
        self.write_u32(BITMAPINFOHEADER_SIZE)?; // biSize
        self.write_u32(self.width)?; // biWidth
        self.write_u32(self.height)?; // biHeight
        self.write_u16(1)?; // biPlanes
        self.write_u16(24)?; // biBitCount
        self.write_bytes(b"MJPG")?; // biCompression
        let size_image =
            (u64::from(self.width) * u64::from(self.height) * 3).min(u64::from(u32::MAX));
        self.write_u32(size_image as u32)?; // biSizeImage
        self.write_u32(0)?; // biXPelsPerMeter
        self.write_u32(0)?; // biYPelsPerMeter
        self.write_u32(0)?; // biClrUsed
        self.write_u32(0)?; // biClrImportant

        let strl_end = self.pos;
        self.patch_u32(strl_size_offset, (strl_end - (strl_size_offset + 4)) as u32)?;
        let hdrl_end = self.pos;
        self.patch_u32(hdrl_size_offset, (hdrl_end - (hdrl_size_offset + 4)) as u32)?;

        // LIST movi
        self.write_bytes(b"LIST")?;
        self.movi_size_offset = self.pos;
        self.write_u32(0)?;
        self.movi_base = self.pos;
        self.write_bytes(b"movi")?;
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    fn write_u16(&mut self, value: u16) -> Result<()> {
        self.write_bytes(&value.to_le_bytes())
    }

    fn write_u32(&mut self, value: u32) -> Result<()> {
        self.write_bytes(&value.to_le_bytes())
    }

    /// Overwrites 4 bytes at `offset` without disturbing the logical position.
    fn patch_u32(&mut self, offset: u64, value: u32) -> Result<()> {
        let saved = self.pos;
        self.inner.seek(SeekFrom::Start(offset))?;
        self.inner.write_all(&value.to_le_bytes())?;
        self.inner.seek(SeekFrom::Start(saved))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_zero_dimensions() {
        assert!(matches!(
            AviWriter::new(Cursor::new(Vec::new()), 0, 10, 30),
            Err(RecorderError::Mux(_))
        ));
        assert!(matches!(
            AviWriter::new(Cursor::new(Vec::new()), 10, 0, 30),
            Err(RecorderError::Mux(_))
        ));
    }

    #[test]
    fn rejects_zero_fps() {
        assert!(matches!(
            AviWriter::new(Cursor::new(Vec::new()), 4, 4, 0),
            Err(RecorderError::Mux(_))
        ));
    }

    #[test]
    fn odd_length_frames_are_padded_to_a_word() {
        let mut writer = AviWriter::new(Cursor::new(Vec::new()), 4, 4, 25).unwrap();
        let before = writer.pos;
        writer.write_frame(&[1, 2, 3]).unwrap();
        // 4-byte id + 4-byte size + 3 bytes + 1 pad byte.
        assert_eq!(writer.pos - before, 8 + 3 + 1);
        writer.write_frame(&[4, 5]).unwrap();
        assert_eq!(writer.frames(), 2);
        writer.finish().unwrap();

        let bytes = writer.inner.into_inner();
        assert_eq!(bytes.len() % 2, 0, "RIFF files are word-aligned");
        let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        assert_eq!(riff_size, bytes.len() - 8);
    }

    #[test]
    fn finish_is_idempotent() {
        let mut writer = AviWriter::new(Cursor::new(Vec::new()), 4, 4, 25).unwrap();
        writer.write_frame(&[1, 2]).unwrap();
        writer.finish().unwrap();
        let len = writer.pos;
        writer.finish().unwrap();
        assert_eq!(writer.pos, len, "a second finish must not append anything");
    }
}
