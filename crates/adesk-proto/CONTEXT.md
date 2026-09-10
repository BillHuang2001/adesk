# adesk-proto — AGP v1 wire types, typed methods and NDJSON codec

## Intent

`adesk-proto` is the single implementation of `docs/protocol.md` (normative Agent GUI Protocol v1).
It defines the frames, the typed method vocabulary (§5.1–§5.7), the event subscription kinds (§5.6), the image payload (§4) and the NDJSON codec (§1).
It is pure serialization: no I/O, no async, no tokio, no Smithay.
`adesk-server` serves these frames, `adesk-client`/`adesk-agent` consume them and `adesk-testkit` drives the server through them, so every wire detail lives here and nowhere else.

## API Surface

Crate root (`src/lib.rs`):
- `PROTOCOL_VERSION: u32 = 1`, `is_compatible_version(u32) -> bool` (const), `check_version(u32) -> Result<()>`; re-exports every public type below and `pub mod methods`.

Frames (`src/frame.rs`):
- `Frame::{Request, Response, Event}` with `From<RequestFrame|ResponseFrame|EventFrame>` and manual `Serialize`/`Deserialize` that discriminate by keys (`method` → request, `id` + exactly one of `result`/`error` → response, `event` → event).
- `RequestFrame { id: u64, method: Method }` (`new`).
- `ResponseFrame { id, outcome: ResponseOutcome }` with `ResponseOutcome::{Result(ResultPayload), Error(ErrorPayload)}`, accessors `result_payload`/`error_payload`/`is_error`, constructors `ResponseFrame::result::<R>(id, &R)` and `::error(id, ErrorPayload)`.
- `ResultPayload(serde_json::Value)` with `new::<R>(&R)`, `decode::<R>()`, `as_value()`.
- `ErrorPayload { code: ErrorCode, message: String, data: Option<Value> }` with `new`, `with_data`.
- `EventFrame { event: EventKind, seq: u64, ts_ms: u64, data: EventPayload }` with `new`, `from_runtime(&RuntimeEvent)`, `to_runtime() -> Option<RuntimeEvent>`.
- Crate-internal `Frame::from_value(serde_json::Value) -> Result<Frame>` — the decoder every entry point routes through (see Design Decisions).

Methods (`src/methods.rs` + `src/methods/*.rs`):
- `Method` — 29 variants, one per spec method — plus `method_name()`, `from_parts(name, params)`, `params_value()`, manual map serde.
- `ActionResult { action_id: ActionId }` (result of pointer/key actions).
- `runtime::PingParams/PingResult`; `apps::ListAppsParams/Result`, `GetAppParams/Result`, `LaunchAppParams/Result`; `windows::ListWindowsParams/Result`, `GetWindowParams/Result`, `ActivateWindowParams`, `CloseWindowParams`, `GetFocusParams/Result`; `capture::CaptureWindowParams`, `CaptureRegionParams`, `CaptureResult`, `ObserveParams`, `ObserveResult`, `WaitForChangeParams`, `WaitForQuietParams`; `input::` params for all 11 input methods plus `TypeTextResult`; `subscription::SubscribeEventsParams/Result`, `UnsubscribeEventsParams/Result`; `inspector::InspectCaptureParams/Result`, `InspectSubscribeParams/Result`.

Events (`src/event.rs`):
- `EventKind` — 12 snake_case variants (`window_created` … `inspect_frame`) — with `SUBSCRIBABLE: [EventKind; 11]` (the §5.6 set), `is_subscribable()`, `matches(&RuntimeEvent)`.
- `EventPayload` — 11 typed variants — with `kind()`, `from_runtime`, `to_runtime(seq, ts_ms)`, `from_data(kind, Value)`, `to_data()`.
- Data structs: `WindowCreatedEvent`, `WindowDestroyedEvent`, `WindowActivatedEvent`, `TitleChangedEvent`, `SurfaceCommitEvent`, `FocusChangedEvent`, `PopupAppearedEvent`, `PopupDisappearedEvent`, `AppLaunchedEvent`, `QuietEvent`, `InspectFrameEvent`.

Images (`src/image.rs`): `ImagePayload { width, height, format, stride: Option<u32>, data: String (base64), scale: f64 }` with `from_rgba8`, `from_png`, `decode_data`, `to_rgba8_buffer`.

Vocabulary (`src/types.rs`): `ImageFormat::{Png, Rgba8}`, `RendererKind::{Gl, Pixman}`, `Condition::{Quiet{quiet_ms}, Change, Timeout}` (tagged by `type`), `KeySpec::{Single(String), Chord(Vec<String>)}` (untagged) with `keys()` and `From` impls.

Codec (`src/codec.rs`): `trait Codec { name, encode(&Frame) -> Result<Vec<u8>>, decode(&[u8]) -> Result<Frame> }`, `NdjsonCodec` with `encode_str`/`decode_str`, free `encode_frame(&Frame) -> Result<String>` and `decode_frame(&str) -> Result<Frame>`.

