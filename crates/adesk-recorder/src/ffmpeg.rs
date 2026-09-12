//! GPU-accelerated `ffmpeg` child-process encoder.
//!
//! This backend spawns an external `ffmpeg` process and pipes raw RGBA frames
//! to its stdin (`-f rawvideo -pix_fmt rgba -s WxH -r FPS -i -`). It selects a
//! hardware H.264 encoder when one is available (`h264_vaapi` with
//! `-vaapi_device /dev/dri/renderD128`, else `h264_nvenc`, else
//! `h264_v4l2m2m`) and falls back to `libx264`, writing an `.mp4`.
//!
//! Availability is probed once with `ffmpeg -hide_banner -encoders` and cached.
//! When `ffmpeg` (or a hardware encoder) is absent the backend degrades
//! cleanly: [`FfmpegEncoder::create`] reports a structured
//! [`RecorderError::Unsupported`] and nothing panics.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;

use adesk_core::ImageBuffer;

use crate::encoder::Encoder;
use crate::error::{RecorderError, Result};

/// The render node `h264_vaapi` is asked to upload frames through.
const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// A hardware H.264 encoder offered by the `ffmpeg` build (or the software
/// `libx264` fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FfmpegChoice {
    H264Vaapi,
    H264Nvenc,
    H264V4l2m2m,
    LibX264,
}

impl FfmpegChoice {
    /// The `ffmpeg` encoder name.
    fn name(self) -> &'static str {
        match self {
            FfmpegChoice::H264Vaapi => "h264_vaapi",
            FfmpegChoice::H264Nvenc => "h264_nvenc",
            FfmpegChoice::H264V4l2m2m => "h264_v4l2m2m",
            FfmpegChoice::LibX264 => "libx264",
        }
    }

    /// Whether this encoder is GPU-accelerated.
    fn is_hardware(self) -> bool {
        !matches!(self, FfmpegChoice::LibX264)
    }
}

/// Cached result of probing `ffmpeg -hide_banner -encoders`.
///
/// `None` means `ffmpeg` could not be run; `Some(names)` is the parsed encoder
/// list.
static ENCODER_CACHE: OnceLock<Option<Vec<String>>> = OnceLock::new();

/// Runs `ffmpeg -hide_banner -encoders` once and parses the encoder names.
fn probe_encoders() -> Option<&'static Vec<String>> {
    ENCODER_CACHE
        .get_or_init(|| {
            let output = Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .stdin(Stdio::null())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            let mut names = Vec::new();
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let (Some(flags), Some(name)) = (fields.next(), fields.next()) else {
                    continue;
                };
                // Encoder rows look like ` V....D h264_vaapi  H.264 ...`; the
                // legend/separator rows start with a non-uppercase character.
                if flags.len() == 6
                    && flags.as_bytes()[0].is_ascii_uppercase()
                    && !name.starts_with('=')
                {
                    names.push(name.to_string());
                }
            }
            Some(names)
        })
        .as_ref()
}

/// Whether `ffmpeg` is installed and runnable.
pub fn ffmpeg_available() -> bool {
    probe_encoders().is_some()
}

/// Selects the preferred hardware H.264 encoder, if `ffmpeg` offers one.
fn select_hardware_encoder() -> Option<FfmpegChoice> {
    let names = probe_encoders()?;
    [
        ("h264_vaapi", FfmpegChoice::H264Vaapi),
        ("h264_nvenc", FfmpegChoice::H264Nvenc),
        ("h264_v4l2m2m", FfmpegChoice::H264V4l2m2m),
    ]
    .into_iter()
    .find_map(|(name, choice)| names.iter().any(|n| n == name).then_some(choice))
}

/// Whether a GPU-accelerated H.264 encoder is available.
///
/// This is the probe behind [`crate::detect`] and [`crate::EncoderKind::Auto`]:
/// it is `true` only when `ffmpeg` is present **and** offers a hardware encoder.
pub fn gpu_available() -> bool {
    select_hardware_encoder().is_some()
}

/// The GPU-accelerated `ffmpeg` encoder.
pub struct FfmpegEncoder {
    path: PathBuf,
    fps: u32,
    /// The encoder chosen at construction (hardware preferred).
    preferred: FfmpegChoice,
    /// The encoder actually in use once the process has been spawned.
    active: FfmpegChoice,
    /// Human-readable name returned by [`Encoder::name`].
    name: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr_thread: Option<JoinHandle<String>>,
    width: u32,
    height: u32,
    raw: Vec<u8>,
    frames: u64,
    finished: bool,
}

