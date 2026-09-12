# adesk-recorder — screen recording for the ADesk runtime

## Intent

`adesk-recorder` turns the runtime's rendered desktop frames into an encoded video
file. It is the *encoder + muxer* half of screen recording; the capture half
(pulling frames out of the compositor and pacing them) belongs to `adesk-server`,
which drives this crate.

The crate is deliberately decoupled: no compositor, no Smithay, no VAP/AGP protocol
types, no async runtime. Its input is a CPU-side `adesk_core::ImageBuffer` (RGBA8,
rows may be padded) plus a monotonic `ts_ms`; its output is a video file on disk.

Two encoder backends are compiled in and selected at runtime (never by a Cargo
feature):

- **Software** ([`mjpeg`], always available, the default fallback): a pure-Rust
  Motion-JPEG encoder (`jpeg-encoder`) muxed by [`avi::AviWriter`] into a RIFF/AVI
  `'MJPG'` file. No GPU, display, network, C toolchain or external process — it
  works in every environment, including CI.
- **GPU-accelerated** ([`ffmpeg`], used when available): an external `ffmpeg`
  child process driving a hardware H.264 encoder (`h264_vaapi` with
  `-vaapi_device /dev/dri/renderD128`, else `h264_nvenc`, else `h264_v4l2m2m`),
  falling back to `libx264`, writing an `.mp4`. Frames are piped to its stdin as
  raw RGBA. This is the "GPU accelerated stack if possible" path: it is detected
  at runtime and only selected when the platform actually provides a hardware
  encoder.

## API Surface

Crate root (`src/lib.rs`) re-exports every public item below; the modules are
`pub` as well.

### Crate root (`src/lib.rs`)
- `EncoderKind { Auto, Software, Gpu }` (`Default = Auto`) — the requested backend.
- `detect(kind: EncoderKind) -> EncoderKind` — resolves a request against the
  platform. `Auto` -> `Gpu` when `gpu_available()` else `Software`; explicit
  `Software`/`Gpu` pass through unchanged (so `Recorder::create` can still reject
  a `Gpu` request).
- `gpu_available() -> bool` — `true` iff `ffmpeg` is present **and** offers a
  hardware H.264 encoder (delegates to `ffmpeg::gpu_available`, a cached probe).
- `ffmpeg_available() -> bool` — `true` iff the `ffmpeg` binary can be run.
- `suggest_extension(kind: EncoderKind) -> &'static str` — `".mp4"` for `Gpu`,
  `".avi"` for `Software`/`Auto`-without-hardware.

### `src/error.rs`
- `RecorderError` (`thiserror`) + `pub type Result<T>`: `Io(std::io::Error)`,
  `Encode(String)`, `Unsupported(String)`, `Mux(String)`, `Backend(String)`.
- `RecorderError::code() -> ErrorCode`: `Unsupported` -> `not_supported`,
  `Encode`/`Mux` -> `capture_failed`, `Io` -> `internal`, `Backend` -> `render_failed`.
- `From<RecorderError> for adesk_core::Error` preserves the code and message.

### `src/encoder.rs`
- `Encoder` (object-safe, `Send`): `name() -> &str`,
  `encode_frame(&mut self, &ImageBuffer, ts_ms: u64) -> Result<()>`,
  `frames() -> u64`, `finish(&mut self) -> Result<()>`. Frames always arrive with
  `stride >= width * 4`; implementations must honour `stride`.

### `src/avi.rs`
- `AviWriter<W: Write + Seek>`: `new(w, width, height, fps) -> Result<_>`,
  `write_frame(&[u8])`, `frames() -> u64`, `finish()`. Streams chunks as frames
  arrive and patches every deferred size (RIFF, `movi`, `avih.dwTotalFrames`,
  `strh.dwLength`, the `*SuggestedBufferSize` fields) plus writes the trailing
  `'idx1'` index in `finish`. `finish` is idempotent.

### `src/mjpeg.rs`
- `MjpegEncoder`: `create(path, fps) -> Result<_>`, `path() -> &Path`; implements
  `Encoder` (name `"mjpeg"`). `DEFAULT_JPEG_QUALITY: u8 = 90`.

