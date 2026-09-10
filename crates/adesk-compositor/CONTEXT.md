# adesk-compositor — headless Smithay compositor core

## Intent

`adesk-compositor` is the ADesk runtime's core: a single dedicated thread running a `calloop` loop that owns one `Display<State>`, all v1 protocol globals, one virtual output, the seat (keyboard + pointer), `adesk_wm::WindowManager` and the headless renderer.
It is the **only** crate that may touch Smithay state; everything above it (server, observer, agent) talks to it exclusively through `RuntimeCommand` (in) and `RuntimeEvent` (out).
It has **no agent semantics**: no quiet/observe timers, no image encoding, no Unix socket server and no frame loop.
Rendering happens only when a `RenderWindow`/`RenderOutput` command asks for it; the crate reports what happened and the server decides what it means.
Window-management policy and coordinate conversion live in `adesk-wm`; pixel production (crop/downscale/readback/encoding) lives in `adesk-render`; this crate owns renderer *construction* and render-*element* collection only.

## API Surface

Entry point:
- `spawn(CompositorConfig) -> Result<CompositorHandle>` — starts the thread named `adesk-compositor` and returns immediately.
- `CompositorHandle` — `Clone + Send + Sync` (`Arc` inside):
  - `command() -> calloop::channel::Sender<RuntimeCommand>`
  - `send(RuntimeCommand) -> Result<()>`
  - `events() -> broadcast::Sender<RuntimeEvent>`
  - `subscribe() -> broadcast::Receiver<RuntimeEvent>`
  - `wait_ready() -> Result<ReadyInfo>` (async, cached after first success)
  - `wayland_display_name() -> Option<String>`
  - `output_size() -> Size`
  - `renderer() -> Option<RendererName>`
  - `shutdown() -> Result<()>` (async; served after queued commands)
  - `take_thread() -> Option<JoinHandle<()>>`
- `ReadyInfo { display_name: String, renderer: RendererName, output_size: Size }`.

Config:
- `CompositorConfig { output_size: Size (1280x800), renderer: RendererKind (Auto), xkb: XkbSettings (us), socket_name: Option<String> (auto), event_channel_capacity: usize (4096) }`; `new()`/`default()` plus `with_output_size`, `with_renderer`, `with_xkb`, `with_socket_name`, `with_event_channel_capacity`.
- `RendererKind { Auto, Gl, Pixman }` — `Auto` tries surfaceless EGL and falls back to pixman with a warning; `Gl` fails startup when EGL is unavailable.
- `RendererName { Gl, Pixman }` — `as_str()` → `"gl"`/`"pixman"` (AGP `ping`), `Display`.
- `XkbSettings { rules, model, layout, variant, options }` — defaults `evdev`/`pc105`/`us`/empty/`None`; `us()`, `to_xkb_config() -> smithay::input::keyboard::XkbConfig<'_>`.

Commands and replies:
- `RuntimeCommand` — `docs/architecture.md` §3 plus `NoteLaunch` (compositor-side launch-ledger bookkeeping for `launch_app`) and `ReserveSeq` (silent `seq` allocation for server-synthesized events): `RenderWindow`, `RenderOutput`, `QueryState`, `NoteLaunch`, `ReserveSeq`, `ActivateWindow`, `CloseWindow`, `PointerMove`, `PointerButton`, `PointerAxis`, `KeyEvent`, `Shutdown`.
- Every result-bearing variant carries its own `tokio::sync::oneshot::Sender<adesk_core::Result<T>>`; `QueryState` replies `StateSnapshot` infallibly; `NoteLaunch` acknowledges `()` infallibly; `ReserveSeq` replies the next `seq` (`u64`) infallibly; `Shutdown` acknowledges `()`.
- `RuntimeCommand::method() -> &'static str` is the stable tracing span name.
- `StateSnapshot { windows: Vec<WindowInfo>, active_window_id: Option<WindowId>, keyboard_focus: Option<WindowId>, seq: u64, ts_ms: u64 }` + `window(id)`, `len()`, `is_empty()`.
- `RenderedFrame { image: ImageBuffer, commit_seq: u64, damage: Vec<Rect> }` + `new()`, `size()`.

Input vocabulary:
- `Keysym` — resolved xkb keysym: `parse(name) -> adesk_core::Result<Keysym>`, `name()`, `value()`.
- `KeyCode { Key(Keysym), Chord(Vec<Keysym>) }` — `parse`, `parse_chord`, `keysyms()`, `is_chord()`, `display_name()`; chords are valid only with `KeyState::Pressed`.

