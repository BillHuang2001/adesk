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
- `ViewerError` (`thiserror`): `Io`, `Protocol(ViewerProtoError)`, `Closed`, `Handshake(String)`, `Backend(String)`, `VersionMismatch { client, server }`, `Transport(String)`.
- `Result<T, E = ViewerError>`.
### Backend seam (`src/backend.rs`)
- `#[async_trait] trait ViewerBackend: Send + Sync + 'static` — the runtime implements exactly this:
  - `fn display(&self) -> ServerHello` — output size, runtime version, renderer, initial cursor, control owner (used for the handshake reply).
  - `async fn render_frame(&self) -> Result<ViewerFrame>` — render the current desktop into an `ImagePayload` + cursor + active window.
  - `async fn desktop_state(&self) -> Result<DesktopState>` — window list + active window.
  - `async fn apply_input(&self, input: ViewerInput) -> Result<Option<ActionId>>` — apply one viewer input through the seat; `Some(action_id)` when the runtime recorded an action.
  - `fn change_signal(&self) -> ChangeSignal { ChangeSignal::never() }` — notified when the desktop changes (a commit/damage/window event), so frames are pushed on demand.
  - `async fn set_control(&self, owner: ControlOwner) -> Result<()> { Ok(()) }` — advisory ownership handshake (a no-op by default).
