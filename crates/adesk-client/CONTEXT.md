# adesk-client — async Rust SDK for AGP

## Intent

`adesk-client` is the only supported way for Rust programs to drive a running ADesk runtime over its Unix socket.
It implements the **client side of `docs/protocol.md` (AGP v1)** and nothing else: typed async methods for every §5.1–§5.7 method, plus `futures::Stream` views over the event channel.
Primary consumers: `adesk-agent` (the multimodal agent prototype) and integration tests (`adesk-server/tests/`, `adesk-compositor/tests/` via `adesk-testkit`).
The crate is a pure client: it never links Smithay, never touches the compositor thread, and contains no server-side logic.
Its design follows the runtime's invariants — observations are causal (action ids, never sleeps), coordinates stay window-relative, and quietness is evidence rather than a promise.

## API Surface

Modules are private; every public item is re-exported flat at the crate root (`adesk_client::<Name>`).

### Connection (`src/client.rs`)
- `Client` — cheap-to-clone async handle; all methods take `&self`, so one handle can be shared across tasks. Clones share socket, request-id space, pending map and event fan-out.
- `Client::connect(path)`, `Client::connect_default()`, `Client::connect_with(ConnectOptions) -> Result<Client>`.
- `Client::socket_path()`, `is_closed()`, `protocol_version()`, `close(self) -> Result<()>` (closes the shared connection; in-flight requests fail with `Closed`).
- `ConnectOptions` (`#[non_exhaustive]`): `path`, `max_frame_len`, `connect_timeout`, `verify_version` (default `true` → post-connect `ping` refuses version skew); builders `new/path/max_frame_len/connect_timeout/verify_version`.
- `default_socket_path()`: `$ADESK_SOCKET` → `$XDG_RUNTIME_DIR/adesk.sock` → `<system temp dir>/adesk.sock` (the exact fallback expression `adesk_server::default_socket_path()` uses, so the two meet under any `TMPDIR`).
- `DEFAULT_MAX_FRAME_LEN` (16 MiB), `DEFAULT_CONNECT_TIMEOUT` (5 s).

### Errors (`src/error.rs`)
- `ClientError` (`#[non_exhaustive]`, `thiserror`): `Io`, `Protocol{message}`, `Closed`, `Server{code, message}`, `VersionMismatch{client, server}`, `InvalidPayload{message}`, `Image{message}`, `Lagged{skipped}`.
- `Result<T, E = ClientError>`; `From<adesk_core::Error>` maps the umbrella error to `ClientError::Server`.
- AGP semantic outcomes are **results, not errors**: expired waits return `Observation{timed_out:true}`; unmapped `type_text` characters appear in `TypeTextResult::skipped`.

### Methods (all `async`, one per protocol method, snake_case matching the wire)
- §5.1 `ping() -> PingInfo` (validates `protocol_version`, `VersionMismatch` on skew); `ping_raw()` skips the check. `PingInfo{protocol_version, runtime_version, uptime_ms, renderer, output}`, `Renderer{Gl, Pixman, Unknown}`.
- §5.2 `list_apps(query: Option<&str>, include_hidden: bool) -> Vec<AppInfo>`; `get_app(&AppId) -> AppInfo`; `launch_app(&AppId, &[String]) -> LaunchResult{launch_id, app_id, pid}`.
- §5.3 `list_windows() -> WindowList{windows, active_window_id}`; `get_window(WindowId) -> WindowInfo`; `activate_window(WindowId) -> ActionId`; `close_window(WindowId) -> ActionId`; `get_focus() -> FocusInfo{window_id, surface_focus}`.
- §5.4 `capture_window(CaptureRequest) -> CaptureResult`; `capture_region(CaptureRegionRequest) -> CaptureResult`; `observe(ObserveRequest) -> ObserveResult{observation, image}`; `wait_for_change(WaitForChangeRequest) -> Observation`; `wait_for_quiet(WaitForQuietRequest) -> Observation`.
- §5.5 `pointer_move(WindowId, Position)`; `click(ClickRequest)`; `double_click(PointerButtonRequest)`; `mouse_down`/`mouse_up(PointerButtonRequest)`; `scroll(ScrollRequest)`; `drag(DragRequest)`; `keypress(impl Into<KeyChord>, Option<WindowId>)`; `key_down`/`key_up(&str, Option<WindowId>)`; `type_text(&str, Option<WindowId>) -> TypeTextResult{action_id, skipped}`. All return `ActionId` except `type_text`.
- §5.6 `subscribe_events(EventFilter) -> EventStream`; `subscribe_frames(EventFilter) -> AgpEventStream` (client extension: all frames, forward-compatible); `unsubscribe_events(u64) -> ()`.
- §5.7 `inspect_capture(InspectCaptureRequest) -> ImagePayload`; `inspect_subscribe(InspectSubscribeRequest) -> InspectStream`.