Errors:
- `CompositorError` (thiserror, 14 variants: `ThreadSpawn`, `Display`, `Socket`, `EventLoop`, `Renderer`, `Keyboard`, `NotReady`, `StartupAborted`, `Stopped`, `UnknownWindow`, `InvalidRequest`, `WindowManagement`, `Render`, `Internal`), `code() -> adesk_core::ErrorCode`, `From<CompositorError> for adesk_core::Error`, `pub type Result<T>`.
- `InvalidRequest(String)` covers malformed-but-well-formed requests (unresolvable key name, released chord, input without a focused window) and maps to `ErrorCode::InvalidRequest`.

## Constraints

Threading:
- Exactly one compositor thread; `State` is created, used and dropped on it and is not `Send`; Smithay state and the renderer never leave it.
- Three channels only: commands (`calloop::channel`, FIFO, one command served per loop callback), events (`tokio::sync::broadcast<RuntimeEvent>`, capacity ≥ 4096, send never blocks), readiness (`oneshot`, cached in `wait_ready`).
- A command's reply is sent from inside the callback that produced it, after the state change, so "reply implies the event is visible" holds.
- `seq` comes from one central counter in `EventSink` — the single allocation point for compositor- and server-emitted events (`ReserveSeq` takes a number without emitting one; gaps are allowed, reuse is not); `ts_ms` is monotonic milliseconds since compositor construction (`Instant`), never wall clock.

Scope:
- v1 protocols in scope: `wl_compositor`, `wl_subcompositor`, `wl_shm`, `xdg-shell` (+ popups), `wl_seat` (keyboard + pointer), `wl_output`, `wl_data_device_manager` (basic clipboard), `zwp_linux_dmabuf`, `xdg-decoration`.
- Explicitly out of scope: XWayland, layer-shell, screencopy, fractional scale, multi-seat.
- Runtime-native operations (`ActivateWindow`, `CloseWindow`) mutate compositor state directly and are never synthesized input; only pointer/key/axis go through the seat.

Code rules:
- Errors: `thiserror` enums + `Result<T>`; no panics on request/event paths (the only production `expect`s are a mutex-poison guard and an impossible `ClientState` lookup).
- `unsafe` only for EGL construction in `src/render/headless.rs` (`EGLDisplay::new`, `EGLContext::make_current`, `GlesRenderer::new`).
- `tracing` only; never log pixel payloads or clipboard contents.
- ~1000 lines per file is the concern threshold; split along module boundaries.
- Dependencies come from the root `[workspace.dependencies]`; never inline versions.
- Public API is exactly what this file documents; everything else is `pub(crate)`.

## Routing Table

| Area | Owner |
|---|---|
| Public command vocabulary (§3) | `src/command.rs` |
| Config, renderer selection, xkb settings | `src/config.rs` |
| Error enum + `ErrorCode` mapping | `src/error.rs` |
| `EventSink`: seq/ts allocation + typed event emitters | `src/events.rs` |
| `spawn`, `CompositorHandle`, `ReadyInfo` | `src/handle.rs` |
| `StateSnapshot`, `RenderedFrame` | `src/snapshot.rs` |
| `State`: globals, seat, output, renderer, side-effect API | `src/state.rs` |
| Thread entry: display + calloop loop + three sources | `src/run.rs` |
| `handle_command`: one command → outcome/reply (module `run::dispatch`) | `src/dispatch.rs` |
| Wayland socket bind/name | `src/socket.rs` |
| `WmBridge` + coordinate resolution | `src/wm.rs` |
| `WmBridge` unit tests (included from `wm.rs` via `#[path]`) | `src/wm_tests.rs` |
| Protocol handler impls + delegate macros | `src/protocols/` |
| Input injection internals (keycode, keymap, injector) | `src/input/` |
| Headless renderer + element collection | `src/render/` |
| `compositor_smoke.rs` (public API only) + `window_lifecycle.rs`/`input_delivery.rs`/`popups.rs`/`clipboard.rs`/`reserve_seq.rs`/`output_composition.rs` integration suites + `integration_plan.md` scenario/suite map | `tests/` |

`src/protocols/`: `compositor.rs` (CompositorHandler + `ClientState`/`ClientData`), `xdg_shell.rs` (XdgShellHandler), `seat.rs` (SeatHandler), `output.rs` (OutputHandler), `shm.rs` (ShmHandler + BufferHandler), `dmabuf.rs` (DmabufHandler), `data_device.rs` (DataDeviceHandler + SelectionHandler + DnD), `decoration.rs` (XdgDecorationHandler).
`src/input/`: `keycode.rs` (public `KeyCode`/`Keysym` parsing + alias table), `keymap.rs` (`KeymapTable`: keysym → keycode + level), `injector.rs` (`InputInjector` associated functions + keyboard/pointer handle getters).
`src/render/`: `headless.rs` (`HeadlessRenderer`: create/name/dmabuf_formats/render_window/render_output), `elements.rs` (`window_elements`, `window_scene`, `output_scene`, `popup_surfaces`, `overlay_elements`, `rect_to_smithay`), `mod.rs` (`OutputWindow`).

