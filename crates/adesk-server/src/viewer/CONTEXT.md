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
- The backend owns one `ChangeSignal` (fed by the pump in `spawn_change_pump`) and one `InputQueue` shared by every connection, so input from concurrent viewers stays in submission order. Note this `InputQueue` is **runtime-global** (one per `ViewerBackendImpl`), unlike the per-connection `Session::input()` the AGP §5.5 handlers use — concurrent viewers' pointer/key/text serialize against each other, not just per connection.
- **Viewer input is never gated on control ownership.** `apply_input` (`backend.rs:577`) touches no `control` state, and `viewer::backend::ViewerBackendImpl::control` (`backend.rs:54`) is read only by `display()` (announced in `ServerHello`) and written only by `set_control` (`backend.rs:725`); no code path checks it before applying input. `set_control` is purely advisory: it overwrites the single runtime-global `Mutex<ControlOwner>` (no per-connection state) and is never cleared — not on disconnect, not by a second viewer, not on a timeout. `ControlOwner` is not consulted for any input decision.
- **VAP pointer coordinates are clamped, never rejected.** `output_fraction_position` (`../dispatch/input.rs:335`) calls `Position::normalized(x, y).resolve(output_rect)`, and `adesk_core::position::clamp01` maps `NaN → 0.0` and saturates infinities / out-of-range values into `0.0..=1.0`; no `invalid_request` is raised for a wild or non-finite fraction.

## Known Issues

- **A viewer pointer *button* can be swallowed with no error at all: the compositor answers `Ok` and nothing is delivered.** `crates/adesk-compositor/src/state.rs:656` `inject_pointer_button` checks only that *some* window is active, then calls Smithay `PointerHandle::button`, which early-returns when the pointer focus is `None` (Smithay 0.7 `src/input/pointer/mod.rs:271`); `inject_pointer_axis` (`state.rs:690`) is the same. `inject_pointer_move` (`state.rs:619`) is not: it always passes a focus surface and errors (`unknown_window`/`invalid_request`) when it cannot. So an unappliable **motion is loud** (an error the caller sees) while an unappliable **button/axis is silent** (an `Ok` with no delivery) — and because an error reply is the only way `apply_input` can fail, a silently-dropped click is invisible to `adesk-server`.
- **A Smithay `ClickGrab` pins pointer focus to the press-time surface.** On a press, `DefaultGrab::button` installs a `ClickGrab` whose `start_data.focus` is `current_focus()` (Smithay `src/input/pointer/grab.rs:233-250`); afterward `ClickGrab::motion` ignores the focus `inject_pointer_move` passes and re-targets the pinned surface (`grab.rs:357-365`) until a matching release empties `pressed_buttons` (`grab.rs:377-383`). A press whose release is lost — a VAP press with no matching release, or an AGP `mouse_down` never followed by `mouse_up` — therefore redirects *every subsequent* motion and button to that frozen surface, whatever `input_target` resolves; the grab is discarded only once the pinned surface is no longer alive, so a mapped-but-inactive window keeps it.
- **Viewer input is not ordered against AGP input.** The backend's `InputQueue` is private to `ViewerBackendImpl` while an AGP §5.5 client uses its own per-`Session` queue, so a viewer's `move_before` → `button_event` pair (`backend.rs:613-619`) can be interleaved by an AGP `pointer_move`/`activate_window`, landing the button somewhere other than where the viewer moved the pointer.
- `listener.rs:74` calls `ViewerServer::new(backend).with_config(ViewerServerConfig::default())` — a no-op, because `ViewerServer::new` already installs `ViewerServerConfig::default()`. The `ViewerServerConfig` import exists only for that call.
- `backend.rs::render_frame` hand-rolls `images::encode_png` + `ImagePayload::from_png(.., 1.0)`; `crate::images::encode(image, ImageFormat::Png, 1.0)` does exactly that in one call.
- `apply_input` re-implements the §5.5 *orchestration* (parse key → `record_action` → `activate_if_needed` → `send_unit`) that `dispatch/input.rs` handlers also perform, per `ViewerInput` arm. It reuses the `pub(crate)` seat helpers, so the duplication is at the sequence level, not the command level. The `ActivateWindow`/`CloseWindow` arms are not duplicated at all: they reuse `dispatch::windows::activate_window`/`close_window` wholesale.
- In-module `#[cfg(test)]` tests exist only for `backend.rs::app_entry` (the `AppInfo` → `AppEntry` projection and its empty-name fallback); everything else in these three files is covered by the E2E tests in `tests/viewer.rs`. The TCP accept arm and the Unix arm's error branch, `spawn_change_pump`/`is_desktop_change`, `cursor_state`/`normalize`, `no_active_window`, `not_a_single_key` and the `input_target` helper's `unknown_window` branch have no direct test.

## Notes for Agents

- `ViewerBackendImpl` is the canonical "second consumer" pattern for a further transport: implement the sibling crate's backend trait over `ServerContext`, bind before returning, reuse `dispatch::input`/`dispatch::windows`/`dispatch::apps`/`inspection`/`images`, never own teardown.
- Directional asymmetry to keep in mind when touching window resolution: `apply_input` and `activate_if_needed` use `keyboard_focus.or(active_window_id)`, while `dispatch/capture.rs::observed_window` uses `active_window_id.or(keyboard_focus)`.
