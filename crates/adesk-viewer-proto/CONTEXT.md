# adesk-viewer-proto — Viewer Attachment Protocol (VAP) v1 wire types + codec
## Intent
`adesk-viewer-proto` is the single implementation of `docs/viewer.md` (normative VAP v1).
It defines the messages a Viewer and the ADesk runtime exchange over a viewer connection, the NDJSON codec, and the version helpers.
It is pure serialization: no I/O, no async, no tokio, no transport — so the same vocabulary drives a Unix socket, a TCP stream or an in-memory pipe.
`adesk-viewer` (server session + client) and `adesk-server` (the runtime endpoint) build on exactly this surface.
## API Surface
Crate root (`src/lib.rs`): the version helpers and every public type below, re-exported flat (`adesk_viewer_proto::<Name>`).
- `PROTOCOL_VERSION: u32 = 1`, `is_compatible_version(u32) -> bool` (const), `check_version(u32) -> Result<()>` (mismatch → `ViewerProtoError::VersionMismatch { client, server }`).
- `DEFAULT_MIN_INTERVAL_MS: u64 = 100`, `DEFAULT_OVERLAYS: &[OverlayKind]` = `[WindowIds, Focus, Damage]` (the `docs/viewer.md` §2 default set, in order).
Messages (`src/message.rs`):
- `ClientMessage` (viewer → server), tagged by `"type"`: `Hello(ViewerHello)`, `RequestFrame { id: Option<u64> }`, `RequestState { id: Option<u64> }`, `PointerMove { x: f64, y: f64 }`, `PointerButton { button: Button, state: ButtonState, x: Option<f64>, y: Option<f64> }`, `Scroll { dx: f64, dy: f64, x: Option<f64>, y: Option<f64> }`, `Key { keys: KeySpec, action: KeyAction }`, `Text { text: String }`, `SetControl { owner: ControlOwner }`, `Bye { reason: Option<String> }`, and `Unknown { message_type: String, value: serde_json::Value }` (forward compatibility).
- `ServerMessage` (server → viewer), tagged by `"type"`: `Hello(ServerHello)`, `Frame(ViewerFrame)`, `State(DesktopState)`, `Control { owner: ControlOwner }`, `InputAck { id: Option<u64>, action_id: ActionId }`, `Error { code: ErrorCode, message: String, id: Option<u64> }`, `Bye { reason: String }`, and `Unknown { message_type, value }`.
- Both enums carry `message_type() -> &str` (the wire tag; `Unknown` returns its stored `message_type`), derive `Debug, Clone, PartialEq`, and implement `Serialize`/`Deserialize` manually (see Design Decisions).
- Wire tags: `"hello"`, `"request_frame"`, `"request_state"`, `"pointer_move"`, `"pointer_button"`, `"scroll"`, `"key"`, `"text"`, `"set_control"`, `"bye"`; server adds `"frame"`, `"state"`, `"control"`, `"input_ack"`, `"error"` (and reuses `"hello"`/`"bye"` in the other direction).
Types (`src/types.rs`):
- `ViewerHello { protocol_version: u32, client: Option<String>, overlays: Vec<OverlayKind>, min_interval_ms: u64 }` (`new()`/`Default` fill the crate defaults; `client` is `#[serde(default)]`).
- `ServerHello { protocol_version, runtime_version: String, output: Size, renderer: RendererKind, cursor: CursorState, control: ControlOwner }`.
- `CursorState { x: f64, y: f64, visible: bool }` — normalized `0.0..=1.0` output fractions; `hidden()` / `at(x, y)` (visible).
- `ControlOwner { Ai, Human }` — serde `"ai"`/`"human"`.
- `ViewerFrame { seq: u64, ts_ms: u64, image: ImagePayload, cursor: CursorState, active_window_id: Option<WindowId> }`.
- `DesktopState { active_window_id: Option<WindowId>, windows: Vec<WindowInfo> }`.
- `KeyAction { Pressed, Released, Tap }` — serde `"pressed"`/`"released"`/`"tap"`.
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
## Test Strategy
Integration tests in `./tests/`, no display/GPU/network/socket; run with `./scripts/dev.sh cargo test -p adesk-viewer-proto` (bare `cargo` cannot link outside the Nix dev shell).
- `tests/wire.rs` (21) — golden JSON for every `ClientMessage`/`ServerMessage` variant (comparing parsed `Value`s, so key order is irrelevant), the exact `"type"` tags, `ControlOwner`/`KeyAction` wire names, `CursorState` constructors, defaults, and the version helpers (`protocol_version_is_one_and_checked` alone asserts `PROTOCOL_VERSION == 1`, `is_compatible_version` and the `VersionMismatch` error code). Golden tests use the shared constructors where an instance matches and keep file-specific values inline (e.g. the `client_hello_golden` hello, the `Right` `pointer_button`, the `Pressed` key chord, the `mystery` tags).
- `tests/codec.rs` (14) — round-trip every message both through the free `encode_*`/`decode_*` functions and through the serde impls; single-line-object checks; unknown `"type"` → `Unknown` (both directions, incl. a cross-direction tag); unknown fields ignored (top level and nested payload); malformed JSON / non-object / missing or non-string `type` / schema mismatch → `Malformed`; version mismatch → `VersionMismatch` with `error_code() == ProtocolVersionMismatch`.
- One doctest in `lib.rs` shows an `encode_client`/`decode_client` round-trip.
- The only test scaffolding is in-file: `tests/codec.rs`'s `every_client_message()`/`every_server_message()` fixture vectors and `tests/wire.rs`'s `client()`/`server()` (`serde_json::to_value`) + `sample_window()` builders.
- No test here defines transport plumbing: no `read_line`/`write_line`, no `BufReader`/socket/duplex, no fake peer, no line-splitting helper (NDJSON framing is transport-owned, e.g. `adesk-viewer::transport`).
- `tests/wire.rs` exercises the `Serialize` impls directly (`serde_json::to_value`) and never calls `encode_*`/`decode_*`; only `tests/codec.rs` does.
- Total: 21 + 14 + 1 doctest = **36 tests**.
- There is no `image_payload()`/`observation()`/`damage()`/`app_info()`/`rect()`/`size()`/`position()` builder: `ImagePayload`, `Rect`, `Size` and `KeySpec` are built inline at each call site (inside `tests/common/mod.rs` or the test that needs a file-specific value).
- `wire.rs` re-pins (via the sibling crates' own serde impls, so byte-identical) the AGP/core shapes it embeds: `ImagePayload` `{"width","height","format","stride","data","scale"}`, `Rect` `{"x","y","w","h"}`, `Size` `{"w","h"}`, `KeySpec` single/chord, `RendererKind`/`ImageFormat` and `WindowInfo`. It does NOT pin `Position`, `Condition`, `app_launched`/`RuntimeEvent` payloads or any AGP frame/envelope — this crate carries no event layer.
## Notes for Agents
- The framing (NDJSON, one object per line, 32 MiB cap) belongs to the transport, not this crate; `encode_*` return line-ready strings without a trailing newline (mirror `adesk-proto::encode_frame`).
- Do not add `ImageFormat`-style closed enums without a catch-all: the viewer must stay forward-compatible.
- `ViewerHello.client` is `#[serde(default)]`; every other handshake/frame/state field is required on the wire (the optional `id`/`x`/`y`/`bye.reason` are omitted when `None`, never `null`).
- `tests/` has no `common/` module (unlike `adesk-client`, `adesk-server`, `adesk-observer`, `adesk-inspector`), so `wire.rs` and `codec.rs` each rebuild the same fixtures: the `WindowInfo` literal (window 17 / firefox / GitHub / 1280x800) appears at `tests/wire.rs:26` and inline at `tests/codec.rs:80`, and ~14 message instances (`ServerHello`, `ViewerFrame`, `DesktopState`, `PointerMove`, `Key`, `Bye`, `Unknown`, `InputAck`, `Error`, `Control`, ...) are constructed identically in both files; `codec.rs`'s `every_client_message`/`every_server_message` corpora could be the single source. `adesk-proto/tests/` has the same gap and re-declares an identical `window_info()`/`wire()` helper.

## Known Issues
- Two `ViewerProtoError` variants are unreachable in the whole workspace: `Json` (every decode path maps a JSON failure to `Malformed`, and no crate `?`s a `serde_json::Error` into this type) and `Unknown` (constructed only by `./tests/codec.rs`); `dead_code` never fires on a public enum variant, so nothing flags them.
- `serde_json` is declared in both `[dependencies]` and `[dev-dependencies]`; the dev-dependency entry is redundant (integration tests already see normal dependencies — `adesk-proto` has no `[dev-dependencies]` and its tests use `serde_json`).
## Status
Implemented and green: `src/lib.rs`, `src/error.rs`, `src/types.rs`, `src/message.rs`, `src/codec.rs`.
`./scripts/dev.sh cargo test -p adesk-viewer-proto` = 38 passed / 0 failed (23 `tests/wire.rs`, 14 `tests/codec.rs`, 1 doctest);
`cargo clippy -p adesk-viewer-proto --all-targets --no-deps -- -D warnings`, `cargo fmt -p adesk-viewer-proto --check` and
`cargo doc -p adesk-viewer-proto --no-deps --document-private-items` are all clean (zero warnings).
No `todo!()`/`unimplemented!()`, no crate-level `allow`, no panics on the decode path.
