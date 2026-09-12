# adesk-proto — AGP v1 wire types, typed methods and NDJSON codec

## Intent

`adesk-proto` is the single implementation of `docs/protocol.md` (normative Agent GUI Protocol v1).
It defines the frames, the typed method vocabulary (§5.1–§5.10), the event subscription kinds (§5.6), the notification vocabulary (§5.9), the image payload (§4) and the NDJSON codec (§1).
It is pure serialization: no I/O, no async, no tokio, no Smithay.
`adesk-server` serves these frames, `adesk-client`/`adesk-agent` consume them and `adesk-testkit` drives the server through them, so every wire detail lives here and nowhere else.

## API Surface

Crate root (`src/lib.rs`):
- `PROTOCOL_VERSION: u32 = 1`, `is_compatible_version(u32) -> bool` (const), `check_version(u32) -> Result<()>`; re-exports every public type below and `pub mod methods`.
- Re-exports the notification vocabulary owned by `adesk_core` — `Notification`, `NotificationAction`, `NotificationUrgency`, `NotificationCloseReason` — so consumers get the wire-facing notification types from `adesk-proto` (they are never redefined here).

Frames (`src/frame.rs`):
- `Frame::{Request, Response, Event}` with `From<RequestFrame|ResponseFrame|EventFrame>` and manual `Serialize`/`Deserialize` that discriminate by keys (`method` → request, `id` + exactly one of `result`/`error` → response, `event` → event).
- `RequestFrame { id: u64, method: Method }` (`new`).
- `ResponseFrame { id, outcome: ResponseOutcome }` with `ResponseOutcome::{Result(ResultPayload), Error(ErrorPayload)}` (an ordinary enum consumers pattern-match), constructors `ResponseFrame::result::<R>(id, &R)` and `::error(id, ErrorPayload)`.
- `ResultPayload(serde_json::Value)` with `new::<R>(&R)`, `decode::<R>()` (borrowing, clones the `Value`) and `into_decode::<R>()` (consuming, no clone), plus the borrow-only `as_value()`.
- `ErrorPayload { code: ErrorCode, message: String, data: Option<Value> }` with `new`, `with_data`.
- `EventFrame { event: EventKind, seq: u64, ts_ms: u64, data: EventPayload }` with `new`, `from_runtime(&RuntimeEvent)`, `to_runtime() -> Option<RuntimeEvent>`.
- Crate-internal `Frame::from_value(serde_json::Value) -> Result<Frame>` — the decoder every entry point routes through (see Design Decisions).

Methods (`src/methods.rs` + `src/methods/*.rs`):
- `Method` — 34 variants, one per spec method — plus `method_name()`, `from_parts(name, params)`, `params_value()`, manual map serde.
- `ActionResult { action_id: ActionId }` (result of pointer/key actions).
- `runtime::PingParams/PingResult`; `apps::ListAppsParams/Result`, `GetAppParams/Result`, `LaunchAppParams/Result`; `windows::ListWindowsParams/Result`, `GetWindowParams/Result`, `ActivateWindowParams`, `CloseWindowParams`, `GetFocusParams/Result`; `capture::CaptureWindowParams`, `CaptureRegionParams`, `CaptureResult`, `ObserveParams`, `ObserveResult`, `WaitForChangeParams`, `WaitForQuietParams`; `input::` params for all 11 input methods plus `TypeTextResult`; `subscription::SubscribeEventsParams/Result`, `UnsubscribeEventsParams/Result`; `inspector::InspectCaptureParams/Result`, `InspectSubscribeParams/Result`; `notification::PostNotificationParams/PostNotificationResult`, `ListNotificationsParams/ListNotificationsResult`, `CloseNotificationParams/CloseNotificationResult`, `InvokeNotificationActionParams/InvokeNotificationActionResult` (§5.9); `events::WaitForEventsParams/WaitForEventsResult`, `EventRecord` (§5.10).

