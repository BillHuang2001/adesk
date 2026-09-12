# dispatch — AGP §5 request handlers

## Intent

`src/dispatch/` implements every AGP method of `docs/protocol.md` §5.1–§5.10: one handler per method, one response per request, no panic on a request path.
`mod.rs` owns the router (`Dispatcher::dispatch`), the error→wire conversion (`error_response`) and the outbound-sink seam.
Each group file owns one protocol section: `runtime.rs` §5.1, `apps.rs` §5.2, `windows.rs` §5.3, `capture.rs` §5.4, `input.rs` §5.5, `events.rs` §5.6 + §5.10, `inspect.rs` §5.7, `notify.rs` §5.9.
Handlers are thin adapters: they translate proto params into sibling-crate calls (`adesk-compositor`, `adesk-observer`, `adesk-app-registry`, `adesk-inspector`, `adesk-notify`) and never re-implement sibling logic.

## API Surface

Router (`mod.rs`):
- `RequestContext<'a> { server: &'a ServerContext, session: &'a Session }` — the only handle handlers receive.
- `Dispatcher::new(ServerContext)` / `context()` / `async dispatch(&Session, RequestFrame) -> ResponseFrame` — total over `adesk_proto::Method`; every `Err` becomes `error_response` and the connection stays open. `dispatch` opens no span itself: it runs inside the `request{id method}` span the read loop (`crate::connection::read_loop`) instruments the task with.
- `error_response(id, &ServerError) -> ResponseFrame` = `ResponseFrame::error(id, error.payload())`.
- Sink seam: `pub(crate) session_sink(&Session) -> Result<EventSink>`, `pub(crate) register_session_sink(SessionId, EventSink)`, `pub(crate) forget_session_sink(SessionId)`; `Connection::run` publishes the writer queue on connect and forgets it on disconnect.

Group handlers — all `pub async fn (ctx: &RequestContext<'_>, params: <Proto>Params) -> Result<<Proto>Result>`:
- `runtime::ping`.
- `apps::{list_apps, get_app, launch_app}`.
- `windows::{list_windows, get_window, activate_window, close_window, get_focus}`.
- `capture::{capture_window, capture_region, observe, wait_for_change, wait_for_quiet}`.
- `input::{pointer_move, click, double_click, mouse_down, mouse_up, scroll, drag, keypress, key_down, key_up, type_text}`.
- `events::{subscribe_events, unsubscribe_events, wait_for_events}` (`wait_for_events` is §5.10).
- `inspect::{inspect_capture, inspect_subscribe}`.
- `notify::{post_notification, list_notifications, close_notification, invoke_notification_action}`.

