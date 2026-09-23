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
- `ViewerBackendImpl` (`backend.rs`) — `new(ServerContext)` builds it and spawns the change pump (must be called inside a tokio runtime). Implements every `ViewerBackend` method: the four required ones (`display`, `render_frame`, `desktop_state`, `apply_input`), the recording trio (`start_recording`, `stop_recording`, `recording_status`), the app pair (`list_apps`, `launch_app`) and the two advisory defaults (`change_signal`, `set_control`). Two private methods back the runtime-native `ViewerInput` arms through the shared `InputQueue`: `activate_window` calls `crate::dispatch::windows::activate_window` and `close_window` calls `crate::dispatch::windows::close_window`. `list_apps`/`launch_app` reuse `crate::dispatch::apps::list_apps`/`launch_app` verbatim; a private `runtime_context` helper builds the throwaway `Session` + `RequestContext` those reuse sites need.
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
- The pointer/key/text `apply_input` arms require a target window, resolved by the free `input_target` helper (`snapshot.keyboard_focus.or(snapshot.active_window_id)`; VAP carries no `window_id` for them). No window ⇒ `invalid_request("no window is active")`, never a panic and never a silent drop. Keyboard input activates the target first; pointer input never does. The `ViewerInput::ActivateWindow` and `ViewerInput::CloseWindow` arms are the exceptions: they name a window explicitly and are runtime-native, so they resolve no target and delegate to `crate::dispatch::windows::activate_window`/`close_window` (which report `unknown_window` for an unknown id).
- `list_apps`/`launch_app` are the runtime's registry surface over VAP. `list_apps` reuses the AGP §5.2 handler with `include_hidden = false` (VAP lists the applications a human can launch; AGP's own default excludes `Hidden`/`NoDisplay`) and projects each `adesk_core::AppInfo` onto the wire `AppEntry` via the free `app_entry` helper (id, name — falling back to the id when the entry's `Name` is empty, icon, raw categories). `launch_app` reuses the §5.2 handler too and reports `action_id: None` (the launch path records no observer action) and `window_id: None` (the reply precedes the window mapping; the viewer discovers the window through `request_state`/`state`, and no blocking wait is added that would stall the session loop).
- `ViewerInput::PointerButton` and `ViewerInput::Scroll` always move the pointer before acting, through the free `move_before` helper (`backend.rs`): an explicit `x`/`y` pair (both or neither, per `viewer_position`) resolves through `dispatch::input::output_fraction_position`, an absent pair falls back to §2's default (the current tracked cursor when it lies inside the target window, else the window centre) via `dispatch::input::move_to`. So unlike AGP §5.5 pointer methods, a VAP button/scroll never relies on a preceding `pointer_move` — the compositor delivers the button/axis at whatever position the just-issued move set. `ViewerInput::PointerMove` uses the same `output_fraction_position` path.
- Input ownership is advisory and never gates input: `control: Mutex<ControlOwner>` is initialized to `ControlOwner::Ai`, written by `set_control` and read only by `display()` (the handshake/stater report); `apply_input` consults it nowhere and applies every message regardless of owner (`docs/viewer.md` §5). Gating is coordinated above ADesk.
- The backend owns one `ChangeSignal` (fed by the pump in `spawn_change_pump`) and one `InputQueue` shared by every connection, so input from concurrent viewers stays in submission order.

## Known Issues

- `listener.rs:74` calls `ViewerServer::new(backend).with_config(ViewerServerConfig::default())` — a no-op, because `ViewerServer::new` already installs `ViewerServerConfig::default()`. The `ViewerServerConfig` import exists only for that call.
- `backend.rs::render_frame` hand-rolls `images::encode_png` + `ImagePayload::from_png(.., 1.0)`; `crate::images::encode(image, ImageFormat::Png, 1.0)` does exactly that in one call.
- `apply_input` re-implements the §5.5 *orchestration* (parse key → `record_action` → `activate_if_needed` → `send_unit`) that `dispatch/input.rs` handlers also perform, per `ViewerInput` arm. It reuses the `pub(crate)` seat helpers, so the duplication is at the sequence level, not the command level. The `ActivateWindow`/`CloseWindow` arms are not duplicated at all: they reuse `dispatch::windows::activate_window`/`close_window` wholesale.
- In-module `#[cfg(test)]` tests exist only for `backend.rs::app_entry` (the `AppInfo` → `AppEntry` projection and its empty-name fallback); everything else in these three files is covered by the E2E tests in `tests/viewer.rs`. The TCP accept arm and the Unix arm's error branch, `spawn_change_pump`/`is_desktop_change`, `cursor_state`/`normalize`, `no_active_window`, `not_a_single_key` and the `input_target` helper's `unknown_window` branch have no direct test.

## Notes for Agents

- `ViewerBackendImpl` is the canonical "second consumer" pattern for a further transport: implement the sibling crate's backend trait over `ServerContext`, bind before returning, reuse `dispatch::input`/`dispatch::windows`/`dispatch::apps`/`inspection`/`images`, never own teardown.
- Directional asymmetry to keep in mind when touching window resolution: `apply_input` and `activate_if_needed` use `keyboard_focus.or(active_window_id)`, while `dispatch/capture.rs::observed_window` uses `active_window_id.or(keyboard_focus)`.