impl FfmpegEncoder {
    /// Prepares an `ffmpeg`-backed encoder writing H.264 to `path`.
    ///
    /// The `ffmpeg` process is spawned lazily on the first frame (its `-s WxH`
    /// argument needs the frame size). Returns [`RecorderError::Unsupported`]
    /// when `ffmpeg` cannot be run at all.
    pub fn create(path: impl Into<PathBuf>, fps: u32) -> Result<FfmpegEncoder> {
        if !ffmpeg_available() {
            return Err(RecorderError::Unsupported(
                "ffmpeg is not available for GPU-accelerated recording".into(),
            ));
        }
        let preferred = select_hardware_encoder().unwrap_or(FfmpegChoice::LibX264);
        Ok(FfmpegEncoder {
            path: path.into(),
            fps,
            preferred,
            active: preferred,
            name: format!("ffmpeg/{}", preferred.name()),
            child: None,
            stdin: None,
            stderr_thread: None,
            width: 0,
            height: 0,
            raw: Vec::new(),
            frames: 0,
            finished: false,
        })
    }

    /// The output path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Spawns the `ffmpeg` process for the first frame, falling back to
    /// `libx264` if the preferred hardware encoder fails to start.
    fn ensure_started(&mut self, width: u32, height: u32) -> Result<()> {
        if self.child.is_some() {
            if (self.width, self.height) != (width, height) {
                return Err(RecorderError::Encode(format!(
                    "frame size changed from {}x{} to {width}x{height}",
                    self.width, self.height
                )));
            }
            return Ok(());
        }
        if width == 0 || height == 0 {
            return Err(RecorderError::Encode(
                "cannot encode a zero-sized frame".into(),
            ));
        }
        self.width = width;
        self.height = height;

        match self.spawn(self.preferred) {
            Ok(()) => Ok(()),
            Err(err) if self.preferred.is_hardware() => {
                tracing::warn!(
                    encoder = self.preferred.name(),
                    error = %err,
                    "hardware ffmpeg encoder failed to start; falling back to libx264"
                );
                self.spawn(FfmpegChoice::LibX264)
            }
            Err(err) => Err(err),
        }
    }

    fn spawn(&mut self, choice: FfmpegChoice) -> Result<()> {
        let args = self.build_args(choice);
        let mut child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| RecorderError::Backend(format!("failed to start ffmpeg: {err}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RecorderError::Backend("ffmpeg stdin was not captured".into()))?;
        if let Some(stderr) = child.stderr.take() {
            self.stderr_thread = Some(std::thread::spawn(move || drain(stderr)));
        }
        self.stdin = Some(stdin);
        self.child = Some(child);
        self.active = choice;
        self.name = format!("ffmpeg/{}", choice.name());
        Ok(())
    }

    fn build_args(&self, choice: FfmpegChoice) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let mut args: Vec<OsString> = Vec::new();
        for arg in ["-hide_banner", "-loglevel", "error", "-y"] {
            args.push(arg.into());
        }
        if choice == FfmpegChoice::H264Vaapi {
            args.push("-vaapi_device".into());
            args.push(VAAPI_DEVICE.into());
        }
        for arg in ["-f", "rawvideo", "-pix_fmt", "rgba"] {
            args.push(arg.into());
        }
        args.push("-s".into());
        args.push(format!("{}x{}", self.width, self.height).into());
        args.push("-r".into());
        args.push(self.fps.to_string().into());
        for arg in ["-i", "-", "-an"] {
            args.push(arg.into());
        }
        match choice {
            FfmpegChoice::H264Vaapi => {
                for arg in ["-vf", "format=nv12,hwupload", "-c:v", "h264_vaapi"] {
                    args.push(arg.into());
                }
            }
            FfmpegChoice::H264Nvenc => {
                for arg in ["-c:v", "h264_nvenc", "-pix_fmt", "yuv420p"] {
                    args.push(arg.into());
                }
            }
            FfmpegChoice::H264V4l2m2m => {
                for arg in ["-c:v", "h264_v4l2m2m", "-pix_fmt", "yuv420p"] {
                    args.push(arg.into());
                }
            }
            FfmpegChoice::LibX264 => {
                for arg in ["-c:v", "libx264", "-pix_fmt", "yuv420p"] {
                    args.push(arg.into());
                }
            }
        }
        args.push(self.path.as_os_str().to_owned());
        args
    }
}

impl Encoder for FfmpegEncoder {
    fn name(&self) -> &str {
        &self.name
    }

