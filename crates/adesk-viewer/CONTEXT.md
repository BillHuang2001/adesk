# adesk-viewer — VAP server session + client SDK + headless viewer binary

## Intent
`adesk-viewer` is the ADesk side of the Viewer Attachment Protocol (VAP v1, `docs/viewer.md`) plus the client that consumes it.
It has three responsibilities and nothing else:
1. a **server session** (`ViewerServer`) that, over a connected viewer, streams rendered desktop frames + metadata and applies the viewer's input through a device-agnostic `ViewerBackend` trait the runtime implements;
2. an **async client SDK** (`ViewerClient`) that connects over a Unix or TCP transport, performs the handshake, streams frames and sends human input;
3. a **headless `adesk-viewer` binary** that connects, captures frames to disk and can drive input from a script.
The crate is transport-agnostic (any `AsyncRead + AsyncWrite` stream), never links Smithay and never touches the compositor — `adesk-server` implements `ViewerBackend` and binds the transport.

## API Surface
Crate root (`src/lib.rs`) re-exports every public item below (`adesk_viewer::<Name>`).

### Errors (`src/error.rs`)
- `ViewerError` (`thiserror`): `Io`, `Protocol(ViewerProtoError)`, `Closed`, `Handshake(String)`, `Backend { code: ErrorCode, message: String }`, `VersionMismatch { client, server }`, `Transport(String)`.
- `Result<T, E = ViewerError>`.
- `Backend` carries the AGP `ErrorCode` the backend failure maps to, which the session sends as the VAP `error` code (`docs/viewer.md` §6) — so an unknown window keeps `unknown_window` on the wire.
- `VersionMismatch` mirrors `ViewerProtoError`'s field meaning: `client` is the peer's version, `server` is `PROTOCOL_VERSION`.

### Backend seam (`src/backend.rs`)
- `#[async_trait] trait ViewerBackend: Send + Sync + 'static` — the runtime implements exactly this:
  - `fn display(&self) -> ServerHello` — output size, runtime version, renderer, initial cursor, control owner (used for the handshake reply).
  - `async fn render_frame(&self) -> Result<ViewerFrame>` — render the current desktop into an `ImagePayload` + cursor + active window.
  - `async fn desktop_state(&self) -> Result<DesktopState>` — window list + active window.
  - `async fn apply_input(&self, input: ViewerInput) -> Result<Option<ActionId>>` — apply one viewer action; pointer/key/text input goes through the seat, `ViewerInput::ActivateWindow` changes compositor window state directly. `Some(action_id)` when the runtime recorded an action.
  - `fn change_signal(&self) -> ChangeSignal { ChangeSignal::never() }` — notified when the desktop changes (a commit/damage/window event), so frames are pushed on demand.
  - `async fn set_control(&self, owner: ControlOwner) -> Result<()> { Ok(()) }` — advisory ownership handshake (a no-op by default).
- `ViewerInput` — the viewer actions the backend sees (no handshake/protocol traffic): `PointerMove { x, y }`, `PointerButton { button, state, x, y }`, `Scroll { dx, dy, x, y }`, `Key { keys, action }`, `Text { text }`, `ActivateWindow { window_id }`.
  - Positions are **normalized** `0.0..=1.0` output coordinates and optional; the pointer/key/text variants carry no `window_id` — the runtime resolves them to output pixels and targets its active window through its window model.
  - `ActivateWindow` names a window explicitly and is **runtime-native**: the backend changes compositor window state directly (exactly like AGP §5.3 `activate_window`), never synthesized input (`docs/viewer.md` §5).
  - `button` is an `adesk_core::Button`, `keys` an `adesk_proto::KeySpec`, `action` a `KeyAction`, `window_id` an `adesk_core::WindowId`.
- `ChangeSignal` — cheap-clone "desktop changed" notifier: `new()`, `never()`, `notify()`, `async changed(&self)`.

### Server session (`src/server.rs`, `src/session.rs`)
- `ViewerServer<B: ViewerBackend>` — `new(Arc<B>)`, `with_config(ViewerServerConfig)`, `config()`, and `async serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(&self, stream: S, peer: PeerInfo) -> Result<()>` which runs one viewer connection to completion.
- `ViewerServerConfig` — `handshake_timeout` (default 5 s), `max_frame_len` (default `DEFAULT_MAX_FRAME_LEN`), `default_min_interval_ms` (default `0` = no default pacing), `default_overlays` (default `adesk_viewer_proto::DEFAULT_OVERLAYS`); builders.
- `PeerInfo` — a display label for logs (`Unix(PathBuf)` / `Tcp(SocketAddr)` / `Other(String)`) + `Display`.