Sibling cross-references (read-only from this node; escalate writes to the parent):
- `../adesk-core/` — domain types and `RuntimeEvent` (implemented).
- `../adesk-wm/` — window model, tiling policy, focus, coordinate authority (implemented; consumed by `WmBridge`).
- `../adesk-render/` — crop/downscale/readback/encoding and the `Scene` pipeline used by both render paths (implemented).
- `../adesk-testkit/` — implemented integration harness the suites consume: in-process runtime, temp `XDG_RUNTIME_DIR`, Wayland test client (input and clipboard recorders), image assertions, event tap.

## Design Decisions

### Smithay 0.7 facts this crate depends on (verified against the vendored source)

Handlers and wiring:
- `CompositorHandler` requires `compositor_state`, `client_compositor_state(&self, &Client)`, `commit(&mut self, &WlSurface)`; `on_commit_buffer_handler::<State>` lives in `smithay::backend::renderer::utils`, **not** `wayland::compositor`.
- `delegate_compositor!` does **not** implement `BufferHandler`; `BufferHandler::buffer_destroyed` is implemented in `src/protocols/shm.rs` and sweeps the renderer texture cache via `Renderer::cleanup_texture_cache()` (0.7 has no per-buffer destruction helper).
- `ClientState { compositor_state: CompositorClientState }` (`Default`) + `impl ClientData`; clients are inserted with `display.handle().insert_client(stream, Arc::new(ClientState::default())) -> io::Result<Client>`.
- All eight `delegate_*!(State)` macros take only the state type and generate `Dispatch`/`GlobalDispatch` only — they never create globals; globals are created explicitly in `State::new`, so no `OutputManagerState` field is needed.
- `XdgShellHandler`: required `xdg_shell_state`, `new_toplevel`, `new_popup(PopupSurface, PositionerState)`, `grab(PopupSurface, WlSeat, Serial)`, `reposition_request(PopupSurface, PositionerState, u32)`; defaulted `toplevel_destroyed`, `popup_destroyed`, `title_changed`, `app_id_changed`, `ack_configure`. There is **no `request_close`** — close is `ToplevelSurface::send_close()`.
- There is no ADesk-side `get_popup` handler: Smithay's `xdg_surface.get_popup` dispatch seeds the popup's `server_pending.geometry` from `positioner.get_geometry()` and then calls `XdgShellHandler::new_popup`; `PopupSurface::send_configure` emits **both** `xdg_popup.configure(x, y, w, h)` and `xdg_surface.configure(serial)` (serial from `SERIAL_COUNTER.next_serial()`) and sets `initial_configure_sent`; `ack_configure` stays Smithay's default (not overridden here).
- `XdgDecorationHandler`: all three methods required (`new_decoration`, `request_mode(Mode)`, `unset_mode`); the crate answers `Mode::ServerSide`. `XdgDecorationState { global: GlobalId }` is kept alive in `State`.
- `SeatHandler`: focus types are `WlSurface` (`WaylandFocus`); `SeatState::new()` takes no arguments.
- `SeatState::new_wl_seat(display, name)`: the method's generic `N` is the seat **name** (`N: Into<String>`) while the state type comes from `SeatState<D>` — do not turbofish the method.
- `DataDeviceHandler::data_device_state`; `SelectionHandler::SelectionUserData = ()`; `ClientDndGrabHandler`/`ServerDndGrabHandler` live in `smithay::wayland::selection::data_device` (not `selection`); `DataDeviceState::new::<D>(&dh)`. Clipboard payloads stay client-to-client (never stored or logged).
- `ShmHandler::shm_state`; `ShmState::new::<D>(&dh, formats)` (Argb8888/Xrgb8888 are auto-inserted).
- `OutputHandler` is fully defaulted; the output global is created manually with `output.create_global::<D>(&dh) -> GlobalId`.
- `Output::new(name, PhysicalProperties { size: Size<i32, Raw> /* millimetres */, subpixel, make, model })`; `change_current_state(Option<Mode>, Option<Transform>, Option<Scale>, Option<Point<i32, Logical>>)`; `Mode { size: Size<i32, Physical> /* pixels */, refresh: i32 /* mHz */ }`; `set_preferred`, `add_mode`. `create_global` stores `WlOutputData { output: self.clone() }`, so `State::output` is the compositor's own Arc handle.
- `DmabufHandler`: `dmabuf_state() -> &mut DmabufState`, `dmabuf_imported(&mut self, &DmabufGlobal, Dmabuf, ImportNotifier)` (0.7 has no `Node`/`BufferInfo` arguments); `DmabufState::new()`; `create_global::<D>(&dh, formats) -> DmabufGlobal`; `ImportNotifier::successful::<D>()` takes **no texture argument**.
- `Format` is `smithay::backend::allocator::Format` (re-export of `drm_fourcc::DrmFormat`); `FormatSet` is at `smithay::backend::allocator::format::FormatSet` and iterates `Format`s.
- Smithay **does** re-export pixman's `Image` as `smithay::reexports::pixman::Image` — no extra workspace dependency is needed for the pixman offscreen path.