    fn frames(&self) -> u64 {
        self.frames
    }

    fn encode_frame(&mut self, frame: &ImageBuffer, _ts_ms: u64) -> Result<()> {
        if self.finished {
            return Err(RecorderError::Backend(
                "ffmpeg encoder already finalized".into(),
            ));
        }
        self.ensure_started(frame.width, frame.height)?;
        pack_rgba(frame, &mut self.raw)?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| RecorderError::Backend("ffmpeg stdin is closed".into()))?;
        stdin.write_all(&self.raw).map_err(|err| {
            RecorderError::Backend(format!("failed to write frame to ffmpeg: {err}"))
        })?;
        self.frames += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        // Closing stdin signals EOF so ffmpeg flushes and finalizes the file.
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let status = child.wait().map_err(|err| {
                RecorderError::Backend(format!("failed to wait for ffmpeg: {err}"))
            })?;
            let stderr = self
                .stderr_thread
                .take()
                .and_then(|handle| handle.join().ok())
                .unwrap_or_default();
            if !status.success() {
                return Err(RecorderError::Backend(format!(
                    "ffmpeg exited with {status}: {}",
                    stderr.trim()
                )));
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for FfmpegEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegEncoder")
            .field("path", &self.path)
            .field("encoder", &self.name)
            .field("frames", &self.frames)
            .field("running", &self.child.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(self.stdin.take());
            let _ = child.wait();
            if let Some(handle) = self.stderr_thread.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Converts an RGBA8 frame (honouring `stride`) into tightly packed RGBA.
fn pack_rgba(frame: &ImageBuffer, out: &mut Vec<u8>) -> Result<()> {
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
    out.reserve(needed);
    if stride == row_bytes {
        out.extend_from_slice(&frame.data[..needed]);
    } else {
        for y in 0..height {
            let start = y * stride;
            out.extend_from_slice(&frame.data[start..start + row_bytes]);
        }
    }
    Ok(())
}

/// Reads a child's stderr to completion so it can be reported on failure.
fn drain<R: Read>(reader: R) -> String {
    let mut buffer = String::new();
    let _ = BufReader::new(reader).read_to_string(&mut buffer);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::PixelFormat;

    #[test]
    fn probe_never_panics_and_is_cached() {
        let first = gpu_available();
        assert_eq!(gpu_available(), first, "the probe result is cached");
        // Whether or not ffmpeg is installed this must simply return a bool.
        let _ = ffmpeg_available();
    }

    #[test]
    fn choice_metadata() {
        assert_eq!(FfmpegChoice::H264Vaapi.name(), "h264_vaapi");
        assert_eq!(FfmpegChoice::LibX264.name(), "libx264");
        assert!(FfmpegChoice::H264Nvenc.is_hardware());
        assert!(FfmpegChoice::H264V4l2m2m.is_hardware());
        assert!(!FfmpegChoice::LibX264.is_hardware());
    }

    #[test]
    fn pack_rgba_honours_stride() {
        let stride = 12u32;
        let mut data = vec![0u8; (stride * 2) as usize];
        data[0..4].copy_from_slice(&[1, 2, 3, 4]);
        data[4..8].copy_from_slice(&[5, 6, 7, 8]);
        data[8..12].copy_from_slice(&[9, 9, 9, 9]);
        let row = stride as usize;
        data[row..row + 4].copy_from_slice(&[10, 11, 12, 13]);
        data[row + 4..row + 8].copy_from_slice(&[14, 15, 16, 17]);
        data[row + 8..row + 12].copy_from_slice(&[9, 9, 9, 9]);
        let frame = ImageBuffer {
            width: 2,
            height: 2,
            stride,
            format: PixelFormat::Rgba8,
            data,
        };
        let mut out = Vec::new();
        pack_rgba(&frame, &mut out).unwrap();
        assert_eq!(
            out,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17]
        );
    }

    #[test]
    fn pack_rgba_rejects_a_stride_smaller_than_the_row() {
        let frame = ImageBuffer {
            width: 2,
            height: 1,
            stride: 4,
            format: PixelFormat::Rgba8,
            data: vec![0; 4],
        };
        assert!(matches!(
            pack_rgba(&frame, &mut Vec::new()),
            Err(RecorderError::Encode(_))
        ));
    }

    #[test]
    fn gpu_request_without_ffmpeg_is_unsupported() {
        if ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let err = FfmpegEncoder::create(dir.path().join("out.mp4"), 30).unwrap_err();
        assert!(matches!(err, RecorderError::Unsupported(_)));
    }
}