### Transport framing (`src/transport.rs`)
- `DEFAULT_MAX_FRAME_LEN` (32 MiB) — the one shared line cap for server and client.
- `async read_line<R: AsyncBufRead + Unpin>(&mut R, max_len) -> Result<Option<String>>` — the owned-`String` convenience reader; strips the terminator, `Ok(None)` on EOF, `Err(Transport)` on an over-cap or non-UTF-8 line.
- `async read_line_into<R: AsyncBufRead + Unpin>(&mut R, &mut Vec<u8>, max_len) -> Result<bool>` — the allocation-reusing reader used by the server session loop and the client dispatcher: clears and refills a caller-owned buffer with the line's raw bytes (terminator stripped), `Ok(false)` on clean EOF, `Ok(true)` otherwise, UTF-8 validated in place. Callers decode the buffer as `&str`.
- `async write_line<W: AsyncWrite + Unpin>(&mut W, &str) -> Result<()>` — offers the line and its `\n` in a single vectored write (a short write is completed in place), then flushes once.
- `src/lib.rs` additionally re-exports `read_line`/`read_line_into`/`write_line` under `#[doc(hidden)] pub use` so the integration tests reuse the crate's framing instead of hand-rolling it; this is additive and outside the documented surface.
- This is the single place byte framing lives.

### Client SDK (`src/client.rs`)
- `ViewerTarget::{Unix(PathBuf), Tcp(SocketAddr)}` + `Display`.
- `ViewerClient` — `connect(ViewerTarget)`, `connect_with(ConnectOptions)`; `hello() -> &ServerHello`, `target() -> &ViewerTarget`, `socket_path() -> Option<&Path>`; `frames() -> impl Stream<Item = Result<ViewerFrame>>`; `request_frame()`, `request_state()`, `pointer_move(x, y)`, `pointer_button(button, state, pos)`, `scroll(dx, dy, pos)`, `key(KeySpec, KeyAction)`, `text(text)`, `activate_window(WindowId)`, `set_control(owner)`, `input_ack() -> impl Stream<Item = (Option<u64>, ActionId)>`, `async close(self) -> Result<()>`.
- `ConnectOptions` (`#[non_exhaustive]`): `target`, `max_frame_len`, `connect_timeout`, `handshake_timeout`, `client_name`, `overlays`, `min_interval_ms`, `verify_version` (default `true`); `new` + `with_*` builders.
- `DEFAULT_CONNECT_TIMEOUT` / `DEFAULT_HANDSHAKE_TIMEOUT` (5 s each), `DEFAULT_MAX_FRAME_LEN` (re-exported from `transport`).

### Frame capture helpers (`src/capture.rs`)
- `save_frame_png(&ImagePayload, &Path) -> Result<()>` — decode an `ImagePayload` and write a PNG (used by the binary and tests).
- `write_rgba8(&ImageBuffer, &Path) -> Result<()>`.
- `FrameWriter` — `new(dir)`, `directory()`, `write(&ViewerFrame) -> Result<PathBuf>` writing `frame-<seq:08>.png` into the directory.

### Input scripts (`src/script.rs`)
- `parse_script(&str) -> Result<Vec<ScriptCommand>, ScriptError>`; every `ScriptError` variant carries the 1-based `line` it occurred on.
- `ScriptCommand`: `Move { x, y }`, `Click { button }`, `Down { button }`, `Up { button }`, `Scroll { dx, dy }`, `Key { keys, action }`, `Text { text }`, `ActivateWindow { window_id }`, `Control { owner }`, `Wait { ms }`, `Capture { path }`.
- Grammar — one command per line, whitespace-separated tokens, blank lines and `#` comments ignored:
  `move X Y`, `click [BUTTON]` (left when omitted), `down BUTTON`, `up BUTTON`, `scroll DX DY`, `key KEYS...` (one token = key, several = chord, always a tap), `type TEXT` (rest of line verbatim), `activate WINDOW_ID` (numeric AGP window id; the runtime-native window switch), `control ai|human`, `wait MS`, `capture FILE` (single token).