### Request types
- `CaptureRequest::window(id)` + `.region/.max_dimension/.format`; `CaptureRegionRequest::new(id, rect)` (region mandatory) + `.max_dimension/.format`.
- `ObserveRequest::quiet(ms)/change()/timeout()` + `.window/.after_action/.timeout_ms/.include_image/.max_dimension/.region`; `Condition{Quiet{quiet_ms}, Change, Timeout}`; `DEFAULT_TIMEOUT_MS = 5000`.
- `WaitForChangeRequest::default()` + `.window/.since_commit/.timeout_ms`; `WaitForQuietRequest::default()` (quiet_ms 250) + `.window/.quiet_ms/.timeout_ms/.after_action`. These two helpers never request pixels (see Design Decisions).
- `ClickRequest::window(id)` + `.position/.button/.count`; `PointerButtonRequest::window(id)` + `.position/.button` (used by `double_click`/`mouse_down`/`mouse_up`); `ScrollRequest::new(id, dx, dy)` + `.position`; `DragRequest::new(id, from, to)` + `.button/.duration_ms`.
- `KeyChord{Single(String), Chord(Vec<String>)}` + `single`/`chord`, `From<&str>`, `From<String>`, `From<Vec<String>>`, `From<&[&str]>`; serialises as string or array per protocol §3.
- `InspectCaptureRequest::default()` (= `DEFAULT_OVERLAYS`: window_ids+focus+damage) + `.region/.max_dimension`; `InspectSubscribeRequest::new(overlays)` + `.min_interval_ms`.

### Events (`src/events.rs`)
- `EventFilter{kinds: Option<Vec<EventKind>>, window_id: Option<WindowId>}` + `all()`, `kinds(..)`, `.window(id)`; `None` fields are omitted from the params object (= "all").
- `EventKind` — the 11 protocol §5.6 names (9 core + `SurfaceDamage`, `Quiet`), snake_case serde, `as_str()`, `From<adesk_core::EventKind>`.
- `AgpEvent{Runtime(RuntimeEvent), InspectFrame(InspectFrame), Other{name, seq, ts_ms, data}}`; `InspectFrame{seq, ts_ms, image}`.
- `EventStream: Stream<Item = Result<RuntimeEvent, ClientError>>`; `AgpEventStream: Stream<Item = Result<AgpEvent, ClientError>>`; `InspectStream: Stream<Item = Result<InspectFrame, ClientError>>`.
- All three are `Send + Unpin`, expose `subscription_id()`, and **unsubscribe on drop** (best-effort `unsubscribe_events`).

### Images (`src/image.rs`)
- `ImageFormat{Png, Rgba8}` (serde `"png"`/`"rgba8"`) for capture requests.
- `ImagePayload` — re-export of the shared wire type from `adesk-proto` (base64 `data`, `width`, `height`, `stride`, `format`, `scale`).
- `decode_image(&ImagePayload) -> Result<ImageBuffer>`; `CaptureResult::decode_image()`; `ObserveResult::decode_image() -> Option<Result<ImageBuffer>>`.

## Constraints

- Only `docs/protocol.md` defines the wire; the client must never invent a method, field or default. Adding one is a root-owned protocol change.
- The client owns no semantics: waits, filtering, ordering and quiet detection are server-side (`docs/architecture.md` §6, §9). Methods marshal params and map results — nothing else.
- All `adesk_proto` frame/codec types are named **only** in `src/wire.rs` (plus the `ImagePayload` re-export in `lib.rs` and its `format` vocabulary in `src/image.rs`); the rest of the crate uses the crate-internal `RawEvent`/`Inbound` vocabulary.
- No third-party version literals: every dependency comes from `[workspace.dependencies]` via `.workspace = true`.
- No `unsafe`; `#![deny(missing_docs)]`. No panics on request/event paths.
- Files stay well under the ~1000-line threshold; split along protocol sections rather than growing a file.
- Do not log pixel payloads; `tracing` at `debug`/`trace` only in transport internals, never per event above `trace`.

