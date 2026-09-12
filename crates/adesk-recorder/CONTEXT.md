# adesk-recorder — screen recording for the ADesk runtime

## Intent

`adesk-recorder` turns the runtime's rendered desktop frames into an encoded video
file. It is the *encoder + muxer* half of screen recording; the capture half
(pulling frames out of the compositor on a schedule) belongs to `adesk-server`,
which drives this crate.

The crate is deliberately decoupled: no compositor, no Smithay, no VAP/AGP protocol
types, no async runtime. Its input is a CPU-side `adesk_core::ImageBuffer` plus a
monotonic `ts_ms`; its output is a video file on disk.

Two encoder backends exist:

- **Software** (always available, default fallback): a pure-Rust Motion-JPEG
  encoder (`jpeg-encoder`) writing an AVI container. No GPU, display, C toolchain
  or external process — so it works in every environment, including CI.
- **GPU-accelerated** (used when available): an external `ffmpeg` process driving a
  hardware H.264 encoder (`h264_vaapi` / `h264_nvenc` / `h264_v4l2m2m`), falling
  back to `libx264`. Frames are piped to its stdin as raw RGBA. This is the
  "GPU accelerated stack if possible" path: it is detected at runtime and only
  selected when the platform actually provides a hardware encoder.

## API Surface

Crate root (`src/lib.rs`) re-exports every public item below.

- `EncoderKind { Auto, Software, Gpu }` (`Default = Auto`) — the requested backend.
  `Auto` resolves to `Gpu` when a hardware encoder is detected, else `Software`.
- `RecorderConfig { path: PathBuf, fps: u32, encoder: EncoderKind, max_frames: Option<u64> }`
  (`new(path)`, `Default`, `with_*` builders) — how a recording is created.
- `RecorderError` (`thiserror`) + `pub type Result<T>`: `Io`, `Encode`, `Unsupported`,
  `Mux`, `Backend(String)`; `code() -> ErrorCode` maps to AGP codes
  (`Unsupported` -> `not_supported`, `Io`/`Backend` -> `internal`/`render_failed`,
  `Encode`/`Mux` -> `capture_failed`).
- `Recorder` — `create(RecorderConfig) -> Result<Recorder>`, `encoder_name() -> &str`,
  `push_frame(&mut self, &ImageBuffer, ts_ms: u64) -> Result<()>`, `frames() -> u64`,
  `finish(self) -> Result<RecordingSummary>`.
- `RecordingSummary { path: PathBuf, encoder: String, frames: u64, duration_ms: u64 }`.
- `detect(kind: EncoderKind) -> EncoderKind` and `gpu_available() -> bool` — runtime
  probing used by `Auto` and by `adesk-server`'s `translate` layer.
- (Optional convenience) `RecordingSession` — a `Recorder` owned by a dedicated OS
  thread with a channel-based `push`/`stop`, so an async caller never blocks on an
  encode. `adesk-server` uses this.

## Constraints

- The software backend **must work headless with no GPU, display, network or external
  binary**; every test must run with none of those.
- The GPU backend must degrade cleanly: requesting `Gpu` with no hardware encoder is a
  structured `Unsupported` error, never a panic; `Auto` silently falls back to software.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; files stay well under ~1000 lines.
- Dependencies come only from the root `[workspace.dependencies]` (`adesk-core`,
  `jpeg-encoder`, `thiserror`, `tracing`); never add inline versions. No `tools` beyond
  an optional `ffmpeg` **process** (never a linked library).
- Frames arrive RGBA8 (`adesk_core::ImageBuffer`, possibly row-padded): the recorder
  must honour `stride`, not assume tightly packed rows.
- Timestamps are monotonic ms (`ts_ms`), never wall clock.

## Routing Table

| Area | Owner |
|---|---|
| Crate root, re-exports, encoder selection (`EncoderKind`, `detect`) | `./src/lib.rs` |
| `Recorder` / `RecorderConfig` / state machine + frame pacing | `./src/recorder.rs` |
| Encoder trait + backend dispatch | `./src/encoder.rs` |
| Software Motion-JPEG encoder + AVI muxer | `./src/mjpeg.rs`, `./src/avi.rs` |
| GPU-accelerated `ffmpeg` child-process encoder | `./src/ffmpeg.rs` |
| Optional threaded `RecordingSession` | `./src/session.rs` |
| `RecorderError`, AGP error-code mapping | `./src/error.rs` |
| Round-trip / container / detection tests | `./tests/` |

## Known Issues

- The sandbox has no GPU and no `ffmpeg`; the GPU backend's tests are detection-gated
  (skip-by-early-return when no hardware encoder is present), mirroring
  `adesk-render`'s `ADESK_TEST_GL` pattern.

## Status

Scaffold only: the crate manifest and an empty `lib.rs` exist so the workspace
resolves. All modules and tests are pending.