Seat and input:
- `Seat::add_keyboard(XkbConfig<'_>, repeat_delay, repeat_rate) -> Result<KeyboardHandle<D>, KeyboardError>`; `Seat::add_pointer() -> PointerHandle<D>`.
- `XkbConfig { rules, model, layout, variant: &str, options: Option<String> }` at `smithay::input::keyboard::XkbConfig`.
- `KeyboardHandle::input(&self, &mut D, Keycode, KeyState, Serial, u32, F) -> Option<T>` with `FilterResult::{Forward, Intercept}`; `set_focus(&self, data, Option<WlSurface>, serial)`.
- `PointerHandle::motion(&self, data, Option<(PointerFocus, Point<f64, Logical>)>, &MotionEvent { location, serial, time })`; `button(.., &ButtonEvent { serial, time, button: u32, state: ButtonState })`; `axis(.., AxisFrame::new(time).source(..).value(Axis::Vertical, f64))` then `frame()`.
- Button codes: BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112, BTN_SIDE=0x113, BTN_EXTRA=0x114; `Serial::from(0u32)` works; `SERIAL_COUNTER.next_serial()`.
- xkb re-exports at `smithay::input::keyboard::xkb`; `keysym_from_name(name, KEYSYM_CASE_INSENSITIVE)`; `Keysym::raw()`; `Keymap::new_from_names::<str>(&ctx, rules, model, layout, variant, options, flags) -> Option<Keymap>`; `min_keycode`/`max_keycode`/`num_layouts`/`num_levels_for_key`/`key_get_syms_by_level(keycode, layout, level) -> &[Keysym]`.
- Smithay/xkbcommon has **no reverse keysym→keycode API**, so `KeymapTable` indexes the keymap once at startup; single-character inputs (e.g. `"+"`) need a `utf32_to_keysym` fallback because `keysym_from_name("+")` returns NoSymbol.
- Keyboard setup needs `XKB_CONFIG_ROOT` (set by the dev shell); running binaries outside the shell fails keymap compilation.
- `InputInjector` is the single source of truth for seat injection: associated functions plus `keyboard()`/`pointer()` handle getters, so `State` never duplicates evdev tables.

Rendering:
- Headless GL: `EGLSurfacelessDisplay` (`smithay::backend::egl::native`), `unsafe EGLDisplay::new(native)`, `EGLContext::new(&display)`, `unsafe EGLContext::make_current()`, `unsafe GlesRenderer::new(context)`; pixman: `PixmanRenderer::new()`.
- `GlesRenderer` is `!Send` (fits the single-thread model); `PixmanRenderer` also implements `ImportDma`; GL readback is y-flipped (`GlesMapping::flipped() == true`) and GL readback rows arrive top-down, so the image conversion must not double-flip.
- There is **no unified offscreen abstraction** (GL uses `Offscreen<GlesTexture>`, pixman `Offscreen<Image>`), which is why `HeadlessRenderer` is an enum with per-backend render paths; the GL variant is boxed (`Gl(Box<GlesRenderer>)`) to keep the enum small.
- `render_elements_from_surface_tree` walks the whole surface tree (subsurfaces yes, **popups no**); popups come from the static `PopupManager::popups_for_surface(&WlSurface)` yielding `(PopupKind, Point)` and are collected separately.
- `adesk-render`'s `Scene` is bottom-to-top; `OutputDamageTracker::render_output` wants front-to-back — the render crate handles the ordering, so this crate only builds `Scene` nodes.
- The window/output paths render through `adesk_render::{create_target, render_scene}`; this crate never uses Smithay's `OutputDamageTracker`.
- `HeadlessRenderer::render_output(output_size, windows, overlays, region, max_dimension)` — an empty window list is a valid clear frame (this is why `output_size` is passed explicitly).

Event loop:
- `Display::new()`, `backend().poll_fd()`, `dispatch_clients(&mut self, &mut State) -> io::Result<usize>`, `flush_clients()`.
- `ListeningSocketSource::{new_auto, with_name, socket_name() -> &OsStr}`; `Event = UnixStream`, `Metadata = ()`, `Ret = ()`, `Error = io::Error`; `ListeningSocket` requires a writable `XDG_RUNTIME_DIR`.
- calloop 0.14.4: `EventLoop::run(timeout, data, cb)` is 3-arg; `Generic` callbacks return `Result<PostAction, io::Error>`; `calloop::channel::Event::{Msg, Closed}`; `Channel<T>` has `Ret = ()`; `Sender<T>: Send + Sync` (this is what makes `CompositorHandle: Send + Sync`); `LoopSignal::stop()`.
- Startup order is part of the contract: display → bind socket → create globals → register the three sources → publish readiness. Binding before globals means the reported name is already connectable.