Errors (`src/error.rs`): `ProtoError` (`Malformed`, `UnknownMethod`, `InvalidParams`, `InvalidEventData`, `UnknownEventKind`, `InvalidResult`, `VersionMismatch`, `Json`, `Base64`), `error_code()`, `From<ProtoError> for adesk_core::Error`, `pub type Result<T>`.

## Constraints

- `docs/protocol.md` is normative: never invent methods, fields or error codes; additive changes only (§7).
- Pure serialization only — no tokio, no async, no I/O, no Smithay in this crate.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; every public item is documented.
- Dependencies come only from root `[workspace.dependencies]` (`adesk-core`, `serde`, `serde_json`, `base64`, `thiserror`); never add inline versions.
- Decoding must ignore unknown fields (forward compatibility, §1) and must never panic on request/event paths; malformed input returns `ProtoError`.
- `adesk_core` owns the domain vocabulary (`WindowInfo`, `Observation`, `RuntimeEvent`, `ErrorCode`, `Region`, `Position`, `AppInfo`, `OverlayKind`); depend on it, never fork it.
- `ImageBuffer` is not wire-facing; `ImagePayload` is the wire form.
- `PROTOCOL_VERSION` is bumped only for breaking changes; a mismatch is a hard error (§5.1).

## Routing Table

| Area | File |
|---|---|
| Crate root, version helpers, re-exports | `src/lib.rs` |
| Frame kinds, request/response/event frames, `ResultPayload`, `ErrorPayload`, `Frame::from_value` decoder | `src/frame.rs` |
| `Method` enum, `ActionResult`, per-group params/results | `src/methods.rs`, `src/methods/*.rs` |
| `EventKind` filter, `EventPayload`, event data structs | `src/event.rs` |
| `ImagePayload` and base64/RGBA conversions | `src/image.rs` |
| `Codec` trait, `NdjsonCodec`, `encode_frame`/`decode_frame` | `src/codec.rs` |
| `ProtoError`, `Result`, AGP error-code mapping | `src/error.rs` |
| Spec defaults used by `#[serde(default = ...)]` | `src/defaults.rs` (crate-private) |
| Protocol vocabulary types | `src/types.rs` |
| Type-level + golden-JSON tests | `tests/wire.rs` |
| Frame-level acceptance spec | `tests/codec.rs` |
| Method round-trip + golden JSON tests | `tests/methods_roundtrip.rs` |
| Frame/event/codec round-trip tests | `tests/frames_events_roundtrip.rs` |
| Image payload tests | `tests/image_roundtrip.rs` |

## Design Decisions

- `ObserveResult` resolves the §4-vs-§5.4 ambiguity: `image` lives INSIDE the `observation` object (per §4), so the wire shape is `{"observation": {<core Observation fields>, "image": <ImagePayload|null>}}`; `image` is always present (`null` when absent) and is split out of the core `Observation` on deserialize.
- `EventFrame` hoists `seq`/`ts_ms` out of the core `RuntimeEvent` into frame-level fields; `data` carries the variant fields minus those two.
- `EventKind::SurfaceDamage` is a filter alias, never an emitted kind: durable commits are `surface_commit` (which carries `damage`), and `matches` returns true only for commits with non-empty damage; `EventPayload::from_data` rejects it with `Malformed`.
- `EventKind::InspectFrame` is a 12th, non-subscribable kind so `inspect_subscribe` pushes are typed; the §5.6 eleven filterable kinds are exactly `SUBSCRIBABLE`.
- `QuietEvent` and `InspectFrameEvent` are the data structs of the two protocol-only kinds: §5.7 fixes `inspect_frame`'s shape (`{"subscription_id", "image"}`), while `quiet` — a reserved filterable kind with no v1 emitter (§5.6) — keeps a crate-defined shape (additive, §7).
- `EventKind::Quiet` is subscribable (`is_subscribable()` returns true; it is in `SUBSCRIBABLE`) but protocol-only: no `RuntimeEvent` maps to `EventPayload::Quiet` (`from_runtime` has no such arm), `EventKind::Quiet::matches` is always false, and within this crate the payload is produced only by wire decoding (`EventPayload::from_data`) and tests.
- Response results are untyped at frame level (`ResultPayload(serde_json::Value)`): a codec cannot correlate an `id` to a method, so server/client decode with the method's typed result via `ResultPayload::decode::<R>()`.
- All decode entry points (`decode_frame`, `NdjsonCodec::{decode, decode_str}`) route through crate-internal `Frame::from_value`, because serde's `Deserialize` cannot carry `ProtoError` payloads while the frozen acceptance spec requires exact `UnknownMethod(name)`/`Malformed(_)`/`UnknownEventKind(name)`; the serde `Deserialize` impls map errors to generic serde errors.
- `Frame::from_value` discrimination order is `event` → `method` → `id` + exactly one outcome; missing/null `params`/`data` are treated as `{}`; non-objects and unrecognized shapes are `Malformed`; JSON syntax errors are `Json`/`Malformed`.
- `Method::from_parts` is the canonical `(name, params)` decoder — the frame layer calls it directly so `ProtoError::UnknownMethod`/`InvalidParams` survive; `Method`'s serde impls exist for `#[serde(flatten)]` in `RequestFrame` and accept the same `{"method", "params"}` mapping.
- `EventPayload::to_data`/`from_data` carry only the variant fields (no tag); shape mismatches are `InvalidEventData{kind, message}`.
- `Method` implements `Serialize`/`Deserialize` manually, emitting/reading a JSON map (`method` + `params`), so `#[serde(flatten)]` in `RequestFrame` works and unknown request fields are ignored.
- `Codec` is payload-oriented (bytes, no terminator) so a future binary framing (§7) needs no method-definition changes; NDJSON adds the string helpers.
- `ProtoError::error_code()` maps `UnknownMethod` → `ErrorCode::UnknownMethod`, `VersionMismatch` → `ProtocolVersionMismatch` and everything else → `InvalidRequest`.
- `CaptureResult` and `InspectCaptureResult` derive `PartialEq` but not `Eq` because `ImagePayload::scale` is `f64`.
- Spec defaults live in crate-private `defaults.rs` and are wired through `#[serde(default = ...)]`: `timeout_ms=5000`, `quiet_ms=250`, `duration_ms=150`, `min_interval_ms=100`, `count=1`, `observe.include_image=true` (waits default `false`), `format=png`, `kinds=SUBSCRIBABLE`, `overlays=["window_ids","focus","damage"]`, `scale=1.0`.
- The `quiet_ms=250` default is scoped to `WaitForQuietParams`; `Condition::Quiet { quiet_ms }` has no serde default, so an `observe`/`until` of `{"type":"quiet"}` without `quiet_ms` fails as `InvalidParams`, and no field on `ObserveParams`/`WaitForChangeParams` can carry a quiet threshold for a non-quiet condition.
- `QuietEvent.quiet_ms` is required when decoding `quiet` event data (`window_id` is optional).
- `ImagePayload::from_rgba8` rejects dimension/byte-count overflow and length mismatch with `Malformed`; `to_rgba8_buffer` is strict — non-`Rgba8` format, `stride != width*4` (including `stride: null`), or length mismatch is an error.
- Base64 encoding is infallible in `base64` 0.22; `ProtoError::Base64` is reachable only on decode paths.