### Binary (`src/main.rs`)
- `adesk-viewer` (clap): transport `--unix <PATH>` (default `$XDG_RUNTIME_DIR/adesk-viewer.sock`, else `<temp_dir>/adesk-viewer.sock`) / `--tcp <HOST:PORT>` (mutually exclusive); `--fps <N>` (→ `min_interval_ms = 1000 / N`, `0` = unpaced); `--overlays <LIST>`; `--log <FILTER>` (env `ADESK_LOG`, default `info`).
- One mutually exclusive mode ArgGroup: `--capture <FILE>` (one frame then exit) | `--follow --out-dir <DIR> [--max-frames N] [--duration-ms M]` | `--input <FILE>` | `--input-stdin`.
- Exit codes: `0` success, `1` runtime/connection failure, `2` usage or configuration error.

## Constraints
- `docs/viewer.md` is normative; the crate invents no message or field — it speaks only `adesk-viewer-proto`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay well under the ~1000-line threshold (largest: `src/session.rs` 829, `src/client.rs` 823).
- Transport-agnostic: the crate never imports a listener (`tokio::net::UnixListener`, `std::os::unix::net::*`) — `adesk-server` owns binding; `ViewerTarget` is the only place socket/TCP addresses appear.
- No panics on connection/input paths: every failure returns `ViewerError`. Empty `pub mod` stubs are not viable here — `missing_docs` requires at least a `//!` module doc.
- No pixel payloads in logs; `tracing` at `debug`/`trace` for transport internals only.
- Dependencies come only from root `[workspace.dependencies]`; never inline versions.
- The server session must never block the runtime: it renders only while a viewer is attached and the desktop changed (or the pacing timer fires), matching the on-demand-rendering invariant.

## Routing Table
| Area | Owner |
|---|---|
| Error type + VAP error mapping | `./src/error.rs` |
| `ViewerBackend`, `ViewerInput`, `ChangeSignal` | `./src/backend.rs` |
| NDJSON line framing + `DEFAULT_MAX_FRAME_LEN` | `./src/transport.rs` |
| Per-connection session state machine (handshake, select loop, pacing, message dispatch) | `./src/session.rs` |
| `ViewerServer` façade + `ViewerServerConfig`/`PeerInfo` | `./src/server.rs` |
| `ViewerClient`, `ViewerTarget`, `ConnectOptions` | `./src/client.rs` |
| Frame → PNG capture helpers | `./src/capture.rs` |
| Input-script grammar + parser | `./src/script.rs` |
| CLI wiring, mode ArgGroup, exit codes | `./src/main.rs` |
| `#[cfg(test)]` shared `ViewerBackend` fake for the inline unit tests | `./src/test_support.rs` |
| Server-session tests over an in-memory duplex stream | `./tests/session.rs` |
| Client round-trip tests over a real Unix socket | `./tests/client.rs` |
| Input-script parser tests | `./tests/script.rs` |
| Shared integration-test `FakeBackend` + scaffolding | `./tests/common/mod.rs` |