### Crate-local decisions

- `RenderedFrame` = `ImageBuffer` + `commit_seq` + `damage`: the frame travels with the causal history it belongs to.
- `WindowId` allocation is entirely `adesk-wm`'s: `WindowModel::next_id` (a per-`WindowManager` `u64` field, no statics/atomics) starts at `1` and increments only in `policy::on_map`, so a fresh compositor assigns `WindowId(1)`, then `WindowId(2)`, ... to the first two *mapped* toplevels; registration and duplicate maps allocate nothing. Popups use a separate `SurfaceRegistry::next_popup_id` counter and surfaces a separate `SurfaceKey` counter, so only a toplevel map can consume a window id.
- Title/app-id updates are metadata-only: `policy::on_title`/`policy::on_app_id` return no actions and neither path marks damage or re-configures — damage comes exclusively from surface commits.
- A late `xdg_toplevel.set_app_id` is written back into the window model: `WmBridge::app_id_changed` reads the toplevel metadata and delegates to `note_app_id`, which keeps the `app_ids` change detection and the launch-ledger refinement and writes a genuine change through `WindowManager::on_app_id`, so `WindowInfo.app_id`/`QueryState`/`list_windows` report the new value. Smithay applies `set_app_id` while dispatching the request (not at the next commit), so no commit is needed for the write-back to land.
- Renderer split: the compositor constructs the renderer and collects elements; `adesk-render` owns crop/downscale/readback/encoding.
- Output composition is the single-visible-toplevel projection: `State::render_output` collects every tracked window with a root surface into a *candidate* list of `OutputWindow`s (`active` marks the one `adesk-wm` tiles), and `elements::output_scene` draws exactly the first `active` candidate (`visible_index`) — tracked-but-inactive windows are never composed and two toplevels cannot stack. The composed window's popups ride along through `window_elements`, overlays are computed from the composed window alone, and no active candidate yields an empty scene (a clear frame). Window-level `render_window` still renders any window by id.
- `WmBridge` (`src/wm.rs`) is the only place Smithay surfaces meet the window model. Its surface: `new(output_size)`, `active_window`, `keyboard_focus`, `window_for_surface`, `windows`, `tiled_rect`, `toplevel_of`, `surface_of`, `last_commit_seq`, `note_launch`, `resolve_position`, `register_toplevel`, `unmapped_toplevel`, `map_toplevel -> MapOutcome`, `destroy_toplevel`, `title_changed`, `app_id_changed`, `note_app_id`, `popup_added`, `popup_removed`, `popup_window_offset`, `note_popup_grab`, `popup_grab`, `take_popup_grab`, `commit`, `activate`. `WmDecision { actions, previous_focus }` captures focus *before* the policy ran.
- `State`'s side-effect API is the only mutation path: `on_toplevel_mapped`, `on_toplevel_registered`, `on_toplevel_destroyed`, `on_title_changed`, `on_app_id_changed`, `on_popup_created`, `on_popup_destroyed`, `on_surface_commit`, `note_launch`, `reserve_seq`, `activate_window`, `close_window`, `inject_key`, `inject_pointer_move`, `inject_pointer_button`, `inject_pointer_axis`, `render_window`, `render_output`, `snapshot`. Protocol handlers never touch `adesk-wm`, the renderer or input internals directly.
- `WmBridge` owns the surface registry: toplevels, subsurfaces and popups resolve to a `WindowId`; commit counters and damage are per-window and window-relative.
- Popups are tracked manually (`PopupAppeared`/`PopupDisappeared` with owner `window_id` + `popup_id`) because Smithay's element walker skips them.
- `new_popup` confirms the positioner geometry (`PositionerState::get_geometry`) with the initial `PopupSurface::send_configure` *before* registering the popup, so the recorded window-relative origin equals the configured placement; a positioner without a size confirms `(0, 0)` at `0x0` and the popup picks its own size. v1 applies no popup constraint adjustment.
- PopupManager wiring: `State::popup_manager` (Smithay `desktop::PopupManager`) is initialized in `State::new`; `new_popup` registers each popup via `track_popup(PopupKind::Xdg)` (a rejection is logged at debug, never fatal) and the compositor `commit` handler forwards to `popup_manager.commit` before the ADesk-side `on_surface_commit`; this is what makes `PopupManager::popups_for_surface` find popups for element collection, so popup pixels compose into the owner's `RenderWindow`/`RenderOutput`.
- Data-device focus invariant: `apply_activate` in `src/state.rs` is the single keyboard-focus choke point; it calls Smithay's `set_data_device_focus(&display, &seat, surface.client())` on every focus change so the data-device focus always equals keyboard focus (v1: one seat, focus == active window; the destroyed-target fallback passes `None` and clears it). Without this call Smithay's `send_selection` skips every `wl_data_device` client and no `data_offer`/`selection` is ever delivered.
- Selection acceptance is Smithay's, not this crate's (`src/protocols/data_device.rs` adds no checks): `wl_data_device.set_selection` is accepted only when `KeyboardHandle::client_of_object_has_focus` reports the caller's client as the keyboard-focus client (client-level, so any `wl_data_device` object of the focused client passes; `KeyboardHandle::focus` is the focused surface). The request `serial` is ignored entirely (Smithay matches `SetSelection { source, .. }`), and a denial is a silent drop with Smithay's own `debug!` "denying setting selection by a non-focused client" — no protocol error, no `cancelled` on the source. `wl_data_source.cancelled` is emitted only when an accepted selection supersedes a live source (`SeatData::set_clipboard_selection`).
- Launch correlation: `WmBridge::note_launch` records a launch for compositor-local correlation; `RuntimeCommand::NoteLaunch` (sent by the server's `launch_app` right after a successful spawn) feeds it, so the compositor's own `WindowCreated` broadcast carries `launch_id`. The server-side `adesk_app_registry::Correlator` additionally stamps the events the server projects.
- `src/dispatch.rs` is declared from `src/run.rs` with `#[path = "dispatch.rs"] pub(crate) mod dispatch;` (module path `crate::run::dispatch`).
- `src/wm_tests.rs` holds the `wm` unit tests, included from `src/wm.rs` via `#[cfg(test)] #[path = "wm_tests.rs"] mod tests;` to keep `wm.rs` under the size threshold.
- `wl_output` physical size is reported in **millimetres** (96 DPI-derived, minimum 1mm) because `PhysicalProperties.size` is mm; the pixel size is the `Mode`.
- `EventSink` emits the eight compositor-owned `RuntimeEvent` variants; `AppLaunched` is emitted by the server/app-registry side, never here.
- Two `#[allow(dead_code)]` sites remain, both field-level lifetime handles: `State::output` and `State::xdg_decoration_state`. No crate-level allow attributes remain.

### AGP command semantics (verified against the code)

- `RenderWindow` resolves `window_id` through `WmBridge::windows()` and then `surface_of`; either miss is `CompositorError::UnknownWindow` (`unknown_window`).
- The render source is `Rect::from_size(geometry.size())`, so `region` is window-relative and must be non-empty and **strictly contained** in the window rect; an out-of-bounds crop is `invalid_request`, never clipped.
- Crop is applied first, then `max_dimension` downscales the cropped image.
- `max_dimension` bounds the **longest edge**; each axis is `clamp(round_half_up(value * M / longest), 1, value)` with one shared scale, so aspect ratio is approximately preserved; `Some(0)` disables scaling and nothing ever upscales.
- `RenderedFrame.commit_seq` is the window's `last_commit_seq` for `RenderWindow` and `0` for `RenderOutput`; `damage` stays in full window coordinates even when the image is cropped/downscaled.
- Pipeline images are tightly packed `Rgba8` (row-major, top-down, straight alpha, stride == width*4); `ImageBuffer::stride` is a public field that may be padded in general, so read pixels via `pixel(x, y)`.
- `StateSnapshot.windows` is creation order, exactly one record is `Active`, every record has `mapped == true`, and popups appear only as `popup_count`, never as entries.
- `StateSnapshot.keyboard_focus` always equals `active_window_id` in v1 because `WmBridge::keyboard_focus()` returns `manager.active_window()`.
- Snapshot `seq` is the event watermark and snapshot `ts_ms` is monotonic ms from `State::start`; event `ts_ms` uses `EventSink::start` (a distinct but equally monotonic origin).
- `ReserveSeq` allocates the next value from `EventSink`'s single counter and emits nothing (`State::reserve_seq`); the server uses it for events it synthesizes itself (`AppLaunched`), so compositor- and server-emitted events share one global monotonic `seq` domain (`docs/protocol.md` §1) where gaps are allowed and reuse is not.
- Input commands carry no window id: `PointerMove` is a window-relative `Position` resolved and clamped against the focused-or-active window, while `PointerButton`/`PointerAxis` act at the current pointer location; with no window the reply is `invalid_request`.
- `WmBridge::resolve_position` reports an unknown id as `WindowManagement` (→ `internal`), but the command paths resolve the surface first and answer `unknown_window`; normalized `1.0` resolves to the last pixel (`w-1`), never outside the window.

## Test Strategy

Unit tests (colocated `#[cfg(test)]`; 87 tests pass):
- `config`: defaults match the contract, builder overrides, xkb config borrowing, mm conversion (1280x800 → 339x212mm, ≥1mm floor).
- `events`: `seq` globally monotonic across variants, `ts_ms` never decreasing, payload fields preserved, reserved seqs increase without emitting, emitting without subscribers is not an error.
- `handle`: `CompositorHandle: Clone + Send + Sync`, wire renderer names.
- `error`: `ErrorCode` mapping per variant (including `InvalidRequest`).
- `input::keycode`: named keys, aliases (case-insensitive), F1–F24, printable chars, chord order/display, chord release rejection, unknown/empty → `invalid_request`.
- `input::keymap`: letters unshifted, shifted chars at level 1, unknown keysyms, uncompilable settings → keyboard error (needs `XKB_CONFIG_ROOT`).
- `input::injector`: logical buttons → evdev codes.
- `render::elements`: scene nodes keep bottom-to-top order and their own rects, damage coalescing, overlay markers/colors; output-composition selection (`visible_index` picks only the active candidate, and picks none when all candidates are inactive or the list is empty), an empty scene without a visible window, and overlays marking only the composed window. The selection is proven at the selection/scene level, and pixel proof covers both render paths: `RenderWindow` (`window_lifecycle.rs` matches the committed pattern, `popups.rs` asserts the popup's own fill inside the owner's frame) and `RenderOutput` (`output_composition.rs` proves a tracked-but-inactive window is excluded from the composed frame).
- `render::headless`: pixman/GL clear frames, GL path gated by `ADESK_TEST_GL=1`.
- `protocols::xdg_shell`: initial popup configure geometry from the positioner, unconstrained `0x0` fallback without a positioner size.
- `run::dispatch`: method names exact and unique, shutdown outcome, outcome distinguishability.
- `socket`: bind honours the configured name, structured errors (environment-aware when `XDG_RUNTIME_DIR` is not writable).
- `snapshot`: lookup/helpers, frame size.
- `state`: released chord and unresolvable keysym → `invalid_request`.
- `wm` (in `wm_tests.rs`): surface lookup, stable ids, duplicate map idempotence, popup id/offset/grab bookkeeping, launch ledger bounds, title evidence, late app-id write-back, unknown windows.

Smoke tests (`tests/compositor_smoke.rs`; 3 tests, public API only, no `adesk-testkit`):
- spawn/readiness/display name/renderer/output size; `QueryState` on a fresh runtime; shutdown + thread join + post-shutdown send failure.
- They bind a real Wayland socket, so each test installs a private `0700` temp `XDG_RUNTIME_DIR` (restored on drop) and holds a process-wide mutex for its whole body, because the env var is process-global and the tests share one binary.
- This is required in this sandbox: `/run/user/1000` is a **read-only filesystem**, so the ambient `XDG_RUNTIME_DIR` cannot host a socket.

Integration tests (implemented; driven through the `adesk-testkit` dev-dependency on a real in-process compositor, temp `XDG_RUNTIME_DIR`, `RendererKind::Pixman`, event-tap ordering with explicit deadlines and no sleeps): 20 tests in six files, and `tests/integration_plan.md` maps each scenario to its suite — the plan holds the §1–§7 specs and names the file per scenario group, while each suite's module doc names the scenario it implements.
- `tests/window_lifecycle.rs` (3): §1 a window appears with a tiling configure (event order, `QueryState`, `RenderWindow` pixels matching the committed pattern); §1 addendum a late `set_app_id` (set after the first commit) is written back into the window model and reported by `QueryState` (barrier: a commit on the same connection whose `surface_commit` event is awaited); §2 focus follows activation (activation/focus event order with the reply after both events, no re-configure on activation, unknown `WindowId` → `unknown_window`).
- `tests/input_delivery.rs` (6): §3 the real seat path — a normalized `PointerMove` lands on the window model's point, press/release are two ordered `wl_pointer.button` events, `wl_pointer.axis` is negative-vertical and framed, `ctrl+c` is delivered as an ordered chord press with a reverse release, and a released chord or an injection with no focused window is `invalid_request` without panicking.
- `tests/popups.rs` (2): §4 popup lifecycle — `popup_appeared`/`popup_disappeared` name the owner, the popup's pixels compose into the owner's `RenderWindow` under the window's one commit counter, and destroying the owner with a popup open reports the popup's disappearance first.
- `tests/clipboard.rs` (5): §5 two real connections exchanging `wl_data_device` selections — exact bytes read back, a second `set_selection` supersedes the first offer, an unrequested mime is not delivered, activation moves the selection target, and no runtime event carries clipboard payload. Offers are counted as client-observed `wl_data_device.selection` events carrying an offer (`selection(None)` is not counted), and every publication is preceded by the publisher being the keyboard-focus client (map order in `two_peers`, or an awaited `ActivateWindow` plus a real-input serial).
- `tests/reserve_seq.rs` (2): §6 `ReserveSeq` — reservations strictly increase, are silent (no event within a bounded window), move the watermark `QueryState` reports, and a toplevel mapped afterwards emits events strictly above them and immediately following them (no reuse; one seq domain).
- `tests/output_composition.rs` (2): §7 single-visible-toplevel pixels — two clients map toplevels with distinct opaque fills; activating one makes the composed `RenderOutput` match the active fill on every pixel and differ from the inactive window's own frame (which is rendered first as non-vacuity while `QueryState` proves exactly one `Active`; roles then reversed), and a runtime with no active window composes to the default clear frame.
- All six suites are pixman-only; the crate's GL-only path stays behind `ADESK_TEST_GL=1` (the lib `render::headless` clear-frame test).

Validation recipe (all workspace members have manifests, so the crate builds in-tree):
- `./scripts/dev.sh cargo check -p adesk-compositor --all-targets` (warning-free)
- `./scripts/dev.sh cargo clippy -p adesk-compositor --all-targets` (warning-free)
- `./scripts/dev.sh cargo test -p adesk-compositor` (87 lib + 20 integration + 3 smoke + 1 doc-test pass, 1 ignored doc-fence)
- `./scripts/dev.sh cargo doc -p adesk-compositor --no-deps` (warning-free)
- `ADESK_TEST_GL=1 ./scripts/dev.sh cargo test -p adesk-compositor --lib` (runs the GL clear-frame test on llvmpipe)
- `./scripts/dev.sh cargo check --workspace --all-targets` (confirms the public API still satisfies server/testkit)

## Known Issues

- Popup grabs are recorded, not enforced (v1 semantics); an activation that invalidates a grab dismisses it with `popup_done`.
- Clipboard tests establish the publisher's keyboard focus only via the AGP `ActivateWindow` reply (sent after `apply_activate`) and client-observed events; `WaylandTestClient::roundtrip()` is a flush plus one reader poll cycle, not a `wl_display.sync` barrier, so it does not prove the compositor dispatched a preceding `set_selection`. The supersede test's second publication is ordered against the following `ActivateWindow` only by "requests flushed to the socket before the command was queued" — a latent race whose failure mode is a loud offer-count/payload assertion, never a silent pass — and the `publish` doc comment in `tests/clipboard.rs` claiming a "sync callback" is inaccurate.
- `cargo fmt -p adesk-compositor -- --check` reports repo-wide rustfmt-version drift (import ordering, `assert_eq!` wrapping) — tooling drift, not code defects. Do not reformat unrelated files to chase it.
- The sandbox has no GPU and no system EGL on the default library path; only the dev shell provides them (llvmpipe). `XKB_CONFIG_ROOT` likewise comes from the dev shell.

## Dependencies

- Internal: `adesk-core` (domain types, `RuntimeEvent`), `adesk-wm` (window model + policy), `adesk-render` (`Scene`/`create_target`/`render_scene`/readback/image conversion`); dev-dependency `adesk-testkit` (in-process runtime, Wayland test client, image assertions — used only by the crate-local integration suites).
- External (all via root `[workspace.dependencies]`): `smithay 0.7` with `wayland_frontend`, `desktop`, `renderer_pixman`, `renderer_glow`; `wayland-server 0.31`; `calloop 0.14`; `tokio 1` (sync/rt/time/net); `thiserror 2`; `tracing 0.1`; `libc 0.2`.
- System (Nix dev shell only): libxkbcommon + xkeyboard-config (`XKB_CONFIG_ROOT`), pixman, libEGL/GLES (llvmpipe), libwayland, libdrm/gbm, libudev.
- Builds must go through `./scripts/dev.sh`; bare `cargo` cannot link outside the shell.

## Notes for Agents

Hazards:
- `PopupConfigureError` must be handled, never unwrapped, in the xdg-shell path.
- GL readback is y-flipped and GL rows arrive top-down; do not double-flip.
- `GlesRenderer` is `!Send`; there is no shared offscreen abstraction.
- Smithay's `XdgShellHandler` has no `request_close`; use `ToplevelSurface::send_close()`.
- EGL and `XKB_CONFIG_ROOT` exist only in the dev shell; keymap-compiling tests need it.
- A Wayland socket needs a writable `XDG_RUNTIME_DIR`; the ambient one is read-only here, so tests must install a temp dir (see Test Strategy).
- `WaylandTestClient::roundtrip()` is not a `wl_display.sync` barrier (it flushes and waits one reader poll cycle); to order an assertion after a client request, commit on the same connection and await the resulting `surface_commit` event (see the late-app-id test in `window_lifecycle.rs`).
- Never log pixel payloads or clipboard bytes.
- `PointerButton`/`PointerAxis` reply `Ok(())` whenever any window is active but are silently dropped unless a prior `PointerMove` established pointer focus (Smithay's default grab sends only to the focused surface; initial pointer focus is `None`), so e2e tests must move before clicking or scrolling.
- The compositor must never grow quiet/timer semantics: observation belongs to `adesk-observer`, which the server feeds.
