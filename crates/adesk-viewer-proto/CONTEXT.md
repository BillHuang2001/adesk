# adesk-viewer-proto — Viewer Attachment Protocol (VAP) v1 wire types + codec
## Intent
`adesk-viewer-proto` is the single implementation of `docs/viewer.md` (normative VAP v1).
It defines the messages a Viewer and the ADesk runtime exchange over a viewer connection, the NDJSON codec, and the version helpers.
It is pure serialization: no I/O, no async, no tokio, no transport — so the same vocabulary drives a Unix socket, a TCP stream or an in-memory pipe.
`adesk-viewer` (server session + client) and `adesk-server` (the runtime endpoint) build on exactly this surface.
## API Surface
Crate root (`src/lib.rs`): the version helpers and every public type below, re-exported flat (`adesk_viewer_proto::<Name>`).
- `PROTOCOL_VERSION: u32 = 1`, `is_compatible_version(u32) -> bool` (const), `check_version(u32) -> Result<()>` (mismatch → `ViewerProtoError::VersionMismatch { client, server }`).
- `DEFAULT_MIN_INTERVAL_MS: u64 = 100`, `DEFAULT_OVERLAYS: &[OverlayKind]` = `[WindowIds, Focus, Damage]` (the `docs/viewer.md` §2 default set, in order), `DEFAULT_RECORD_FPS: u32 = 30` (the `start_recording` decode default, §4).
Messages (`src/message.rs`):
- `ClientMessage` (viewer → server), tagged by `"type"`: `Hello(ViewerHello)`, `RequestFrame { id: Option<u64> }`, `RequestState { id: Option<u64> }`, `PointerMove { x: f64, y: f64 }`, `PointerButton { button: Button, state: ButtonState, x: Option<f64>, y: Option<f64> }`, `Scroll { dx: f64, dy: f64, x: Option<f64>, y: Option<f64> }`, `Key { keys: KeySpec, action: KeyAction }`, `Text { text: String }`, `ActivateWindow { window_id: WindowId }`, `SetControl { owner: ControlOwner }`, `Bye { reason: Option<String> }`, `StartRecording { id: Option<u64>, path: Option<String>, fps: u32, encoder: RecordingEncoder }`, `StopRecording { id: Option<u64> }`, `RequestRecording { id: Option<u64> }`, `ListApps { id: Option<u64>, query: Option<String> }`, `LaunchApp { id: Option<u64>, app_id: AppId }`, `CloseWindow { window_id: WindowId }`, and `Unknown { message_type: String, value: serde_json::Value }` (forward compatibility).
- `ServerMessage` (server → viewer), tagged by `"type"`: `Hello(ServerHello)`, `Frame(ViewerFrame)`, `State(DesktopState)`, `Control { owner: ControlOwner }`, `InputAck { id: Option<u64>, action_id: ActionId }`, `Error { code: ErrorCode, message: String, id: Option<u64> }`, `Recording { id: Option<u64>, status: RecordingStatus }`, `Apps { id: Option<u64>, apps: Vec<AppEntry> }`, `LaunchResult { id: Option<u64>, result: LaunchOutcome }`, `Bye { reason: String }`, and `Unknown { message_type, value }`.
- Both enums carry `message_type() -> &str` (the wire tag; `Unknown` returns its stored `message_type`), derive `Debug, Clone, PartialEq`, and implement `Serialize`/`Deserialize` manually (see Design Decisions).
- Wire tags: `"hello"`, `"request_frame"`, `"request_state"`, `"pointer_move"`, `"pointer_button"`, `"scroll"`, `"key"`, `"text"`, `"activate_window"`, `"set_control"`, `"bye"`, `"start_recording"`, `"stop_recording"`, `"request_recording"`, `"list_apps"`, `"launch_app"`, `"close_window"`; server adds `"frame"`, `"state"`, `"control"`, `"input_ack"`, `"error"`, `"recording"`, `"apps"`, `"launch_result"` (and reuses `"hello"`/`"bye"` in the other direction).
Types (`src/types.rs`):
- `ViewerHello { protocol_version: u32, client: Option<String>, overlays: Vec<OverlayKind>, min_interval_ms: u64 }` (`new()`/`Default` fill the crate defaults; `client` is `#[serde(default)]`).
- `ServerHello { protocol_version, runtime_version: String, output: Size, renderer: RendererKind, cursor: CursorState, control: ControlOwner }`.
- `CursorState { x: f64, y: f64, visible: bool }` — normalized `0.0..=1.0` output fractions; `hidden()` / `at(x, y)` (visible).
- `ControlOwner { Ai, Human }` — serde `"ai"`/`"human"`.
- `ViewerFrame { seq: u64, ts_ms: u64, image: ImagePayload, cursor: CursorState, active_window_id: Option<WindowId> }`.
- `DesktopState { active_window_id: Option<WindowId>, windows: Vec<WindowInfo> }`.
- `KeyAction { Pressed, Released, Tap }` — serde `"pressed"`/`"released"`/`"tap"`.
- `RecordingEncoder { Auto, Software, Gpu }` — serde `"auto"`/`"software"`/`"gpu"`; `Default` is `Auto`.
- `RecordingStatus { recording: bool, path: Option<String>, encoder: Option<String>, fps: u32, frames: u64, duration_ms: u64, error: Option<String> }` — `idle()` (`recording = false`, `fps = DEFAULT_RECORD_FPS`, zero counters, no path/encoder/error), `Default = idle()`, and `with_recording`/`with_path`/`with_encoder`/`with_counts`/`with_error` builders. `path`/`encoder`/`error` are `skip_serializing_if = None` (omitted from the wire form, never `null`).
- `AppEntry { id: AppId, name: String, icon: Option<String>, categories: Vec<String> }` — one launchable application (the runtime's XDG registry projection). `icon` is `skip_serializing_if = None`; `categories` is always serialized.
- `LaunchOutcome { app_id: AppId, launch_id: LaunchId, action_id: Option<ActionId>, window_id: Option<WindowId> }` — the result of a launch. `action_id`/`window_id` are `skip_serializing_if = None`.
Codec (`src/codec.rs`):
- `encode_client(&ClientMessage) -> String`, `encode_server(&ServerMessage) -> String` (infallible; line-ready, no trailing newline).
- `decode_client(&str) -> Result<ClientMessage>`, `decode_server(&str) -> Result<ServerMessage>` — read the raw `"type"` tag, dispatch to the variant, and return `Unknown` for an unrecognised tag (never an error).
Errors (`src/error.rs`): `ViewerProtoError { Malformed, Unknown { message_type }, InvalidParams { message }, VersionMismatch { client, server }, Json(serde_json::Error) }` + `error_code() -> ErrorCode` (mismatch → `ProtocolVersionMismatch`, everything else → `InvalidRequest`) + `pub type Result<T>`.
## Constraints
- `docs/viewer.md` is normative: never invent a message, field or default; additive changes only (§7).
- Pure serialization: no tokio, no async, no I/O, no Smithay in this crate.
- Reuse the shared vocabulary from `adesk-core` (`WindowId`, `ActionId`, `Size`, `Button`, `ButtonState`, `OverlayKind`, `ErrorCode`, `WindowInfo`) and `adesk-proto` (`ImagePayload`, `KeySpec`, `RendererKind`); depend on them, never fork them.
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
| Shared message fixtures (not a test target) | `./tests/common/mod.rs` |
## Design Decisions
- **Tag dispatch is manual, never `#[serde(tag = "type")]`.** Serde's internally-tagged impl errors on an unknown tag, but §1 requires unknown types to be ignored. Both enums implement `Serialize`/`Deserialize` by hand over a `serde_json::Value`: `to_value()` builds the flat object form (`{"type": ..., <fields>}`), and `from_value()` reads the raw `"type"` string, dispatching a recognised tag to its typed payload and any other tag to the `Unknown` variant. All decode entry points route through `ClientMessage::from_value`/`ServerMessage::from_value` (crate-internal).
- **`key`'s wire field is `state`.** `docs/viewer.md` §4 names it `state`; the Rust field is `action` (`Key { keys, action }`). The builder emits `"state"` and the decode helper renames it, so the on-wire shape matches the normative spec while the Rust API stays ergonomic.
- **`DesktopState` follows this file, not §3's illustrative `focus` field.** §3's example lists `focus`, but focus is carried per-window via `WindowInfo.state` plus `active_window_id`; the sibling `adesk-viewer` client expects exactly `DesktopState { active_window_id, windows }`.
- **Optional wire fields are omitted when absent.** `id?`, the client `bye.reason?` and the `x?`/`y?` on `pointer_button`/`scroll` are dropped from the encoded object when `None` (never emitted as `null`); decoding defaults a missing field back to `None`. `active_window_id` is a required, nullable field on `frame`/`state` and serializes as explicit `null`.
- **Frames reuse the AGP `ImagePayload`.** The runtime encodes a frame once (`adesk-server::images`) and both AGP and VAP carry the same base64 shape; the viewer decodes with `ImagePayload::decode_data`. This is the crate's only `adesk-proto` coupling beyond `KeySpec`/`RendererKind`.
- **Unknown message types are a variant, not an error.** `Unknown { message_type, value }` keeps a future viewer/protocol extension from breaking an older peer. `decode_client` recognises only client tags and `decode_server` only server tags, so a server-only tag (or vice versa) is `Unknown` in the other direction. Re-encoding an `Unknown` emits its stored object verbatim (JSON key order is not preserved — `serde_json::Map` is key-sorted).
- **Encoding is infallible by construction.** `encode_*` return the message's `Value` form via `Value::to_string()` (an infallible `Display`); the only JSON-unrepresentable value, a non-finite float, is outside the normalized-coordinate contract.
- **Malformed JSON maps to `Malformed`, not `Json`.** The decode path maps every JSON-syntax error, non-object payload, missing/non-string `"type"` and schema mismatch to `ViewerProtoError::Malformed`. `Json` exists as the `From<serde_json::Error>` target and `Unknown`/`InvalidParams` complete the documented error surface; the wire decoder produces none of those three.
- **Coordinates are normalized.** `CursorState` and every pointer message use `0.0..=1.0` of the output; the server resolves them through the window model. Pixels never cross VAP.
- **`id` is optional and echoed.** Viewer messages may carry a client `id`; the matching `InputAck`/`Error` echoes it so a viewer can correlate. `None` is always valid (fire-and-forget).
- **Defaults live here.** `DEFAULT_OVERLAYS` and `DEFAULT_MIN_INTERVAL_MS` are this crate's constants (the server must not redefine them); `ViewerHello::new()` fills them and `ViewerHello` also implements `Default` (same value) so `clippy::new_without_default` stays satisfied.
- **`ViewerProtoError` is not `PartialEq`.** It carries a `serde_json::Error`; tests match on the variant (`matches!`) rather than comparing values.
- **Recording is additive (§7).** `start_recording`/`stop_recording`/`request_recording` and the server `recording` reply are additive to VAP v1 (`PROTOCOL_VERSION` stays 1). The reply echoes the client `id` (like `input_ack`/`error`). `start_recording`'s `fps`/`encoder` are optional on the wire and default on decode to `DEFAULT_RECORD_FPS` / `RecordingEncoder::Auto`; the Rust struct keeps them non-optional, so encoding always emits them. On `recording`, `recording`/`fps`/`frames`/`duration_ms` are required (missing → `Malformed`) while `id`/`path`/`encoder`/`error` are omitted when `None`, never `null`. `RecordingStatus::encoder` is a free-form `String` (what the runtime actually used), distinct from the request-side `RecordingEncoder` enum.
## Test Strategy
Integration tests in `./tests/`, no display/GPU/network/socket; run with `./scripts/dev.sh cargo test -p adesk-viewer-proto` (bare `cargo` cannot link outside the Nix dev shell).
- `tests/wire.rs` (26) — golden JSON for every `ClientMessage`/`ServerMessage` variant (comparing parsed `Value`s, so key order is irrelevant), the exact `"type"` tags, `ControlOwner`/`KeyAction`/`RecordingEncoder` wire names, `CursorState` constructors, `RecordingStatus::idle`/`Default` and builder output, defaults, and the version helpers (`protocol_version_is_one_and_checked` alone asserts `PROTOCOL_VERSION == 1`, `is_compatible_version` and the `VersionMismatch` error code). Golden tests use the shared constructors where an instance matches and keep file-specific values inline (e.g. the `client_hello_golden` hello, the `Right` `pointer_button`, the `Pressed` key chord, the `activate_window` message, the `Gpu` `start_recording`, the `mystery` tags).
- `tests/codec.rs` (16) — round-trip every message both through the free `encode_*`/`decode_*` functions and through the serde impls; single-line-object checks; unknown `"type"` → `Unknown` (both directions, incl. a cross-direction tag); unknown fields ignored (top level and nested payload, incl. the recording messages); `start_recording` defaults its absent `fps`/`encoder` while honouring present ones; malformed JSON / non-object / missing or non-string `type` / schema mismatch (incl. `recording` missing a required counter, an unknown `encoder`) → `Malformed`; version mismatch → `VersionMismatch` with `error_code() == ProtocolVersionMismatch`.
- One doctest in `lib.rs` shows an `encode_client`/`decode_client` round-trip.
- `tests/common/mod.rs` — shared fixtures, **not a test target**: each test file declares `mod common;`. It holds the fixture literals both test files need: `client()`/`server()` (serialize through the `Serialize` impls — the local analog of `adesk-proto`'s `wire<T: Serialize>` helper), `sample_window()` (window 17 / `org.mozilla.firefox` / "GitHub" / 1280x800), small named constructors for the message instances both files build (`hello_message()`, `pointer_move_message()`, `state_with_window()`, `recording_message()`, `unknown_server_message()`, ...), and the `every_client_message()`/`every_server_message()` round-trip corpora (assembled from those constructors plus the corpus-only variants no other test shares). It opens with `#![allow(dead_code)]` because each target uses only a subset.
- No test here defines transport plumbing: no `read_line`/`write_line`, no `BufReader`/socket/duplex, no fake peer, no line-splitting helper (NDJSON framing is transport-owned, e.g. `adesk-viewer::transport`).
- `tests/wire.rs` exercises the `Serialize` impls directly (`serde_json::to_value`) and never calls `encode_*`/`decode_*`; only `tests/codec.rs` does.
- Total: 26 + 16 + 1 doctest = **43 tests**.
- There is no `image_payload()`/`observation()`/`damage()`/`app_info()`/`rect()`/`size()`/`position()` builder: `ImagePayload`, `Rect`, `Size` and `KeySpec` are built inline at each call site (inside `tests/common/mod.rs` or the test that needs a file-specific value).
- `wire.rs` re-pins (via the sibling crates' own serde impls, so byte-identical) the AGP/core shapes it embeds: `ImagePayload` `{"width","height","format","stride","data","scale"}`, `Rect` `{"x","y","w","h"}`, `Size` `{"w","h"}`, `KeySpec` single/chord, `RendererKind`/`ImageFormat` and `WindowInfo`. It does NOT pin `Position`, `Condition`, `app_launched`/`RuntimeEvent` payloads or any AGP frame/envelope — this crate carries no event layer.
## Notes for Agents
- The framing (NDJSON, one object per line, 32 MiB cap) belongs to the transport, not this crate; `encode_*` return line-ready strings without a trailing newline (mirror `adesk-proto::encode_frame`).
- Do not add `ImageFormat`-style closed enums without a catch-all: the viewer must stay forward-compatible.
- `ViewerHello.client` is `#[serde(default)]`; every other handshake/frame/state field is required on the wire (the optional `id`/`x`/`y`/`bye.reason` are omitted when `None`, never `null`, as are `RecordingStatus`'s `path`/`encoder`/`error` and the `start_recording`/`stop_recording`/`request_recording`/`recording` `id`).

## Known Issues
- Two `ViewerProtoError` variants are unreachable in the whole workspace: `Json` (every decode path maps a JSON failure to `Malformed`, and no crate `?`s a `serde_json::Error` into this type) and `Unknown` (constructed only by `./tests/codec.rs`); `dead_code` never fires on a public enum variant, so nothing flags them.
## Status
Implemented and green: `src/lib.rs`, `src/error.rs`, `src/types.rs`, `src/message.rs`, `src/codec.rs`.
`./scripts/dev.sh cargo test -p adesk-viewer-proto` = 43 passed / 0 failed (26 `tests/wire.rs`, 16 `tests/codec.rs`, 1 doctest), with the duplicated fixtures consolidated into the shared `tests/common/mod.rs` module.
`cargo clippy -p adesk-viewer-proto --all-targets --no-deps -- -D warnings`, `cargo fmt -p adesk-viewer-proto --check` and
`cargo doc -p adesk-viewer-proto --no-deps --document-private-items` are all clean (zero warnings).
No `todo!()`/`unimplemented!()`, no crate-level `allow`, no panics on the decode path.