Shared internal helpers (not public API):
- `command.rs` (`pub(crate) mod`, `send_infallible`/`send_result`): the shared compositor-command seam every group uses — build a `RuntimeCommand` around a `oneshot` reply, send, await and classify. `send_infallible` is for the infallible reply shapes (`QueryState`, `ReserveSeq`), `send_result` for the `adesk_core::Result<T>` shapes (seat + window commands); a *dropped* reply is classified by a caller-supplied closure (the seat/state helpers report `shutting_down`, `activate_window`/`close_window`/`render_window` report `internal`), and a failure reply maps through `windows::command_error`.
- `windows.rs` is the canonical home of the compositor bridge: `pub(crate) async state(server: &ServerContext) -> Result<StateSnapshot>` (the only `QueryState` read; a dropped reply is `shutting_down`), `pub(crate) async reserve_seq(server: &ServerContext) -> Result<u64>` (the only `seq` allocator for server-synthesized events, called by `apps.rs`, `inspect.rs` and `viewer/backend.rs`; a closed command channel or a dropped reply is `shutting_down`), `pub(crate) command_error(Option<WindowId>, adesk_core::Error) -> ServerError` (preserves the compositor's AGP code across the `adesk_core::Error` boundary), `pub(crate) unknown_window(WindowId) -> ServerError`.
- `input.rs` exposes its seat/state seam `pub(crate)` for the viewer endpoint (`docs/viewer.md` §4): `window_rect`, `move_to`, `move_pointer`, `click_times`, `button_event`, `activate_if_needed`, `type_character`, `send_unit` (each takes `&ServerContext`), plus the pure `resolve_pointer_position`, `is_unmappable_key` and `output_fraction_position` (VAP normalized output fraction → window-relative position). The §5.5 handler bodies stay private.
- `capture.rs`: `pub(super) scale_from(source, &ImageBuffer)` (the reported `ImagePayload::scale` = output width / source width, `1.0` for an empty source; shared with `inspect.rs`), `source_size` (requested crop size, else window geometry), `observed_window` (the observation's own window, else `active_window_id`/`keyboard_focus`, else none).

## Routing Table

| Area | Owner |
|---|---|
| Router, `RequestContext`, `error_response`, outbound-sink seam | `./mod.rs` |
| Compositor-command + oneshot-reply seam (`send_infallible`/`send_result`) | `./command.rs` |
| §5.1 `ping` | `./runtime.rs` |
| §5.2 `list_apps`, `get_app`, `launch_app` | `./apps.rs` |
| §5.3 `list_windows`, `get_window`, `activate_window`, `close_window`, `get_focus` + canonical bridge helpers | `./windows.rs` |
| §5.4 `capture_window`, `capture_region`, `observe`, `wait_for_change`, `wait_for_quiet` | `./capture.rs` |
| §5.5 `pointer_move`, `click`, `double_click`, `mouse_down`, `mouse_up`, `scroll`, `drag`, `keypress`, `key_down`, `key_up`, `type_text` | `./input.rs` |
| §5.6 `subscribe_events`, `unsubscribe_events` + §5.10 `wait_for_events` | `./events.rs` |
| §5.7 `inspect_capture`, `inspect_subscribe` | `./inspect.rs` |
| §5.9 `post_notification`, `list_notifications`, `close_notification`, `invoke_notification_action` | `./notify.rs` |

34 methods, 34 handlers.

## Constraints

- Exactly one response per request; a handler `Err` never closes the connection.
- `adesk_proto::Method` is a total enum, so `unknown_method` can only arise at decode (`Method::from_parts` → `ProtoError::UnknownMethod`, mapped in `src/error.rs` and answered by the read loop in `src/connection.rs`).
- One `tracing` span per request (`request{id method}`) applied with `tracing::Instrument` — never `span.enter()` across an `.await`; never log pixel payloads.
- Input handlers (§5.5) call `ObserverService::record_action` BEFORE any compositor command and run inside `Session::input()` so they keep submission order per connection; keyboard methods activate a named, unfocused `window_id` first (protocol §5.5).
- `activate_window` / `close_window` are runtime-native `RuntimeCommand`s — never synthesized input.
- `activate_window` awaits the compositor's oneshot reply before building its response; the compositor resolves that reply only after activation is fully applied (WM active window, keyboard focus, data-device focus, `WindowActivated`/`FocusChanged` broadcast), so the awaited AGP response is a synchronization barrier (see `crates/adesk-server/CONTEXT.md`, compositor integration). A dropped reply is `internal`; an `Err` reply goes through `command_error`.
- Sequence numbers of server-synthesized events come from the compositor, never from local state: `launch_app` (§5.2 `AppLaunched`), `inspect.rs`'s `render_frame` (§5.7 `inspect_frame`) and every mutating §5.9 handler each call `windows::reserve_seq` (`RuntimeCommand::ReserveSeq`) — one global monotonic `seq` domain covering compositor- and server-emitted events (`docs/protocol.md` §1), gaps allowed, reuse not. A reserved number may go unused (spawn failure, a dropped or throttled inspect frame, a `close_notification` of an already-dismissed notification). No handler derives a `seq` from the `QueryState` watermark or from a server-private counter; the `inspect_frame` `ts_ms` stays the snapshot's compositor-clock value.
- §5.9 handlers mutate the `adesk_notify::NotificationService` store and publish the matching `RuntimeEvent` on the compositor broadcast (the one event stream), so §5.6 subscribers and the §5.10 inbox observe it. The `seq` is reserved **before** the store mutation so the stored notification and its event share one `seq`/`ts_ms`; `ts_ms` is `ServerContext::now_ms()`. `close_notification` publishes `notification_closed` **only** when `CloseOutcome::newly_dismissed` is true (a no-op close publishes nothing, leaving a legal `seq` gap); `invoke_notification_action` never dismisses; `list_notifications` is a pure read. A publish send failure means "no subscribers" and is logged at `trace`, never turned into an error.
- `wait_for_events` (§5.10) bridges the wire `kinds` filter to `adesk_notify::EventWaitSpec` through `crate::translate::proto_event_kind` (`surface_damage` → `surface_commit`; `quiet`/`inspect_frame` have no core emitter and are dropped) and reuses `crate::translate::event_record` (built on `EventPayload::from_runtime`/`to_data`) so a waited event is described exactly like a §5.6 fan-out frame. A timeout is a normal answer (`timed_out: true`, empty `events`), never an error.
- Observation methods await the observer first, render only afterwards and only when `include_image` is set; timeouts are observations with `timed_out: true`, never errors.
- A per-request quiet threshold reaches the observer only when the condition is quiet: `wait_for_quiet` is the only method reading `params.quiet_ms` (proto default 250) into `QuietSpec::quiet_ms`; `observe` carries `quiet_ms` only inside `until: {"type":"quiet","quiet_ms":N}` (required there, no wire default); `wait_for_change` has no quiet field at all.
- For a non-quiet condition (`change`/`timeout`) the `quiet` evidence flag is therefore computed against the observer's `DEFAULT_QUIET_MS` constant — 250 (`adesk_observer::DEFAULT_QUIET_MS`).
- The returned `Observation` is not post-processed: `translate::observe_result` only pairs it with the optional image (rendered afterwards from `observation.window_id`), so `timed_out`, `quiet` and the observer's condition semantics reach the wire verbatim.
- Errors map through `crate::error::ServerError` per `crates/adesk-server/CONTEXT.md`; do not invent AGP methods or fields.
- Files ≤ ~1000 lines; no new dependencies; no `unwrap`/`expect`/`panic!` outside `#[cfg(test)]`.

## Known Issues

- `type_text` classifies a character as unmappable by matching the compositor's `CompositorError::InvalidRequest` message for the substring `"keymap"`; a typed compositor error variant would be more robust (rewording the message surfaces `invalid_request` instead of a `skipped` entry — visible, not silent).
- `double_click`'s interval (100 ms, 50 ms gap) and `drag`'s post-move sleep are local server policy — protocol §5.5 specifies no interval.
- `inspect_subscribe` with `min_interval_ms == 0` re-renders continuously (yielding between iterations); it is client-controlled and protocol-legal but CPU-hungry.
- `mod.rs::Dispatcher::context()` has no caller in the workspace: only `Dispatcher::new`/`Dispatcher::dispatch` are used (`src/connection.rs`; the request-context handle it would return is `RunningServer::context()`, a different method). It is kept as public API but is currently dead code.
- The `adesk_core::Error` → `ServerError` mapping exists twice: the canonical `windows::command_error` (identifier-aware `unknown_window`, `ShuttingDown` → `Compositor(Stopped)`) and `src/inspection.rs::compositor_reply` (which collapses `UnknownWindow` to `Internal` and maps `ShuttingDown` to `ServerError::ShuttingDown`). They agree only on `InvalidRequest` and `RenderFailed`/`CaptureFailed`.
- The dropped-oneshot-reply classification is not uniform: `state`/`reserve_seq`/`send_unit` report `ShuttingDown`, while `activate_window`/`close_window`/`render_window` report `Internal("compositor dropped the ... reply")`.
## Test Strategy

- In-module unit tests cover the runtime-free parts: error-response shape, sink-registry round-trip, `command_error` mapping, `scale_from`/`source_size`, pointer-position resolution, `is_unmappable_key`, overlay/scale helpers.
- Behavioral coverage belongs to `crates/adesk-server/tests/` (a live runtime driven through `adesk-client` and `RawClient`); no test in this directory may start the runtime. The §5.9/§5.10 handlers are behavior-only (they need a live `ServerContext`/`NotificationService`); the notification fan-out is additionally pinned by `crate::subscriptions`'s in-module tests.
- Validate with `./scripts/dev.sh cargo check -p adesk-server --all-targets` and `./scripts/dev.sh cargo clippy -p adesk-server --all-targets --no-deps`.

## Notes for Agents

- `windows.rs` owns `state`, `reserve_seq`, `command_error` and `unknown_window`; do not reintroduce copies in other files.
- `inspect.rs`'s push loop stops when its subscription id disappears from `InspectRegistry::list()`, so `unsubscribe_events` must remove it there (it does); whichever way the loop ends (unsubscribe, closed sink, render failure) it then deregisters its own subscription id, so a stopped stream never stays in `InspectRegistry`.
- `input.rs` resolves window-relative and normalized coordinates through the window model's geometry from `QueryState`; never hard-code an origin.
- `capture.rs` and `inspect.rs` render on demand only; there is no per-event rendering anywhere in this directory.
- `launch_app` returns immediately after the spawn (plus a best-effort `NoteLaunch` ack); it never waits or polls for a window. The toplevel arrives later as a `WindowCreated` carrying `launch_id` (compositor ledger and/or `event_pump::correlate_window`); `WindowInfo.app_id` stays the client-set xdg app_id, so correlate by `launch_id`/`pid`, not by `app_id`.
- Observation methods do not validate `window_id` in the server — the observer answers `unknown_window` for an untracked id. `include_image` on a global observation renders the active window (or attaches no image when none is active). `max_dimension` is passed through to `RuntimeCommand::RenderWindow` unchanged; the compositor bounds the longest edge with per-axis half-up rounding, never upscales, and treats `0` as "disabled".