## Routing Table

| Area | Owner |
|---|---|
| `Client` handle, `ConnectOptions`, socket path resolution | `./src/client.rs` |
| `ClientError`, `Result`, error mapping | `./src/error.rs` |
| Connection, request ids, pending map, reader/writer tasks, line cap | `./src/transport.rs` |
| AGP frame encode/decode; the only `adesk-proto` touchpoint | `./src/wire.rs` |
| `EventFilter`, `EventKind`, `AgpEvent`, the three event streams | `./src/events.rs` |
| `ImageFormat`, `decode_image` | `./src/image.rs` |
| §5.1 `ping`, `PingInfo`, `Renderer` | `./src/api/runtime.rs` |
| §5.2 app registry methods, `LaunchResult` | `./src/api/apps.rs` |
| §5.3 window methods, `WindowList`, `FocusInfo` | `./src/api/windows.rs` |
| §5.4 capture/observe methods, requests, `Condition`, results | `./src/api/capture.rs` |
| §5.5 input methods, pointer/key request types, `KeyChord` | `./src/api/input.rs` |
| §5.6 subscribe/unsubscribe methods | `./src/api/subscribe.rs` |
| §5.7 inspector methods, inspect request types | `./src/api/inspect.rs` |
| Mock AGP server harness (tests only) | `./tests/common/mod.rs` |
| Integration tests (api, concurrency, errors, events, framing, socket_path, version, images) | `./tests/` |

## Design Decisions

- **Public SDK surface is client-owned and stable.** Method params/results are defined here (`ClickRequest`, `ObserveResult`, …) rather than re-exported from `adesk-proto`, so `adesk-agent` codes against one stable API; the wire mapping lives in the api modules. The only shared wire type exposed is `ImagePayload` (re-exported), because consumers must be able to read pixels without depending on `adesk-proto` directly. If `adesk-proto` later lands typed method params/results, adopt them internally without changing this surface.
- **`adesk-proto` coupling is one file.** `src/wire.rs` converts proto frames to `RawEvent`/`Inbound` immediately; a proto API change touches that file only (plus the `ImagePayload` re-export in `lib.rs` and the `format` match in `src/image.rs`).
- **Event demux is local.** AGP event frames carry no subscription id, so the reader task publishes every event to a per-connection fan-out (`EventFanout` in `src/transport.rs`) and each stream applies its own `EventFilter`. Server-side filtering is an optimisation, not a correctness dependency.
- **The fan-out is per-subscriber bounded `mpsc`, not `broadcast`.** `tokio::sync::broadcast::Receiver` exposes no `poll_recv`, so it cannot be driven from `Stream::poll_next`; instead every subscription gets its own `mpsc` queue (`EVENT_CHANNEL_CAPACITY`) plus an explicit skipped counter. The observable contract matches a broadcast channel: the reader never blocks, and when the connection ends the senders are dropped so receivers wake and observe `RecvError`-equivalent end-of-stream.
- **Backpressure is explicit.** A lagging subscriber gets `ClientError::Lagged{skipped}` on its next poll rather than stalling the reader; the stream stays usable and the consumer re-synchronises (e.g. `list_windows`).
- **Requests are id-matched and non-blocking.** Ids are per-connection monotonic from 1; `pending` is a `std::sync::Mutex<HashMap<u64, oneshot::Sender<..>>>` never held across `await`; the outbound queue is bounded so a broken connection fails fast with `Closed` instead of blocking callers.
- **Unsubscribe is a `Drop` side effect.** Streams enqueue a best-effort `unsubscribe_events` when dropped; failures are ignored because the connection is going away anyway.
- **`ping` is the version gate.** `connect*` pings by default (`ConnectOptions::verify_version`) so a skewed client fails at connect, not at the first request. `ping_raw` exists for diagnostics.
- **The wait helpers are pixel-free.** `wait_for_change`/`wait_for_quiet` omit `include_image` (protocol default `false`) and return `Observation`; use `observe` when pixels are needed. This keeps the return type honest instead of silently discarding an image.
- **`ServerError` drops the wire `data` object.** `ClientError::Server` carries `code` + `message` only; `data` is advisory and the typed variants cover actionable cases. Add a field here (not in a second error type) if a consumer needs it.
- **Observation image handling is layout-tolerant.** The protocol puts the optional image inside the `Observation` object (§4) while `adesk-core::Observation` has no image field, so `ObserveResult` deserialises the observation object with `#[serde(flatten)]` and picks up `image` whether it is a sibling or nested.
- **`Renderer::Unknown` and `AgpEvent::Other`** keep the client forward-compatible with additive protocol changes (protocol §7) instead of failing deserialisation.