Events (`src/event.rs`):
- `EventKind` — 15 snake_case variants (`window_created` … `inspect_frame`, incl. the §5.9 `notification`/`notification_closed`/`notification_action`) — with `SUBSCRIBABLE: [EventKind; 14]` (the filterable set), `is_subscribable()`, `matches(&RuntimeEvent)`.
- `EventPayload` — 14 typed variants — with `kind()`, `from_runtime`, `to_runtime(seq, ts_ms)`, `from_data(kind, Value)`, `to_data()`.
- Data structs: `WindowCreatedEvent`, `WindowDestroyedEvent`, `WindowActivatedEvent`, `TitleChangedEvent`, `SurfaceCommitEvent`, `FocusChangedEvent`, `PopupAppearedEvent`, `PopupDisappearedEvent`, `AppLaunchedEvent`, `QuietEvent`, `InspectFrameEvent`, `NotificationEvent` (`data` = `{"notification": Notification}`), `NotificationClosedEvent` (`{"notification_id", "reason"}`), `NotificationActionEvent` (`{"notification_id", "action_key"}`).

Images (`src/image.rs`): `ImagePayload { width, height, format, stride: Option<u32>, data: String (base64), scale: f64 }` with `from_rgba8`, `from_png`, `decode_data`, `to_rgba8_buffer`.

Vocabulary (`src/types.rs`): `ImageFormat::{Png, Rgba8}`, `RendererKind::{Gl, Pixman}`, `Condition::{Quiet{quiet_ms}, Change, Timeout}` (tagged by `type`), `KeySpec::{Single(String), Chord(Vec<String>)}` (untagged) with `keys()` and a single `From<&str>` conversion.

Codec (`src/codec.rs`): `trait Codec { name, encode(&Frame) -> Result<Vec<u8>>, decode(&[u8]) -> Result<Frame> }`, `NdjsonCodec` with `encode_str`/`decode_str`.
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
| §5.9 notification methods (post/list/close/invoke) params + results | `src/methods/notification.rs` |
| §5.10 `wait_for_events` params/result + `EventRecord` | `src/methods/events.rs` |
| `EventKind` filter, `EventPayload`, event data structs | `src/event.rs` |
| `ImagePayload` and base64/RGBA conversions | `src/image.rs` |
| `Codec` trait, `NdjsonCodec` | `src/codec.rs` |
| `ProtoError`, `Result`, AGP error-code mapping | `src/error.rs` |
| Spec defaults used by `#[serde(default = ...)]` | `src/defaults.rs` (crate-private) |
| Protocol vocabulary types | `src/types.rs` |
| Type-level + golden-JSON tests | `tests/wire.rs` |
| Frame-envelope NDJSON goldens (exact request lines + full round-trip) | `tests/codec.rs` |
| Method round-trip + golden JSON tests | `tests/methods_roundtrip.rs` |
| Frame/event/codec round-trip tests | `tests/frames_events_roundtrip.rs` |
| Image payload tests | `tests/image_roundtrip.rs` |
| Shared test helpers + fixtures | `tests/common/mod.rs` |

## Design Decisions