- `ViewerInput` — the input subset the backend sees (no handshake/protocol traffic): `PointerMove { x, y }`, `PointerButton { button, state, x, y }`, `Scroll { dx, dy, x, y }`, `Key { keys, action }`, `Text { text }`.
- `ChangeSignal` — a cheap-clone "desktop changed" notifier: `new()`, `never()`, `notify()`, `async changed(&self)`.
### Server session (`src/server.rs`, `src/session.rs`, `src/transport.rs`)
- `ViewerServer<B: ViewerBackend>` — `new(Arc<B>)`, `with_config(ViewerServerConfig)`; `async serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(&self, stream: S, peer: PeerInfo) -> Result<()>` runs one viewer connection to completion.
- `ViewerServerConfig` — `handshake_timeout`, `max_frame_len` (default 32 MiB), `default_min_interval_ms`, `default_overlays`; builders.
- `PeerInfo` — a display label for logs (`Unix(PathBuf)` / `Tcp(SocketAddr)` / `Other(String)`).
- `transport.rs` — line-framed read/write over any stream: `read_line`/`write_line` (NDJSON), enforcing `max_frame_len`; the single place the byte framing lives.
### Client SDK (`src/client.rs`)
- `ViewerTarget::{Unix(PathBuf), Tcp(SocketAddr)}` + `Display`.
- `ViewerClient` — `connect(ViewerTarget)`, `connect_with(ConnectOptions)`; `hello() -> &ServerHello`, `socket_path()`/`target()`; `frames() -> impl Stream<Item = Result<ViewerFrame>>`; `request_frame() -> Result<ViewerFrame>`; `request_state() -> Result<DesktopState>`; `pointer_move(x, y)`, `pointer_button(button, state, pos)`, `scroll(dx, dy, pos)`, `key(keys, action)`, `text(text)`, `set_control(owner)`, `input_ack()` stream; `close(self) -> Result<()>`.
- `ConnectOptions` (`#[non_exhaustive]`): `target`, `max_frame_len`, `connect_timeout`, `handshake_timeout`, `client_name`, `overlays`, `min_interval_ms`, `verify_version` (default true).
- `DEFAULT_MAX_FRAME_LEN` (32 MiB), `DEFAULT_CONNECT_TIMEOUT`, `DEFAULT_HANDSHAKE_TIMEOUT`.
### Frame capture helpers (`src/capture.rs`)
- `save_frame_png(&ImagePayload, &Path) -> Result<()>` — decode an `ImagePayload` and write a PNG (used by the binary and tests); `write_rgba8(&ImageBuffer, &Path)`.
- `FrameWriter` — writes successive frames into a directory as `frame-<seq:08>.png`, returning the path written.
### Binary (`src/main.rs`)
- `adesk-viewer` (clap): transport `--unix <PATH>` (default the runtime-derived path) / `--tcp <HOST:PORT>`; `--fps <N>` (→ `min_interval_ms`), `--overlays <list>`; capture: `--capture <FILE>` (one frame then exit) or `--follow --out-dir <DIR> --max-frames <N> --duration-ms <M>`; input: `--input <FILE>` / `--input-stdin` (a script of `move X Y`, `click [BUTTON]`, `down/up BUTTON`, `scroll DX DY`, `key KEYS...`, `type TEXT`, `control ai|human`, `wait MS`, `capture FILE`, `# comment`); `--log <FILTER>`.
- Exit codes: `0` success, `1` runtime error, `2` config/CLI error.
## Constraints
- `docs/viewer.md` is normative; the crate invents no message or field — it speaks only `adesk-viewer-proto`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay well under the ~1000-line threshold.
- Transport-agnostic: the crate never imports `std::os::unix::net`, `tokio::net::UnixListener` or a listener — `adesk-server` owns binding; the client dialect (`ViewerTarget`) is the only place socket/TCP addresses appear.
- No panics on connection/input paths: every failure returns `ViewerError`.
- No pixel payloads in logs; `tracing` at `debug`/`trace` for transport internals only.
- Dependencies come only from root `[workspace.dependencies]`; never inline versions.
- The server session must never block the runtime: it renders only while a viewer is attached and the desktop changed (or the pacing timer fires), matching the on-demand-rendering invariant.
## Routing Table
| Area | Owner |
|---|---|
| Error type + PG error mapping | `./src/error.rs` |
| `ViewerBackend`, `ViewerInput`, `ChangeSignal` | `./src/backend.rs` |
| NDJSON line framing over any stream | `./src/transport.rs` |
| Per-connection session state machine (handshake, select loop, pacing) | `./src/session.rs` |
| `ViewerServer` façade + `ViewerServerConfig`/`PeerInfo` | `./src/server.rs` |
| `ViewerClient`, `ViewerTarget`, `ConnectOptions` | `./src/client.rs` |
| Frame → PNG capture helpers | `./src/capture.rs` |
| CLI wiring | `./src/main.rs` |
| Server-session tests over an in-memory duplex stream | `./tests/session.rs` |
| Client round-trip tests over a real Unix socket | `./tests/client.rs` |
| Input-script parser tests | `./tests/script.rs` |
## Design Decisions
- **The backend trait is the only runtime coupling.** `ViewerServer` depends on `ViewerBackend`, never on the compositor; `adesk-server` implements it over its render command + seat input. This keeps the viewer reusable and testable with a fake backend.
- **Frames are pushed on change, paced by `min_interval_ms`.** The session awaits the `ChangeSignal` or the pacing deadline, renders one frame, and writes it; a viewer that only wants a snapshot sends `request_frame` instead. No background render loop exists without a viewer.
- **One session task per connection.** Unlike the AGP server (concurrent dispatch), viewer messages are cheap and order-sensitive (input), so a single select loop per connection applies them in submission order — mirroring the AGP §5.5 input ordering guarantee.
- **Handshake is mandatory and version-checked.** `serve` refuses (error + close) a missing/`Unknown` first message, a version mismatch, or a handshake timeout; the client refuses the same on the server's reply.
- **`ViewerInput` is a narrowing of `ClientMessage`.** The backend never sees handshake/`request_frame`/`bye` traffic, so the trait stays stable if the protocol grows non-input messages.
## Test Strategy
Tests in `./tests/`, no display/GPU/network; a fake `ViewerBackend` and an in-memory duplex stream:
- `tests/session.rs` — handshake (version check, timeout), `request_frame`/`request_state` round-trip, change-driven frame push, input forwarding to the backend + `input_ack`, `set_control`, `bye`, malformed line handling.
- `tests/client.rs` — `ViewerClient` against a `ViewerServer` on a real `tokio::net::UnixListener` in a `tempfile` dir: handshake, frame stream, input methods, close.
- `tests/script.rs` — the headless input-script parser.
- Run with `./scripts/dev.sh cargo test -p adesk-viewer`.
## Notes for Agents
- The server session owns no transport: `serve` takes an already-connected stream; binding/accepting lives in `adesk-server`.
- `change_signal()` default `never()` means a backend that has no event source still serves `request_frame`/pacing — used by tests and simple backends.
- The binary must never require a display: "rendering" a frame means writing a PNG, and input is script-driven.
## Status
Skeleton only: `src/lib.rs` carries the crate attributes and docs; every module above is **not yet implemented**. Implement the surface exactly as documented here.
