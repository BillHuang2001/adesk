# adesk-viewer-proto — Viewer Attachment Protocol (VAP) v1 wire types + codec
## Intent
`adesk-viewer-proto` is the single implementation of `docs/viewer.md` (normative VAP v1).
It defines the messages a Viewer and the ADesk runtime exchange over a viewer connection, the NDJSON codec, and the version helpers.
It is pure serialization: no I/O, no async, no tokio, no transport — so the same vocabulary drives a Unix socket, a TCP stream or an in-memory pipe.
`adesk-viewer` (server session + client) and `adesk-server` (the runtime endpoint) build on exactly this surface.
## API Surface
Crate root (`src/lib.rs`): the version helpers and every public type below, re-exported flat (`adesk_viewer_proto::<Name>`).
- `PROTOCOL_VERSION: u32 = 1`, `is_compatible_version(u32) -> bool` (const), `check_version(u32) -> Result<()>` (mismatch → `ViewerProtoError::VersionMismatch`).
- `DEFAULT_MIN_INTERVAL_MS: u64 = 100`, `DEFAULT_OVERLAYS: &[OverlayKind]` (the `docs/viewer.md` default overlay set).
Messages (`src/message.rs`):
- `ClientMessage` (viewer → server), tagged by `"type"`: `Hello(ViewerHello)`, `RequestFrame { id: Option<u64> }`, `RequestState { id: Option<u64> }`, `PointerMove { x: f64, y: f64 }`, `PointerButton { button: Button, state: ButtonState, x: Option<f64>, y: Option<f64> }`, `Scroll { dx: f64, dy: f64, x: Option<f64>, y: Option<f64> }`, `Key { keys: KeySpec, action: KeyAction }`, `Text { text: String }`, `SetControl { owner: ControlOwner }`, `Bye { reason: Option<String> }`, and `Unknown { message_type: String, value: serde_json::Value }` (forward compatibility).
- `ServerMessage` (server → viewer), tagged by `"type"`: `Hello(ServerHello)`, `Frame(ViewerFrame)`, `State(DesktopState)`, `Control { owner: ControlOwner }`, `InputAck { id: Option<u64>, action_id: ActionId }`, `Error { code: ErrorCode, message: String, id: Option<u64> }`, `Bye { reason: String }`, and `Unknown { .. }`.
- Both enums carry `message_type() -> &str` (the wire tag) and are `PartialEq`.
Types (`src/types.rs`):
- `ViewerHello { protocol_version: u32, client: Option<String>, overlays: Vec<OverlayKind>, min_interval_ms: u64 }` (`new()` fills the defaults).
- `ServerHello { protocol_version: u32, runtime_version: String, output: Size, renderer: RendererKind, cursor: CursorState, control: ControlOwner }`.
- `CursorState { x: f64, y: f64, visible: bool }` — `x`/`y` are normalized `0.0..=1.0` output fractions; `hidden()`/`at(x, y)`.
- `ControlOwner { Ai, Human }` — serde `"ai"`/`"human"`.
- `ViewerFrame { seq: u64, ts_ms: u64, image: ImagePayload, cursor: CursorState, active_window_id: Option<WindowId> }`.
- `DesktopState { active_window_id: Option<WindowId>, windows: Vec<WindowInfo> }`.
- `KeyAction { Pressed, Released, Tap }` — serde `"pressed"`/`"released"`/`"tap"`.
Codec (`src/codec.rs`):
- `encode_client(&ClientMessage) -> String`, `encode_server(&ServerMessage) -> String` (infallible; a message the codec itself produced always serializes).
- `decode_client(&str) -> Result<ClientMessage>`, `decode_server(&str) -> Result<ServerMessage>` — read the `"type"` tag, dispatch to the variant, and return `Unknown` for an unrecognized tag (never an error).
Errors (`src/error.rs`): `ViewerProtoError { Malformed, Unknown { .. } (internal), InvalidParams { message }, VersionMismatch { client, server }, Json(serde_json::Error) }` + `error_code() -> ErrorCode` (reuses the AGP vocabulary: mismatch → `ProtocolVersionMismatch`, everything else → `InvalidRequest`) + `pub type Result<T>`.
## Constraints
- `docs/viewer.md` is normative: never invent a message, field or default; additive changes only (§7).
- Pure serialization: no tokio, no async, no I/O, no Smithay in this crate.
- Reuse the shared vocabulary from `adesk-core` (`WindowId`, `Size`, `Button`, `ButtonState`, `OverlayKind`, `ErrorCode`) and `adesk-proto` (`ImagePayload`, `KeySpec`, `RendererKind`); depend on them, never fork them.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; every public item documented.
- Dependencies come only from root `[workspace.dependencies]` (`adesk-core`, `adesk-proto`, `serde`, `serde_json`, `thiserror`); never add inline versions.
- Decoding must ignore unknown fields (forward compatibility) and never panic; malformed input returns `ViewerProtoError`.
- Files stay well under the ~1000-line threshold; split along module boundaries.
## Routing Table
| Area | Owner |
|---|---|
| Crate root, version helpers, re-exports | `./src/lib.rs` |
| `ClientMessage` / `ServerMessage` enums, tag dispatch | `./src/message.rs` |
| Handshake/frame/state/cursor/control types | `./src/types.rs` |
| Codec (`encode_*`/`decode_*`) | `./src/codec.rs` |
| `ViewerProtoError`, AGP error-code mapping | `./src/error.rs` |
| Golden-JSON + round-trip tests | `./tests/wire.rs` |
| Codec acceptance (unknown type, malformed, unknown fields) | `./tests/codec.rs` |
## Design Decisions
- **Frames reuse the AGP `ImagePayload`.** The runtime encodes a frame once (`adesk-server::images`) and both AGP and VAP carry the same base64 shape; the viewer decodes with `ImagePayload::decode_data`. This is the crate's only `adesk-proto` coupling beyond `KeySpec`/`RendererKind`.
- **Unknown message types are a variant, not an error.** `Unknown { message_type, value }` keeps a future viewer/protocol extension from breaking an older peer, mirroring `adesk_client::AgpEvent::Other`. `decode_*` must therefore not use `#[serde(tag)]` alone (which fails on an unknown tag) — dispatch on the raw `"type"` string.
- **Coordinates are normalized.** `CursorState` and every pointer message use `0.0..=1.0` of the output; the server resolves them through the window model. Pixels never cross VAP.
- **`id` is optional and echoed.** Viewer messages may carry a client `id`; the matching `InputAck`/`Error` echoes it so a viewer can correlate. `None` is always valid (fire-and-forget).
- **Defaults live here.** `DEFAULT_OVERLAYS` and `DEFAULT_MIN_INTERVAL_MS` are this crate's constants (the server must not redefine them), matching how `adesk-proto` owns AGP defaults.
## Test Strategy
Integration tests in `./tests/`, no display/GPU/network/socket:
- `tests/wire.rs` — golden JSON for every `ClientMessage` and `ServerMessage`, the `"type"` tags, `ControlOwner`/`KeyAction` wire names, defaults, and `PROTOCOL_VERSION` helpers.
- `tests/codec.rs` — round-trip every message; unknown `"type"` → `Unknown`; unknown fields ignored; malformed JSON / missing `type` → `Malformed`; version mismatch → `VersionMismatch` with the right `error_code()`.
- Run with `./scripts/dev.sh cargo test -p adesk-viewer-proto` (bare `cargo` cannot link outside the Nix dev shell).
## Notes for Agents
- The framing (NDJSON, one object per line, 32 MiB cap) belongs to the transport, not this crate; `encode_*` return line-ready strings without a trailing newline (mirror `adesk-proto::encode_frame`).
- Do not add `ImageFormat`-style closed enums without a catch-all: the viewer must stay forward-compatible.
## Status
Skeleton only: `src/lib.rs` carries the crate attributes and docs; every module above is **not yet implemented**. Implement the surface exactly as documented here.
