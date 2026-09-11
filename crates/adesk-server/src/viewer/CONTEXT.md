# adesk-server/src/viewer — runtime half of the VAP v1 endpoint

## Intent

This module is the *runtime* half of the Viewer Attachment Protocol (VAP v1, `docs/viewer.md`).
It implements `adesk_viewer::ViewerBackend` over `ServerContext`, binds the viewer transports (Unix socket + optional TCP) and accepts viewer connections.
It owns **no** protocol logic: the wire types/codec live in `adesk-viewer-proto` and the per-connection session in `adesk-viewer` (`ViewerServer::serve` / `run_session`).
Per `docs/viewer.md` §8, binding the transport is deliberately this crate's job — `adesk-viewer` never imports a listener.
Viewer input is never a special path: it records an `ActionId` on the observer before the compositor command and calls the same `pub(crate)` seat helpers `crate::dispatch::input` exposes for AGP §5.5.
Rendering is on demand: a frame is produced only when a session asks for one, and its `seq` is *reserved* from the compositor's single global counter, never read off the `QueryState` watermark.

## API Surface (all `pub(crate)`; nothing here is `pub`)

- `start(config: &ServerConfig, context: &ServerContext) -> Result<Option<PathBuf>, ServerError>` (`mod.rs`) — binds the transports and spawns the accept loops, returning the resolved Unix socket path or `None` when the endpoint is disabled. Bound *before* `Server::start` returns, so a returned `RunningServer` means VAP accepts connections.
- `ViewerBackendImpl` (`backend.rs`) — `new(ServerContext)` builds it and spawns the change pump (must be called inside a tokio runtime). Implements all six `ViewerBackend` methods: the four required ones (`display`, `render_frame`, `desktop_state`, `apply_input`) and *both* defaults (`change_signal`, `set_control`). A private `activate_window` method backs the `ViewerInput::ActivateWindow` arm by calling `crate::dispatch::windows::activate_window` through the shared `InputQueue`.
- `ViewerListener` (`listener.rs`) — `bind(unix_path, Option<SocketAddr>) -> Result<ViewerListener>` and `spawn_accept_loops(listener, context, backend)`. `ViewerListener { unix: SocketListener, tcp: Option<TcpListener> }`; `unix_path()` is only used inside `listener.rs`.

## Routing Table

| Area | Owner |
|---|---|
| Endpoint entry point: resolve path, build backend, bind, spawn loops | `./mod.rs` |
| `ViewerBackend` impl: frames, desktop state, input, change pump | `./backend.rs` |
| Transport binding + per-transport accept loops | `./listener.rs` |
| Wire types / codec (VAP) | `crates/adesk-viewer-proto/` |
| `ViewerServer`, per-connection session, `ViewerBackend` trait, `ChangeSignal` | `crates/adesk-viewer/` |
| Shared seat/state helpers reused by `apply_input` | `crates/adesk-server/src/dispatch/input.rs`, `crates/adesk-server/src/dispatch/windows.rs` |
| Frame render + PNG encode reused by `render_frame` | `crates/adesk-server/src/inspection.rs`, `crates/adesk-server/src/images.rs` |
| E2E coverage (the only suite here that connects a Wayland client) | `crates/adesk-server/tests/viewer.rs` |

## Constraints

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]` (crate level); every item here still carries `///` docs.
- `mod viewer` is declared `pub mod viewer` in `src/lib.rs` but exposes **zero** public items — every item is `pub(crate)`.
- The two accept loops must not own teardown: only the AGP accept loop runs `crate::shutdown::run`. These tasks hold the shutdown token and stop accepting; the socket file is removed by `shutdown::run`, with `SocketListener`'s RAII drop (keyed on the `(device, inode)` pair) as fallback.
- Both transports are served by one generic `accept_loop<T: ViewerTransport>`; `UnixTransport`/`TcpTransport` are private and differ only in the accepted stream type and the `PeerInfo` variant. `spawn_accept_loops` spawns one `accept_loop` per bound transport.
- The pointer/key/text `apply_input` arms require a target window, resolved by the free `input_target` helper (`snapshot.keyboard_focus.or(snapshot.active_window_id)`; VAP carries no `window_id` for them). No window ⇒ `invalid_request("no window is active")`, never a panic and never a silent drop. Keyboard input activates the target first; pointer input never does. The `ViewerInput::ActivateWindow` arm is the exception: it names a window explicitly and is runtime-native, so it resolves no target and delegates to `crate::dispatch::windows::activate_window` (which reports `unknown_window` for an unknown id).
- The backend owns one `ChangeSignal` (fed by the pump in `spawn_change_pump`) and one `InputQueue` shared by every connection, so input from concurrent viewers stays in submission order.

## Known Issues

- `listener.rs:74` calls `ViewerServer::new(backend).with_config(ViewerServerConfig::default())` — a no-op, because `ViewerServer::new` already installs `ViewerServerConfig::default()`. The `ViewerServerConfig` import exists only for that call.
- `backend.rs::render_frame` hand-rolls `images::encode_png` + `ImagePayload::from_png(.., 1.0)`; `crate::images::encode(image, ImageFormat::Png, 1.0)` does exactly that in one call.
- `apply_input` re-implements the §5.5 *orchestration* (parse key → `record_action` → `activate_if_needed` → `send_unit`) that `dispatch/input.rs` handlers also perform, per `ViewerInput` arm. It reuses the `pub(crate)` seat helpers, so the duplication is at the sequence level, not the command level. The `ActivateWindow` arm is not duplicated at all: it reuses `dispatch::windows::activate_window` wholesale.
- There are no in-module `#[cfg(test)]` tests in any of these three files; the only coverage is the 9 E2E tests in `tests/viewer.rs`. The TCP accept arm and the Unix arm's error branch, `spawn_change_pump`/`is_desktop_change`, `cursor_state`/`normalize`, `no_active_window`, `not_a_single_key` and the `input_target` helper's `unknown_window` branch have no direct test.

## Notes for Agents

- `ViewerBackendImpl` is the canonical "second consumer" pattern for a further transport: implement the sibling crate's backend trait over `ServerContext`, bind before returning, reuse `dispatch::input`/`dispatch::windows`/`inspection`/`images`, never own teardown.
- Directional asymmetry to keep in mind when touching window resolution: `apply_input` and `activate_if_needed` use `keyboard_focus.or(active_window_id)`, while `dispatch/capture.rs::observed_window` uses `active_window_id.or(keyboard_focus)`.
