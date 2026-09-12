//! Round-trip test: record synthetic RGBA frames, then re-open the resulting
//! AVI, parse its RIFF structure, validate the `idx1` index and decode every
//! Motion-JPEG frame with `jpeg-decoder`.
//!
//! The frames are deliberately **row-padded** (stride > width * 4) and the
//! padding is poisoned, so a backend that ignored `stride` would decode wrongly.

use std::io::Cursor;

use adesk_core::{ImageBuffer, PixelFormat};
use adesk_recorder::{EncoderKind, Recorder, RecorderConfig};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 96;
/// Extra padding bytes per row (8 pixels' worth).
const PADDING: u32 = 32;

/// Builds a padded RGBA frame with three horizontal colour bands.
fn sample_frame(brightness: u8) -> ImageBuffer {
    let stride = WIDTH * 4 + PADDING;
    let mut data = vec![0u8; (stride * HEIGHT) as usize];
    for y in 0..HEIGHT {
        let base = (y * stride) as usize;
        let (r, g, b): (u8, u8, u8) = match y {
            0..=31 => (200, 20, 20),
            32..=63 => (20, 200, 20),
            _ => (20, 20, 200),
        };
        for x in 0..WIDTH {
            let p = base + (x * 4) as usize;
            data[p] = r.saturating_add(brightness);
            data[p + 1] = g;
            data[p + 2] = b;
            data[p + 3] = 255;
        }
        for pad in (WIDTH * 4) as usize..stride as usize {
            data[base + pad] = 255;
        }
    }
    ImageBuffer {
        width: WIDTH,
        height: HEIGHT,
        stride,
        format: PixelFormat::Rgba8,
        data,
    }
}