- `ObserveResult` resolves the §4-vs-§5.4 ambiguity: `image` lives INSIDE the `observation` object (per §4), so the wire shape is `{"observation": {<core Observation fields>, "image": <ImagePayload|null>}}`; `image` is always present (`null` when absent) and is split out of the core `Observation` on deserialize.
- `EventFrame` hoists `seq`/`ts_ms` out of the core `RuntimeEvent` into frame-level fields; `data` carries the variant fields minus those two.
- `EventKind::SurfaceDamage` is a filter alias, never an emitted kind: durable commits are `surface_commit` (which carries `damage`), and `matches` returns true only for commits with non-empty damage; `EventPayload::from_data` rejects it with `Malformed`.
- `EventKind::InspectFrame` is a non-subscribable kind so `inspect_subscribe` pushes are typed; the 14 filterable kinds (the §5.6 set plus the three §5.9 notification kinds) are exactly `SUBSCRIBABLE`.
- The three §5.9 notification kinds (`notification`, `notification_closed`, `notification_action`) are ordinary subscribable/filterable kinds, even though §5.6's prose enumerates only 11: §5.9 declares them event kinds and §5.10's "all" default is the §5.6 set. They map 1:1 to `RuntimeEvent::{Notification,NotificationClosed,NotificationAction}`; `is_subscribable()` excludes only `InspectFrame`.
- `EventRecord` (§5.10) is the untyped waiter envelope (`{event, seq, ts_ms, data: Value}`, `data` raw JSON) — deliberately distinct from the typed `EventFrame`; it lives with `wait_for_events` in `src/methods/events.rs`.
- Notification domain types (`Notification`, `NotificationAction`, `NotificationUrgency`, `NotificationCloseReason`, `NotificationId`) are owned by `adesk-core` and re-exported by `adesk-proto` — never forked.
- `QuietEvent` and `InspectFrameEvent` are the data structs of the two protocol-only kinds: §5.7 fixes `inspect_frame`'s shape (`{"subscription_id", "image"}`), while `quiet` — a reserved filterable kind with no v1 emitter (§5.6) — keeps a crate-defined shape (additive, §7).
- `EventKind::Quiet` is subscribable (`is_subscribable()` returns true; it is in `SUBSCRIBABLE`) but protocol-only: no `RuntimeEvent` maps to `EventPayload::Quiet` (`from_runtime` has no such arm), `EventKind::Quiet::matches` is always false, and within this crate the payload is produced only by wire decoding (`EventPayload::from_data`) and tests.
- Response results are untyped at frame level (`ResultPayload(serde_json::Value)`): a codec cannot correlate an `id` to a method, so server/client decode with the method's typed result via `ResultPayload::decode::<R>()`.
  `decode::<R>` cannot borrow (serde's `from_value` is by-value), so it clones `self.0`; `into_decode::<R>` consumes `self.0` with no clone, and a consumer that only needs the owned `Value` can destructure the public tuple field (`let ResultPayload(value) = payload;`) and move it out — `as_value(&self)` is the borrow-only accessor, there is no `into_value`.
- All decode entry points (`NdjsonCodec::{decode, decode_str}`) route through crate-internal `Frame::from_value`, because serde's `Deserialize` cannot carry `ProtoError` payloads while the frozen acceptance spec requires exact `UnknownMethod(name)`/`Malformed(_)`/`UnknownEventKind(name)`; the serde `Deserialize` impls map errors to generic serde errors.
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

- `tests/common/mod.rs` holds the shared helpers and fixtures (`wire<T: Serialize>`, `roundtrip<T>`, `image_payload()`, `observation()`, `damage()`, `window_info()`, `app_info()`, `ping_result()`); each test file declares `mod common;`. It carries `#![allow(dead_code)]` because every test binary compiles the whole module.
- `tests/wire.rs` (24) pins the type layer: golden JSON for the spec examples, wire names, defaults, `Condition`/`KeySpec` shapes, error-code mapping and the `Method::method_name` table.
- `tests/codec.rs` (3) keeps the frame-envelope NDJSON goldens unique to it: the exact encoded `ping`/`click` request lines and a full encode/decode round-trip over every frame kind.
- `tests/methods_roundtrip.rs` (22): all 34 methods through `from_parts`/`params_value`/serde, golden params JSON, error cases, `ObserveResult` wire shape, §5.9 notification defaults/shapes and §5.10 `EventRecord`/`WaitForEventsResult` goldens.
- `tests/frames_events_roundtrip.rs` (15): response/error/event golden JSON, all 14 payload kinds and all 12 `RuntimeEvent`s round-tripped, `EventKind::matches` table, malformed lines, codec trait.
- `tests/image_roundtrip.rs` (15): base64/RGBA/PNG conversions, overflow, stride and length edge cases, plus the authoritative `image_payload_base64_helpers` constructor golden.
- Run with `./scripts/dev.sh cargo test -p adesk-proto --all-targets` (79 tests + 1 doctest at `lib.rs:17`) — the dev shell is required for linking.
- There are 0 lib unit tests: no `#[cfg(test)] mod tests` exists anywhere in `src/`; every assertion is an integration test or the single `lib.rs` doctest.
- `wire<T>` is the single shared golden-JSON helper; no test file re-defines it. There are no `rect()`/`size()`/`position()`/`client()`/`server()` helpers — `Rect`/`Size`/`Position` are built inline via `Rect::new`/`Size::new`/`Position::pixels`/`Position::normalized`.
- Cross-file goldens sit at different envelope layers and each is pinned once: the `click` normalized-position literal at the type layer (`wire.rs`), the method envelope (`methods_roundtrip.rs`) and the encoded NDJSON frame line (`codec.rs`); `ErrorPayload` at the type layer (`wire.rs`) and the frame envelope (`frames_events_roundtrip.rs`); the `surface_commit` event JSON at the frame envelope (`frames_events_roundtrip.rs`); `ImagePayload` at the type layer (`wire.rs`), method layer (`methods_roundtrip.rs`), frame layer (`frames_events_roundtrip.rs`) and image layer (`image_roundtrip.rs`). This is layered spec-golden coverage (§2/§3/§4 type layer vs §5 method envelope vs §1 frame envelope), not independent verification — all drive the same serde impls.
- No test uses a sleep, `Instant`, `Duration`, I/O, async or a timeout; the whole suite is in-memory serialization, so it is wall-clock-free and reproducible.
- Cross-crate overlap: several tests assert exact golden JSON for `adesk_core` types *embedded in proto wire structs* — Position/Button/WindowId (`wire.rs`, `methods_roundtrip.rs`), Rect/Region damage (`wire.rs`, `frames_events_roundtrip.rs`, `methods_roundtrip.rs`), full WindowInfo (`wire.rs`, `methods_roundtrip.rs`), partial AppInfo (`wire.rs`), Observation subsets (`methods_roundtrip.rs`), Size (`wire.rs`), ActionId (`wire.rs`), ErrorCode wire names (`wire.rs`, `frames_events_roundtrip.rs`) — these mirror shapes also pinned in `adesk-core/tests/serde_wire.rs`. The proto versions add the AGP method/frame envelope + base64 image coverage, not new core-type shapes.
- `adesk_core::EventKind` (9 variants) is never imported by proto tests; proto tests exercise only the crate's own 15-variant `EventKind` (superset, in `src/event.rs`).

## Notes for Agents

- Public helpers whose only in-repo consumers are this crate's own tests plus a couple of cross-crate test call sites: `EventFrame::to_runtime`/`EventPayload::to_runtime` (also called by `adesk-server/tests/subscriptions.rs`), `ImagePayload::to_rgba8_buffer` (also called from `adesk-server/src/images.rs`'s test module) and `PingResult::is_compatible`. Consumers otherwise use `NdjsonCodec`/the `Codec` trait directly or pattern-match `ResponseOutcome`/`Frame`. They are the documented public surface: a workspace grep miss is not permission to delete, and removing any of them is a public-API change.
- `EventKind::InspectFrame` is a real enum variant, so `subscribe_events.kinds` deserializes it even though it is not filterable (`EventKind::SUBSCRIBABLE` excludes it); enforcing filterability is `adesk-server`'s job, not this crate's.
- `ObserveResult`'s custom serde assumes core `Observation` has no `image` field; adding one in `adesk-core` would break the split (see Design Decisions).
- Requests are always fully explicit on the wire: `#[serde(default)]` values are still emitted when serializing params (e.g. `format:"png"`, `count:1`), which is additive-safe.
- `ImagePayload` has no `encode_data`/re-encode helper: `data` is a public base64 `String`; build payloads from raw bytes with `from_rgba8` (validates `len == width*height*4`, sets `stride = width*4`) or `from_png` (no validation), both of which base64-encode internally. `base64` is a crate dependency and is NOT re-exported, so consumers assembling payloads by hand need their own base64 dep.
- This crate contains **no PNG codec and no pixel comparison/diff logic** (`src/image.rs` is the whole of its image surface): `from_png` only base64-wraps caller-supplied PNG bytes and there is no PNG→pixels helper; `to_rgba8_buffer` is `rgba8`-only and rejects `Png`. PNG encode lives in `adesk-render` (private `src/image.rs::encode_png`, `image::codecs::png::PngEncoder`), PNG decode plus the payload→`ImageBuffer` path in `adesk-client` (`src/image.rs::decode_image` via `image::load_from_memory_with_format`, with `CaptureResult::decode_image` / `ObserveResult::decode_image` wrappers in `src/api/capture.rs`) — both via the `image` crate, which this crate does not depend on.
- `ImagePayload::to_rgba8_buffer` (strict wire validation: `rgba8` only, stride exactly `width*4`, length exact) overlaps `adesk-client`'s `decode_rgba8`, which additionally accepts a padded or absent stride and re-packs rows for the pixel path; the strict form is the proto-side validator, the lenient one the client-side pixel decoder — a candidate single home if the duplication is ever collapsed.
- `ImagePayload` always emits both `stride` (JSON `null` for `png`) and `scale` (default `1.0`); the only optional wire field that is omitted when `None` is `ErrorPayload.data` (`skip_serializing_if`), so wire assertions must expect `stride`/`scale` keys but not `data`.
- `ImageFormat` is a plain two-variant enum (`Png` default, `Rgba8`) with no `#[non_exhaustive]` and no catch-all; unknown wire strings fail deserialization.
- This crate exposes only `*Params`/`*Result` structs; ergonomic request builders (`CaptureRequest`, `CaptureRegionRequest`, `ObserveRequest`, `WaitFor*Request`) live in `adesk-client` and serialize into these params.
- `ErrorCode` recoverability is not specified anywhere (`docs/protocol.md` §6 lists the 13 wire names only); `ProtoError::error_code` maps everything except `UnknownMethod`/`VersionMismatch` to `InvalidRequest`.