## Test Strategy

Integration tests only (`./tests/`), no compositor, no display, no GPU, no network — a mock AGP server over a real `tokio::net::UnixListener` in a `tempfile` dir.

- `tests/common/mod.rs` — `MockServer`: binds a temp socket, accepts one connection, exposes scripted behaviour: read requests, answer by id (immediately, delayed, or out of order), send error frames, send raw lines (malformed/oversized), push events, close the connection.
- `tests/api.rs` — one round-trip per §5.1–§5.7 method: assert the exact request `method`/`params` JSON and that canned results deserialise into the typed values.
- `tests/concurrency.rs` — N concurrent in-flight requests get distinct ids and all resolve; responses delivered out of order are still matched to the right caller.
- `tests/errors.rs` — every `ErrorCode` maps to `ClientError::Server{code, message}`; an error does not close the connection; unknown response ids are ignored.
- `tests/events.rs` — subscribe yields typed `RuntimeEvent`s; kind/window filtering is applied locally; `subscribe_frames` surfaces `Other`/`InspectFrame`; dropping a stream sends `unsubscribe_events`; lag yields `Lagged`; connection close ends the stream with `Closed`.
- `tests/framing.rs` — malformed JSON → `Protocol`; a line above `max_frame_len` → `Protocol`; EOF with requests in flight → `Closed`.
- `tests/version.rs` — `ping` with a mismatched `protocol_version` → `VersionMismatch`; `connect` fails by default, `verify_version(false)` connects.
- `tests/images.rs` — `decode_image` for PNG and raw RGBA8 (including a non-tight `stride` that must be repacked), plus failure cases (bad base64, unknown format rejected at the wire boundary, truncated PNG) and client-owned extras for length/stride/dimension mismatches.
- `tests/socket_path.rs` — `default_socket_path()` resolution order. The fallback branch is reachable only with `$ADESK_SOCKET` and `$XDG_RUNTIME_DIR` unset, which the ambient environment does not guarantee, so each case asserts in a **child** process of the test binary with a controlled environment (`Command::env`/`env_remove` affect the child only); the suite never mutates its own process environment. The fallback case pins `$TMPDIR` to a private dir, so a hard-coded path fails.
- Determinism: no sleeps longer than needed, deadlines explicit, `tokio::time::pause()` only if `test-util` is enabled in dev-deps, otherwise real time with generous margins (`docs/architecture.md` §10).

## Dependencies

- `adesk-core` (ids, geometry, `WindowInfo`/`AppInfo`, `Observation`, `RuntimeEvent`, `ErrorCode`, `ImageBuffer`), `adesk-proto` (wire frames/payloads), `tokio` (net/sync/io-util/rt), `futures` (`Stream`), `serde`/`serde_json`, `image` (PNG decode), `thiserror`, `tracing`; dev: `base64` (test payloads only — `src/` decodes via `ImagePayload::decode_data`), `tempfile`.
- **Required `adesk-proto` surface** (the client's entire coupling — reconcile here first if proto's API differs):
  - `PROTOCOL_VERSION: u32`
  - `Codec` trait (`encode(&Frame) -> Result<Vec<u8>, _>`, `decode(&[u8]) -> Result<Frame, _>`, both terminator-free) implemented by the unit struct `NdjsonCodec`
  - `Frame::{Request(RequestFrame), Response(ResponseFrame), Event(EventFrame)}`
  - `RequestFrame { id: u64, method: Method }` + `RequestFrame::new`, and `Method::from_parts(name: &str, params: Value) -> Result<Method>` (the typed method vocabulary)
  - `ResponseFrame { id: u64, outcome: ResponseOutcome }`, `ResponseOutcome::{Result(ResultPayload), Error(ErrorPayload)}`, `ResultPayload::as_value() -> &Value`
  - `EventFrame { event: EventKind, seq: u64, ts_ms: u64, data: EventPayload }`; `EventKind` serialises as a snake_case string; `EventPayload::to_data() -> Result<Value>`
  - `ErrorPayload { code: adesk_core::ErrorCode, message: String, data: Option<Value> }` (`data` deliberately dropped by the client)
  - `ImagePayload { width, height, format, stride, data, scale }` with a `Png`/`Rgba8` format enum and base64 `data`