## Test Strategy

- `tests/wire.rs` (24) pins the type layer: golden JSON for the spec examples, wire names, defaults, `Condition`/`KeySpec` shapes, error-code mapping and the `Method::method_name` table.
- `tests/codec.rs` (19) is the frozen frame-level acceptance spec, all active; its assertions are normative — change `docs/protocol.md` first and update this file in the same change.
- `tests/methods_roundtrip.rs` (19): all 29 methods through `from_parts`/`params_value`/serde, golden params JSON, error cases, `ObserveResult` wire shape.
- `tests/frames_events_roundtrip.rs` (14): response/error/event golden JSON, all 11 payload kinds and all 9 `RuntimeEvent`s round-tripped, `EventKind::matches` table, malformed lines, codec trait.
- `tests/image_roundtrip.rs` (15): base64/RGBA/PNG conversions, overflow, stride and length edge cases.
- Run with `./scripts/dev.sh cargo test -p adesk-proto --all-targets` (91 tests) — the dev shell is required for linking.

## Notes for Agents

- `EventKind::InspectFrame` is a real enum variant, so `subscribe_events.kinds` deserializes it even though §5.6 lists 11 filterable kinds; enforcing filterability is `adesk-server`'s job, not this crate's.
- `ObserveResult`'s custom serde assumes core `Observation` has no `image` field; adding one in `adesk-core` would break the split (see Design Decisions).
- Requests are always fully explicit on the wire: `#[serde(default)]` values are still emitted when serializing params (e.g. `format:"png"`, `count:1`), which is additive-safe.
- `ImagePayload` has no `encode_data`/re-encode helper: `data` is a public base64 `String`; build payloads from raw bytes with `from_rgba8` (validates `len == width*height*4`, sets `stride = width*4`) or `from_png` (no validation), both of which base64-encode internally. `base64` is a crate dependency and is NOT re-exported, so consumers assembling payloads by hand need their own base64 dep.
- `ImagePayload` always emits both `stride` (JSON `null` for `png`) and `scale` (default `1.0`); the only optional wire field that is omitted when `None` is `ErrorPayload.data` (`skip_serializing_if`), so wire assertions must expect `stride`/`scale` keys but not `data`.
- `ImageFormat` is a plain two-variant enum (`Png` default, `Rgba8`) with no `#[non_exhaustive]` and no catch-all; unknown wire strings fail deserialization.
- This crate exposes only `*Params`/`*Result` structs; ergonomic request builders (`CaptureRequest`, `CaptureRegionRequest`, `ObserveRequest`, `WaitFor*Request`) live in `adesk-client` and serialize into these params.
- `ErrorCode` recoverability is not specified anywhere (`docs/protocol.md` §6 lists the 13 wire names only); `ProtoError::error_code` maps everything except `UnknownMethod`/`VersionMismatch` to `InvalidRequest`.

## Status

- Implementation-complete: zero `todo!()`, no crate-level `allow` attributes, `cargo check`/`clippy -p adesk-proto --all-targets` clean, 91/91 tests pass under the dev shell.