### `src/ffmpeg.rs`
- `FfmpegEncoder`: `create(path, fps) -> Result<_>`, `path() -> &Path`; implements
  `Encoder` (name `"ffmpeg/<encoder>"`). The child process is spawned lazily on
  the first frame; a hardware encoder that fails to start falls back to `libx264`.
  Closes stdin and waits the child in `finish`, reporting its exit status + stderr.
- `gpu_available()`, `ffmpeg_available()` — the cached `ffmpeg -hide_banner -encoders` probe.

### `src/recorder.rs`
- `RecorderConfig { path: PathBuf, fps: u32, encoder: EncoderKind, max_frames: Option<u64> }`
  + `new(path)`, `Default` (empty path, 30 fps, `Auto`, no cap), `with_fps`,
  `with_encoder`, `with_max_frames`.
- `Recorder`: `create(RecorderConfig) -> Result<Recorder>`, `encoder_name() -> &str`,
  `path() -> &Path`, `frames() -> u64`,
  `push_frame(&mut self, &ImageBuffer, ts_ms: u64) -> Result<()>`,
  `finish(self) -> Result<RecordingSummary>`.
- `RecordingSummary { path: PathBuf, encoder: String, frames: u64, duration_ms: u64 }`.

### `src/session.rs`
- `RecordingSession`: `start(RecorderConfig) -> Result<_>` (creates the recorder on
  the caller's thread, so an unwritable path fails here, then runs it on a
  dedicated `adesk-recorder` OS thread), `push(&self, ImageBuffer, ts_ms: u64) -> Result<()>`,
  `stats(&self) -> RecordingStats`, `encoder_name() -> &str`,
  `stop(&mut self) -> Result<RecordingSummary>` (idempotent). `Drop` finalizes.
- `RecordingStats { frames: u64, encoder: String, running: bool }`.

## Design Decisions

- **Lazy container header.** Frame dimensions are not known until the first frame,
  so both backends defer their real setup to the first `encode_frame`. The software
  backend still *creates* the output file in `create` (so an unwritable path is a
  `create` error), but writes the AVI header on the first frame; the ffmpeg backend
  spawns its child then (its `-s WxH` needs the size).
- **The first frame fixes the dimensions.** `Recorder::push_frame` records the
  first frame's size and rejects any later frame of a different size with
  `RecorderError::Encode`; both backends also enforce this locally.
- **`fps` is metadata only.** It is written into the AVI `dwRate`/`dwScale` and the
  ffmpeg `-r` argument; the recorder never sleeps. The caller timestamps frames.
- **AVI index offsets are relative to the `'movi'` FOURCC** (the first byte of the
  `'movi'` identifier), matching `ffmpeg` and the OpenDML spec, not the absolute
  file offset some writers use.
- **Encoding is streaming + seek-patched.** `AviWriter` writes frames straight to a
  `BufWriter<File>` and patches the header sizes by seeking back in `finish`, so a
  recording never buffers the whole video in memory.
- **`gpu_available()` requires a hardware encoder**, not merely `ffmpeg`. Therefore
  an explicit `EncoderKind::Gpu` request without hardware is a structured
  `Unsupported` (AGP `not_supported`) rather than a silent software fallback. The
  `libx264` branch is the backend's defensive runtime fallback if the preferred
  hardware encoder fails to *start*.
- **The encoder trait takes the RGBA `ImageBuffer`**, not pre-converted RGB: each
  backend converts as needed (MJPEG discards alpha, ffmpeg pipes RGBA), which keeps
  the stride/format handling in one place per backend.
- **`RecordingSession` reconstructs push errors but preserves the real one on
  `stop`.** The worker moves the precise `RecorderError` into a shared slot; `stop`
  returns it (correct code + message). A `push` that races a dead worker returns a
  `Backend` error carrying the recorded message, because `std::io::Error` makes
  `RecorderError` non-`Clone`.

## Constraints

- The software backend **must work headless with no GPU, display, network or external
  binary**; every test runs with none of those.
- The GPU backend degrades cleanly: `FfmpegEncoder::create` without `ffmpeg` is a
  structured `Unsupported`, never a panic; `Auto` silently selects software.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; files stay well under ~1000 lines.
- Dependencies come only from the root `[workspace.dependencies]` (`adesk-core`,
  `jpeg-encoder`, `thiserror`, `tracing`; dev: `jpeg-decoder`, `tempfile`); never add
  inline versions. The only external *tool* is an optional `ffmpeg` **process**
  (never a linked library).