## Known Issues

- **Close reasons travel out of band.** A `oneshot` can only carry the server's answer, so the reader/writer/`close()` store the first `CloseReason` (Protocol / Closed / Io) in the connection; every request cancelled afterwards reports it. `tests/framing.rs` pins Protocol for malformed/oversized frames and Closed for EOF.
- **Unknown event kinds need the lenient wire path.** `adesk_proto::NdjsonCodec::decode` rejects an event name it does not know, so `wire::decode_line` retries the line as bare JSON (`event`/`seq`/`ts_ms`, optional `data`) before reporting `Protocol`; this is what makes `AgpEvent::Other` reachable for future kinds (protocol §7).
- **Proto canonicalises request params.** `Method::from_parts` re-encodes params through proto's typed structs, so defaults are filled: `subscribe_events` with `EventFilter::all()` arrives as `{"kinds":[<the 11 names>]}`, and `wait_for_*` always carry `include_image:false`. Assert the client-level serialisation (`serde_json::to_value(&request)`) where the frozen spec says a key is omitted.
- **`decode_image` cannot observe an unknown format.** `adesk_proto::ImageFormat` is a closed `Png`/`Rgba8` enum, so an unknown `format` is rejected by the wire codec before `decode_image` runs, and `decode_image` matches exhaustively. A decode-time unknown-format error needs `#[serde(other)] Unknown` in `adesk-proto` (root-owned). `tests/images.rs` pins the actual boundary behaviour.
- `docs/protocol.md` §5.7 does not specify the data shape of an `inspect_frame` event; the client assumes `{"image": ImagePayload}` and falls back to `AgpEvent::Other` if the payload does not fit.
- `docs/protocol.md` §5.6 lists 11 `EventKind` values while `adesk_core::EventKind` has 9; the client's filter enum carries all 11 and `AgpEvent::Other` preserves any frame it cannot type.
- `docs/protocol.md` §4 shows `image` inside `Observation` while `adesk-core::Observation` has no such field; see Design Decisions for how `ObserveResult` tolerates both layouts. Root may want to pin one layout.
- The client's `wait_for_*` requests omit `include_image` (proto canonicalises it to `false` on the wire); if a consumer needs a post-wait image it must call `observe` or `capture_window`.

## Notes for Agents

- Bare `cargo` cannot link outside the Nix dev shell, and `scripts/dev.sh` is not executable in worktrees — always run `bash scripts/dev.sh cargo <args>`.
- `cargo check --all-targets` does not compile doctests; `cargo test -p adesk-client` runs the `lib.rs` example (1 doctest).
- `tests/common/mod.rs` is a shared module (`mod common;` in each test file), not a test target; it keeps `#![allow(dead_code)]` because each test target uses only a subset of the harness helpers.
- Transport invariants: the reader task never `await`s while holding the `pending` lock and never `unwrap()`s a peer-controlled frame; `EVENT_CHANNEL_CAPACITY` (4096) is mirrored by the events lag test, so raising it means raising that test's emission count.

## Status

Implementation-complete: zero `todo!()`, no skeleton-phase `allow` attributes (the only one left is the harness's `#![allow(dead_code)]`, explained above).
`bash scripts/dev.sh cargo test -p adesk-client` is green: 61 integration tests (api 30, events 7, images 10, errors 3, framing 3, socket_path 3, version 3, concurrency 2) + the `lib.rs` doctest; `cargo check`/`clippy -p adesk-client --all-targets` are warning-free and the crate is rustfmt-clean.
`adesk-agent` and `adesk-testkit` may build on this surface.

## See Also

- `../../docs/protocol.md` — normative AGP v1 (transport, framing, methods, errors).
- `../../docs/architecture.md` §9 — how the server serves AGP; §10 — test layers.
- `../adesk-core/CONTEXT.md` — the vocabulary this crate returns.
- `../adesk-proto/CONTEXT.md` — wire frames and payloads (designed in parallel).
- `../adesk-agent/CONTEXT.md` — primary consumer of this API.