## Design Decisions
- **The backend trait is the only runtime coupling.** `ViewerServer` depends on `ViewerBackend`, never on the compositor; `adesk-server` implements it over its render command + seat input. This keeps the viewer reusable and testable with a fake backend.
- **Frames are pushed on change, paced by `min_interval_ms`.** The session awaits the `ChangeSignal` or the pacing deadline, renders one frame, and writes it; a viewer that only wants a snapshot sends `request_frame` instead. There is no initial frame and no background render loop without a viewer.
- **`ViewerServerConfig::default_min_interval_ms == 0` means "no default pacing"**, so a viewer that also asks for `min_interval_ms == 0` gets a frame per desktop change; a non-zero config value is substituted when the viewer asks for `0`.
- **`ChangeSignal` collapses.** One wakeup per `notify()`, with a single stored permit when there is no waiter — the right semantics for "something changed, render once".
- **One session task per connection.** Unlike the AGP server (concurrent dispatch), viewer messages are cheap and order-sensitive (input), so a single `select!` loop per connection applies them in submission order — mirroring the AGP §5.5 input ordering guarantee. Input messages carry no `id`, so `InputAck.id` is always `None`.
- **Handshake is mandatory and version-checked.** The session refuses (protocol `error` + close) a malformed first line, a missing/`Unknown`/duplicate `hello`, and a version mismatch; a silent viewer hits `handshake_timeout` and `serve` returns `Err(Handshake)`. The client refuses the same on the server's reply unless `verify_version` is disabled.
- **`ViewerInput` is a narrowing of `ClientMessage`.** The backend never sees handshake/`request_frame`/`bye` traffic, so the trait stays stable if the protocol grows non-input messages.
- **Close is a graceful handshake, and a peer-gone ack is not an error.** `ViewerClient::close()` writes `bye`, shuts its write half, then waits a bounded grace (250 ms) for the dispatcher to read the server's `bye`/EOF and end naturally before aborting it; either way it returns `Ok(())`. On the server, a write failure while acking an *already-received* `bye` is downgraded to the normal clean close only for `BrokenPipe`/`ConnectionReset` (logged at `debug`) — any other error kind still propagates.
- **The client uses a background dispatcher.** One task owns the read half and fans messages into `broadcast` channels (frames, input acks, errors) and a `oneshot` FIFO (state replies), so `frames()`, `input_ack()`, `request_*` and the input methods are usable concurrently without deadlock; `Drop` aborts the dispatcher.
- **Overlays are negotiated, not plumbed into v1 rendering.** `render_frame()` takes no overlay argument, so `overlays` travels through the handshake and is reported, but the backend decides what an overlay set means for a given frame.
- **Exit-code mapping is fixed** (`0`/`1`/`2`) so the binary is safe to script; unrecognized overlay names and unreadable `--input` files are usage errors, a missing socket is a runtime error.
- **One shared test fake per layer.** Each layer has a single configurable `FakeBackend` with builder knobs instead of one copy per test module: `src/test_support.rs` (`#[cfg(test)]`) for the inline tests, `tests/common/mod.rs` for the integration tests. A single fake cannot span both layers without exposing a public test-support API, so two is the floor.

## Performance Notes (frame-streaming hot path)
Every streamed frame carries the full base64 pixel payload (`ImagePayload::data`), so memcpy/allocation of that string — not CPU — dominates per-frame cost; the sites below are where copies still happen.
- **Server encode:** `session::send` calls `adesk_viewer_proto::encode_server`, which builds a `serde_json::Value` tree and then stringifies it (in `adesk-viewer-proto`: `codec.rs`, `message.rs`).
  That copies the whole base64 payload into a `Value::String` and again into the output `String`, plus a double `Map` allocation per frame.
  It applies to both pushed frames and `request_frame` replies.
- **Client delivery:** `dispatch` moves each decoded frame into the bounded `broadcast` channel; every `frames()`/`request_frame` `recv()` clones the full `ViewerFrame` including the base64 `String`.
  `FRAME_CHANNEL_CAPACITY = 32` bounds how many frames are retained, but each retained slot holds a full payload.
- **Capture:** `save_frame_png` base64-decodes the payload into a `Vec<u8>` and hands ownership to `tight_rgba8` as a `Cow<[u8]>`, so an already-tight buffer is returned unchanged (no repack copy); only a row-padded buffer allocates. `--follow` therefore does one broadcast clone + one decode + a PNG encode per frame.
- **Framing:** `write_line` offers the line and its terminator to a single vectored write, then one flush; `read_line_into` refills a caller-owned `Vec<u8>` in place, so the session loop and client dispatcher allocate nothing per inbound line (they decode the buffer as `&str`).
- **Deliberately not done (would change public API / wire handling):** a manual streaming serializer to avoid the `Value`-tree encode, holding frames as `Arc<ViewerFrame>` in the broadcast channel to avoid the per-`recv` payload clone, and a `min_interval` policy change. These remain as-is.
- **No producer/consumer lock contention on the server:** one session task renders, encodes and writes each frame with no shared lock, and the `Notify`-based change signal (backend.rs) collapses a busy desktop to at most one queued frame, so there is no unbounded producer queue.