## Performance Characteristics (per-frame hot path)

Every frame is on the hot path: the server encodes each response/event (`connection.rs` write loop) and decodes each request, the client does the reverse.
- Decode always builds a full `serde_json::Value` DOM first (`codec.rs:59`), then `Frame::from_value` re-parses the params/`data` subtree into typed structs with `serde_json::from_value` (`methods.rs:246`, `event.rs:437`) — a DOM pass plus a typed pass per frame.
- The decode helpers MOVE their subtrees out of the parsed object with `Map::remove` instead of deep-cloning: request method-name + params (`frame.rs:427,441`), response `result`/`error` (`frame.rs:204,205`), event name + `data` (`frame.rs:309,342`). The frame decode path has no remaining clone of the parsed tree, and `Frame::from_value` (`frame.rs:395`) no longer re-wraps the object per arm.
- Encode is typed → `Value` → `String` (double serialization) in three places: `ResponseFrame::result` → `ResultPayload::new` → `serde_json::to_value` (`frame.rs:81,144`); `Method::serialize` → `params_value` → `to_value` (`methods.rs:203,205`); `EventFrame::serialize` → `EventPayload::to_data` → `to_value` (`event.rs:282,474`).
- `EventPayload::to_data` runs once per event *per subscriber*: the server builds the frame once then clones it per subscriber (`adesk-server/src/subscriptions.rs:153`) and each connection re-encodes its clone (`connection.rs:99`).
- `ObserveResult::serialize` (`methods/capture.rs:136`) builds three `Value`s (observation, image, whole) then serializes the third; the image `Value` copies the whole base64 `data` string.
- `ResultPayload::decode` (`frame.rs:89`) clones the `Value` (serde `from_value` is by-value); `ResultPayload::into_decode` (`frame.rs:104`) is the moving counterpart that consumes `self.0` with no clone.
- `EventFrame::from_runtime` (`event.rs:284`) clones `title`/`app_id`/`damage` out of the core event; `ImagePayload` base64 encode/decode (`image.rs:69,81,92`) is one copy each way.
- Byte-changing optimizations deliberately NOT taken (they would alter emitted key order, not semantics): delegating the three `Serialize` impls above to direct typed serialization, a streaming result path, and the `ObserveResult::serialize` rewrite.
- Wire-order caveat: `serde_json` is built without `preserve_order` (deps: no `indexmap`), so every `Value`-built object has alphabetically sorted keys; replacing a `Value` step with direct typed serialization emits declaration order — identical JSON, different bytes. `docs/protocol.md` fixes no key order and the tests compare with `serde_json::Value` equality.

## Status

- `src/` has no `todo!()`/`unimplemented!()` and no crate-level `allow` attributes.
- `./scripts/dev.sh cargo test -p adesk-proto --all-targets` is 79/79 green (+1 doctest); `cargo clippy -p adesk-proto --all-targets --no-deps -- -D warnings` and `cargo fmt -p adesk-proto --check` are clean.
- `cargo check -p adesk-proto --all-targets` is green (the crate builds standalone).
- `cargo doc -p adesk-proto --no-deps --document-private-items` is warning-free: every intra-doc link resolves and no link carries a redundant explicit target (write `[`ProtoError::Json`]`, not `[`ProtoError::Json`](crate::ProtoError::Json)`, and qualify out-of-scope items as `[`crate::ProtoError::Json`]`).