- Frames arrive RGBA8 (`adesk_core::ImageBuffer`, possibly row-padded): the recorder
  honours `stride`, never assuming tightly packed rows.
- Timestamps are monotonic ms (`ts_ms`), never wall clock.

## Test Strategy

33 tests, all display/GPU/network/ffmpeg-free:

- Inline `#[cfg(test)]` unit tests (14): `avi` (zero dimensions/fps rejected, odd
  frames padded to a word, `finish` idempotent), `mjpeg`/`ffmpeg` (`rgba_to_rgb` /
  `pack_rgba` honour `stride`, reject a short stride and truncated data), `ffmpeg`
  (encoder-choice metadata, the probe never panics and is cached, `create` without
  ffmpeg is `Unsupported`), `recorder` (`RecorderConfig` defaults/builders).
- `./tests/roundtrip.rs` (3): record padded, poisoned synthetic frames, then parse
  the RIFF/AVI tree (RIFF size, `hdrl`/`strh`/`strf`/`avih` fields, `movi` size vs
  its chunks, every `idx1` offset) and decode each `00dc` JPEG with `jpeg-decoder`,
  asserting dimensions and sampled band pixels; a second test proves padded and
  tightly-packed frames decode identically (stride correctness); a third covers a
  zero-frame recording.
- `./tests/detection.rs` (5): `detect`/`gpu_available` never panic and the probe is
  cached; `Auto` resolves to `Software` here; extension mapping; an explicit `Gpu`
  request without hardware is `Unsupported`/`not_supported`; the software backend
  always creates a file.
- `./tests/session.rs` (4): push/stats/stop round trip and post-stop push failure;
  `Drop` finalizes the file; a worker error surfaces (`Encode`/`capture_failed`)
  through a later push and `stop`; `start` fails on an unwritable path.
- `./tests/errors.rs` (6): the AGP code mapping for every variant + the umbrella
  conversion; unwritable/missing-parent paths; zero fps; `max_frames`; a mid-stream
  size change; the consume-by-value `finish` contract.
- 1 doctest drives the software backend end to end.

Run with `./scripts/dev.sh cargo test -p adesk-recorder`.

## Known Issues

- The sandbox has no GPU, no `/dev/dri` and no `ffmpeg`, so the GPU backend's
  availability probe returns `false` here and its tests are detection-gated
  (skip-by-early-return), mirroring `adesk-render`'s `ADESK_TEST_GL` pattern. The
  `Auto`->`Gpu` path and the `libx264` runtime fallback are therefore not exercised
  in this environment.
- A recording finalized without calling `finish`/`stop` leaves an AVI whose header
  sizes were never patched (the `Recorder`/`FfmpegEncoder` drop paths close the
  child/file but do not patch the container).

## Routing Table

| Area | Owner |
|---|---|
| Crate root, re-exports, encoder selection (`EncoderKind`, `detect`, `suggest_extension`) | `./src/lib.rs` |
| `Recorder` / `RecorderConfig` / `RecordingSummary` / state machine | `./src/recorder.rs` |
| Encoder trait + backend dispatch | `./src/encoder.rs` |
| Software Motion-JPEG encoder | `./src/mjpeg.rs` |
| RIFF/AVI muxer + `idx1` index | `./src/avi.rs` |
| GPU-accelerated `ffmpeg` child-process encoder + probe | `./src/ffmpeg.rs` |
| Threaded `RecordingSession` / `RecordingStats` | `./src/session.rs` |
| `RecorderError`, AGP error-code mapping | `./src/error.rs` |
| Round-trip / container / detection / session / error tests | `./tests/` |

## Status

Complete: the software (MJPEG/AVI) and GPU (`ffmpeg`) backends, the `Recorder` state
machine and the threaded `RecordingSession` are implemented and tested; 33 tests
pass, `cargo clippy -p adesk-recorder --all-targets --no-deps -- -D warnings` is
clean, `cargo fmt -p adesk-recorder --check` is clean, and
`cargo doc -p adesk-recorder --no-deps --document-private-items` emits zero warnings.
`adesk-server` is expected to consume `RecordingSession`; wiring capture into the
server is not part of this crate.