## Test Strategy
No display, GPU or real network; a fake `ViewerBackend` plus an in-memory duplex stream or a `tempfile` Unix socket.
- `tests/session.rs` — 12 tests on `tokio::io::duplex`: handshake metadata, version mismatch, non-hello/malformed first line, handshake timeout, `request_frame`/`request_state` round trip, change-driven frame push, every input variant (incl. `activate_window`) forwarded in order + `input_ack`, `set_control` echo, `bye` echo, unknown-type tolerance.
- `tests/client.rs` — 11 tests against a real `ViewerServer` on a `tokio::net::UnixListener` inside a `tempfile::TempDir`: connect/handshake, frame stream (`frames()` push on a desktop change), every input method incl. `scroll`/`text`/`activate_window`/`set_control`, server-initiated `bye`, and `close()` (the clean-close assertions are looped and repeated on a multi-thread runtime so a reintroduced teardown race fails the suite).
- `tests/script.rs` — 16 tests for the input-script grammar and error line numbers (this suite fully covers the parser; `src/script.rs` has no inline tests).
- Inline unit tests: 47 in the lib target (backend, transport, capture, session, server, client, error, test_support) and 15 in the bin target (CLI parsing, exit-code mapping, `--fps` mapping, default socket path).
- `tests/CONTEXT.md` records the remaining audit notes and coverage gaps in this suite (the ~6 `src/session.rs` inline tests already covered by `tests/session.rs`; no TCP-transport or multi-connection-fan-out test); read it before adding parser/session tests.
- Run with `./scripts/dev.sh cargo test -p adesk-viewer` → **101 passed / 0 failed / 0 ignored** (47 lib + 15 bin + 11 client + 16 script + 12 session; 0 doc-tests).
- Also green: `cargo clippy -p adesk-viewer --all-targets --no-deps -- -D warnings`, `cargo fmt -p adesk-viewer --check`, `cargo doc -p adesk-viewer --no-deps --document-private-items` (warning-free), and `cargo check --workspace --all-targets`.

## Known Issues
- `capture::write_rgba8` is public and re-exported but has no caller anywhere in the workspace (only its own unit tests); `save_frame_png` and `FrameWriter` are the capture helpers the binary actually uses.
- The `adesk-viewer` binary reads only `ADESK_LOG`; it has no `ADESK_VIEWER_SOCKET`/`ADESK_VIEWER_TCP` fallback and derives `$XDG_RUNTIME_DIR/adesk-viewer.sock` itself, so it cannot follow a runtime started with `--viewer-socket` unless `--unix` is passed. Those env vars exist only on `adesk-server`'s CLI.
- `Cargo.toml` declares `serde` but no source file references it; the only JSON use is `serde_json` in `main.rs` overlay-name parsing.
- `capture::tight_rgba8` (row de-padding) duplicates `adesk-server::images::tightly_packed`, and `capture::encode_rgba8_png` overlaps `adesk-server::images::encode_png` — a cross-crate overlap left as-is (resolving it means changing `adesk-server`).

## Notes for Agents
- The server session owns no transport: `serve` takes an already-connected stream; binding/accepting lives in `adesk-server`.
- `change_signal()` default `never()` means a backend with no event source still serves `request_frame`/pacing — used by tests and simple backends.
- The binary must never require a display: "rendering" a frame means writing a PNG, and input is script-driven.
- Test scaffolding reuses the crate's own framing: `tests/session.rs` `send`/`send_raw` call the re-exported `adesk_viewer::write_line` (no local helper), both integration files share one `FakeBackend` from `tests/common/mod.rs`, and the inline `src/session.rs`/`src/server.rs` tests share `src/test_support.rs`. No `test-support` feature exists; `tempfile` is the only dev-dependency.
- `close()` can block up to the 250 ms grace only when the peer never answers; the happy path returns as soon as the server's `bye`/EOF arrives.
- `adesk-server` is the real consumer: `crates/adesk-server/src/viewer/backend.rs` implements `ViewerBackend` (render via `inspection::refresh` + `images::encode_png`, state via `dispatch::windows::state`, input via the widened `pub(crate)` `dispatch::input` helpers) and `crates/adesk-server/src/viewer/listener.rs` binds the Unix/TCP transports and serves one `ViewerServer` per runtime.

## Status
Implementation-complete, documented and tested: all modules, the client SDK and the headless binary are landed, and the crate's own test/clippy/fmt/doc gates are green (counts in Test Strategy).
`adesk-server` consumes the crate end to end: it implements `ViewerBackend`, binds both transports and serves the endpoint by default, and `crates/adesk-server/tests/` drives the typed `ViewerClient`, so the server session, `PeerInfo`, the config builders and the client SDK all have real callers.