fn u16_le(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_le(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// The parsed pieces of an AVI file that the assertions need.
struct ParsedAvi {
    frames: Vec<Vec<u8>>,
    index: Vec<(u32, u32)>,
    avih_frames: u32,
    strh_length: u32,
    header_end: usize,
}

/// Walks the RIFF chunk tree and extracts the `'movi'` frames and `'idx1'` index.
fn parse_avi(data: &[u8]) -> ParsedAvi {
    assert!(data.len() > 12, "file is too short to be an AVI");
    assert_eq!(&data[0..4], b"RIFF");
    assert_eq!(u32_le(data, 4) as usize, data.len() - 8, "RIFF size");
    assert_eq!(&data[8..12], b"AVI ");

    let mut pos = 12usize;
    let mut movi: Option<(usize, usize, usize)> = None;
    let mut index = Vec::new();
    let mut header_end = 12usize;
    let mut saw_hdrl = false;
    let mut saw_idx1 = false;

    while pos + 8 <= data.len() {
        let fourcc = &data[pos..pos + 4];
        let size = u32_le(data, pos + 4) as usize;
        let body = pos + 8;
        assert!(
            body + size <= data.len(),
            "chunk at {pos} overruns the file"
        );

        if fourcc == b"LIST" {
            let list_type = &data[body..body + 4];
            match list_type {
                b"hdrl" => {
                    saw_hdrl = true;
                    header_end = body + size;
                }
                b"movi" => movi = Some((body, body + 4, body + size)),
                _ => {}
            }
        } else if fourcc == b"idx1" {
            saw_idx1 = true;
            let mut p = body;
            while p + 16 <= body + size {
                assert_eq!(&data[p..p + 4], b"00dc", "index entry id");
                assert_eq!(u32_le(data, p + 4) & 0x10, 0x10, "index key-frame flag");
                index.push((u32_le(data, p + 8), u32_le(data, p + 12)));
                p += 16;
            }
            assert_eq!(p, body + size, "idx1 size is not a whole number of entries");
        }
        pos = body + size + (size & 1);
    }
    assert_eq!(pos, data.len(), "top-level chunks do not fill the file");
    assert!(saw_hdrl, "missing hdrl list");
    assert!(saw_idx1, "missing idx1 index");

    // `strh`/`strf` live inside `hdrl`; scan only the header region.
    let header = &data[12..header_end];
    let strh = find_payload(header, b"strh").expect("strh header");
    assert_eq!(&header[strh..strh + 4], b"vids", "strh fccType");
    assert_eq!(&header[strh + 4..strh + 8], b"MJPG", "strh fccHandler");
    let strh_length = u32_le(header, strh + 32);
    let strf = find_payload(header, b"strf").expect("strf header");
    assert_eq!(u32_le(header, strf), 40, "biSize");
    assert_eq!(u32_le(header, strf + 4), WIDTH, "biWidth");
    assert_eq!(u32_le(header, strf + 8), HEIGHT, "biHeight");
    assert_eq!(u16_le(header, strf + 12), 1, "biPlanes");
    assert_eq!(u16_le(header, strf + 14), 24, "biBitCount");
    assert_eq!(&header[strf + 16..strf + 20], b"MJPG", "biCompression");
    let avih = find_payload(header, b"avih").expect("avih header");
    let avih_frames = u32_le(header, avih + 16);

    let (movi_base, chunks_start, movi_end) = movi.expect("missing movi list");

    // Extract frames and their positions.
    let mut frames = Vec::new();
    let mut chunk_positions = Vec::new();
    let mut p = chunks_start;
    while p + 8 <= movi_end {
        assert_eq!(&data[p..p + 4], b"00dc");
        let len = u32_le(data, p + 4) as usize;
        frames.push(data[p + 8..p + 8 + len].to_vec());
        chunk_positions.push((p, len));
        p += 8 + len + (len & 1);
    }
    assert_eq!(p, movi_end, "movi list size does not match its chunks");

    // The index must point at exactly those chunks (offsets relative to 'movi').
    assert_eq!(index.len(), chunk_positions.len(), "index length");
    for ((offset, length), (pos, real_len)) in index.iter().zip(&chunk_positions) {
        assert_eq!(*offset as usize, pos - movi_base, "index offset");
        assert_eq!(*length as usize, *real_len, "index length");
    }

    ParsedAvi {
        frames,
        index,
        avih_frames,
        strh_length,
        header_end,
    }
}

/// Finds a four-character chunk id and returns the offset of its payload.
///
/// A chunk is `fourcc(4) + size(4) + payload`, so the payload starts 8 bytes on.
fn find_payload(haystack: &[u8], fourcc: &[u8; 4]) -> Option<usize> {
    haystack
        .windows(4)
        .position(|window| window == fourcc)
        .map(|at| at + 8)
}

/// Decodes one Motion-JPEG frame to tightly packed RGB.
fn decode_jpeg(jpeg: &[u8]) -> (u16, u16, Vec<u8>) {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
    decoder.set_color_transform(jpeg_decoder::ColorTransform::YCbCr);
    decoder.read_info().expect("JPEG header");
    let info = decoder.info().expect("JPEG info");
    assert_eq!(info.pixel_format, jpeg_decoder::PixelFormat::RGB24);
    let pixels = decoder.decode().expect("JPEG payload");
    (info.width, info.height, pixels)
}

fn sample_rgb(pixels: &[u8], x: u32, y: u32) -> (u8, u8, u8) {
    let index = ((y * WIDTH + x) * 3) as usize;
    (pixels[index], pixels[index + 1], pixels[index + 2])
}

fn assert_close(actual: (u8, u8, u8), expected: (u8, u8, u8), label: &str) {
    let close = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= 40;
    assert!(
        close(actual.0, expected.0) && close(actual.1, expected.1) && close(actual.2, expected.2),
        "{label}: expected {expected:?} but decoded {actual:?}"
    );
}

#[test]
fn avi_round_trips_and_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recording.avi");

    let frame_count = 5u64;
    let mut recorder =
        Recorder::create(RecorderConfig::new(&path).with_encoder(EncoderKind::Software)).unwrap();
    assert_eq!(recorder.encoder_name(), "mjpeg");
    for i in 0..frame_count {
        recorder
            .push_frame(&sample_frame((i as u8) * 2), i * 33)
            .unwrap();
    }
    assert_eq!(recorder.frames(), frame_count);
    let summary = recorder.finish().unwrap();
    assert_eq!(summary.frames, frame_count);
    assert_eq!(summary.path, path);
    assert_eq!(summary.encoder, "mjpeg");
    assert_eq!(summary.duration_ms, (frame_count - 1) * 33);

    let bytes = std::fs::read(&path).unwrap();
    let parsed = parse_avi(&bytes);
    assert_eq!(parsed.frames.len() as u64, frame_count);
    assert_eq!(parsed.index.len() as u64, frame_count);
    assert_eq!(parsed.avih_frames, frame_count as u32);
    assert_eq!(parsed.strh_length, frame_count as u32);
    assert!(parsed.header_end < bytes.len());

    for (i, jpeg) in parsed.frames.iter().enumerate() {
        let (width, height, pixels) = decode_jpeg(jpeg);
        assert_eq!((width as u32, height as u32), (WIDTH, HEIGHT));
        let brightness = (i as u8) * 2;
        // Sample the centre of each colour band, well away from band edges and
        // from the poisoned row padding.
        assert_close(
            sample_rgb(&pixels, 48, 16),
            (200 + brightness, 20, 20),
            "red band",
        );
        assert_close(sample_rgb(&pixels, 48, 48), (20, 200, 20), "green band");
        assert_close(sample_rgb(&pixels, 48, 80), (20, 20, 200), "blue band");
    }
}

#[test]
fn tightly_packed_and_padded_frames_agree() {
    let dir = tempfile::tempdir().unwrap();

    let padded = sample_frame(0);
    // The same logical content, tightly packed.
    let mut tight_data = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            tight_data.extend_from_slice(&padded.pixel(x, y).unwrap());
        }
    }
    let tight = ImageBuffer::from_rgba(WIDTH, HEIGHT, tight_data).unwrap();

    let record = |frame: &ImageBuffer, name: &str| {
        let path = dir.path().join(name);
        let mut recorder =
            Recorder::create(RecorderConfig::new(&path).with_encoder(EncoderKind::Software))
                .unwrap();
        recorder.push_frame(frame, 0).unwrap();
        recorder.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let parsed = parse_avi(&bytes);
        let (_, _, pixels) = decode_jpeg(&parsed.frames[0]);
        pixels
    };

    let padded_pixels = record(&padded, "padded.avi");
    let tight_pixels = record(&tight, "tight.avi");
    assert_eq!(padded_pixels.len(), tight_pixels.len());
    for (a, b) in padded_pixels.iter().zip(&tight_pixels) {
        assert!(
            (i32::from(*a) - i32::from(*b)).abs() <= 2,
            "padded vs tight pixels differ: {a} vs {b}"
        );
    }
}

#[test]
fn empty_recording_produces_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.avi");
    let recorder =
        Recorder::create(RecorderConfig::new(&path).with_encoder(EncoderKind::Software)).unwrap();
    let summary = recorder.finish().unwrap();
    assert_eq!(summary.frames, 0);
    assert_eq!(summary.duration_ms, 0);
    assert!(path.exists());
}
