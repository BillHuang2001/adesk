# ADesk — system architecture and internal contracts

This document defines the internal contracts that cross crate boundaries.
`docs/protocol.md` defines the external (agent-facing) contract; this file defines
how the crates cooperate to serve it. Child crates must treat both as binding.

## 1. Process, threads and ownership

```text
┌─ thread: compositor (calloop, single-threaded) ───────────────────┐
│ adesk-compositor::Compositor                                      │
│   owns Display<State>, all Smithay protocol state, the seat,      │
│   the virtual Output, adesk-wm::WindowManager, the renderer.      │
│   NOT Send. Never blocks. Never touches sockets or the filesystem  │
│   except the Wayland listening socket.                            │
└────────▲────────────────────────────────────┬─────────────────────┘
         │ RuntimeCommand (calloop channel)   │ RuntimeEvent (tokio broadcast)
┌────────┴────────────────────────────────────▼─────────────────────┐
│ tokio multi-thread runtime: adesk-server                          │
│   Unix socket listener, per-connection tasks, request dispatch,   │
│   action registry, adesk-observer::ObserverService,               │
│   adesk-app-registry::AppRegistry, image encoding, inspector      │
└───────────────────────────────────────────────────────────────────┘
```

Rules:

- All Smithay state lives on the compositor thread. Nothing else may touch it.
  Renderer objects are also compositor-thread-only.
- Cross-thread communication happens through exactly three channels:
  1. **Commands** server → compositor: `calloop::channel::Sender<RuntimeCommand>`
     (created by `adesk-compositor`). Every command that produces a result carries a
     `tokio::sync::oneshot::Sender<Result<T, Error>>` inside it.
  2. **Events** compositor → server: `tokio::sync::broadcast::Sender<RuntimeEvent>`
     (capacity ≥ 4096). The compositor sends without blocking; a full channel never
     stalls the event loop.
  3. **Lifecycle**: the compositor thread reports startup readiness (socket name,
     renderer chosen) through a `tokio::sync::oneshot` held by `CompositorHandle`.
- `adesk-compositor` exposes `CompositorHandle` (Clone + Send + Sync) as its public
  API: `command()`, `events()`, `wait_ready()`, `shutdown()`.
- Long waits (`observe`, `wait_for_quiet`, ...) are implemented **only** in the server
  using the event stream and the observer. The compositor has no timers for agent
  semantics.
- Lag handling: if a subscriber sees `broadcast::error::RecvError::Lagged`, it must
  resync via a `QueryState` command instead of silently continuing; the observer marks
  the affected windows as "state uncertain" and still answers waits correctly.

## 2. Events (compositor → server)

`adesk_core::RuntimeEvent` is the single event vocabulary. Every event carries:

- `seq: u64` — global monotonic sequence. There is exactly **one** `seq` domain: it
  covers the events the compositor emits *and* the events the server synthesizes itself
  (`AppLaunched`, `inspect_frame`), because both take their number from the
  compositor's single counter (§3 `ReserveSeq`). Gaps are allowed (a reserved number
  may go unused), reuse is not.
- `ts_ms: u64` — monotonic ms since compositor start.
- `window_id: Option<WindowId>` — the window the event belongs to, when applicable.

Emitted on the runtime's event broadcast (the compositor emits every one of them except
`AppLaunched`, which the server synthesizes from the `launch_app` it just served):

| Event | Emitted when |
|---|---|
| `WindowCreated` | a new xdg-toplevel is mapped (includes `app_id`, `pid?`, `launch_id?`) |
| `WindowDestroyed` | toplevel unmapped/destroyed |
| `WindowActivated` | focus/active-window policy changed (`activate_window` or auto-focus on map) |
| `TitleChanged` | xdg-toplevel title changed |
| `SurfaceCommit` | any commit on the window's surface tree (toplevel, subsurface, popup), carrying `commit_seq` and damage `Region` |
| `FocusChanged` | keyboard focus moved (window or null) |
| `PopupAppeared` / `PopupDisappeared` | xdg-popup mapped/unmapped |
| `AppLaunched` | registry spawned a process (`launch_id`, `app_id`, `pid`) |

Ordering guarantee: events for one window are delivered in `seq` order; the broadcast
channel preserves global order for non-lagged receivers.

`SurfaceCommit` is the high-frequency event (animations). Consumers must treat it as
cheap: increment counters, union damage, update timestamps — no rendering, no logging
above `trace`.

## 3. Commands (server → compositor)

```rust
enum RuntimeCommand {
    RenderWindow { window_id: WindowId, region: Option<Rect>, max_dimension: Option<u32>,
                   reply: oneshot::Sender<Result<RenderedFrame, Error>> },
    RenderOutput { overlays: Vec<OverlayKind>, region: Option<Rect>, max_dimension: Option<u32>,
                   reply: oneshot::Sender<Result<RenderedFrame, Error>> },
    QueryState { reply: oneshot::Sender<StateSnapshot> },   // windows, focus, seq watermarks
    NoteLaunch { launch_id: LaunchId, app_id: AppId, pid: Option<i32>,
                 reply: oneshot::Sender<()> },
    ReserveSeq { reply: oneshot::Sender<u64> },
    ActivateWindow { window_id: WindowId, reply: oneshot::Sender<Result<(), Error>> },
    CloseWindow { window_id: WindowId, reply: oneshot::Sender<Result<(), Error>> },
    PointerMove { position: Point, reply: ... },
    PointerButton { button: Button, state: ButtonState, reply: ... },
    PointerAxis { dx: f64, dy: f64, reply: ... },
    KeyEvent { key: KeyCode, state: KeyState, reply: ... },
    Shutdown { reply: oneshot::Sender<()> },
}
```

- Commands are processed in FIFO order, which gives input actions their causal order.
- A command must never block the loop; render work is synchronous but bounded by the
  output size, and replies are sent immediately after the frame is produced.
- `RenderedFrame` = `adesk_core::ImageBuffer` + `commit_seq` + damage regions used.
- `NoteLaunch` records a successful `launch_app` in the compositor's launch ledger, so
  the `WindowCreated` the compositor itself publishes can carry `launch_id`. It is
  infallible bookkeeping and acknowledges with `()`.
- `ReserveSeq` changes nothing but the sequence counter: it hands out the next `seq` and
  emits no event. Server-synthesized events (`AppLaunched`, `inspect_frame`) allocate
  their `seq` here, which is what keeps those events inside the single `seq` domain
  of §2.

## 4. Window model and tiling policy (`adesk-wm`)

- `WindowId(u64)` is assigned by the runtime on first map, monotonic, never reused.
- The manager tracks: window id, app id (resolved later), pid, title, surface tree
  handles (opaque `SurfaceKey`), geometry, `state` (`Active` | `Inactive`),
  `last_commit_seq`, popup count, lifecycle state (`Mapping`, `Mapped`, `Closing`).
- Policy: on map → assign id, mark active, request configure of the whole output size
  at `(0,0)`; previous active window becomes inactive (kept mapped, not visible).
  On destroy → if it was active, activate the most recently used remaining window.
- The policy is expressed as pure functions over a `Vec<WindowRecord>` so it can be
  unit-tested without Smithay and replaced for multi-window support later.
- Input coordinate conversion: window-relative `Position` → output `Point` via the
  window's geometry (currently `(0,0)` origin, but never assume it).

## 5. Rendering pipeline (`adesk-render`)

- Renderers: `Gles` (EGL, GPU) and `Pixman` (software). Selection: `--renderer
  auto|gl|pixman`; `auto` tries GL (surfaceless EGL, Mesa llvmpipe in VMs) and falls
  back to pixman with a warning. The chosen renderer is reported by `ping`.
- Rendering is **on demand only**: `RenderWindow`/`RenderOutput` commands. There is no
  frame loop, no continuous composition, no periodic readback.
- `RenderWindow`: renders the window's surface tree (toplevel + subsurfaces + popups
  in z-order) into an offscreen target sized to the window's geometry, then optional
  `region` crop, optional `max_dimension` downscale (box filter), then readback to
  `ImageBuffer` (`Rgba8` or `Png`-encoded by the server).
- Damage: the compositor accumulates per-window damage from `SurfaceCommit` events
  (Smithay's surface damage tracking). `changed_regions` in observations comes from
  this accumulator, coalesced and simplified; it is independent of the renderer.
- The software path must render SHM-backed windows fully (that is what tests use).
  DMA-BUF windows may be unsupported by the software renderer; that must surface as a
  structured `render_failed` error, never a panic.

## 6. Temporal observation (`adesk-observer`)

`ObserverService` consumes `RuntimeEvent`s and maintains per-window temporal state:

```text
last_commit_seq, last_commit_at, last_damage_region, last_meaningful_change_at,
last_input_at, pending_observation, quiet_since, commit_count
```

plus a global `ActionRegistry`: `action_id → {kind, window_id?, position?, seq, ts_ms}`.

Wait implementation (all in the server):

- `wait_for_change(window?, since_commit?, timeout)`: resolve when the first counted
  event arrives; resolve with `timed_out` at the deadline.
- `wait_for_quiet(window?, quiet_ms, timeout, after_action?)`: resolve when
  `now - max(anchor_ts, last counted commit ts) >= quiet_ms`, where `anchor_ts` is the
  `after_action`'s `ts_ms` when one is given, else the time the wait was issued; every
  counted commit re-arms the timer. Without a counted commit the anchor is `anchor_ts`
  alone, so the wait resolves at `anchor_ts + quiet_ms` — immediately when the action
  is already that old.
- `observe(until, after_action?, timeout)`: same machinery with an `until` condition;
  `include_image` triggers `RenderWindow` at resolution time, after the condition, so
  the image reflects the settled state.
- Counted events are filtered by `window_id` (if given) and by `seq > action_seq`
  (if `after_action` given); a *counted commit* is a `SurfaceCommit` that passes those
  filters, and only commits re-arm the quiet timer. `changed_regions` is the union of
  damage in that window.
- `timeout` is a hard bound on every wait: reaching it resolves with `timed_out` (except
  `until: timeout`, whose horizon is its condition). `quiet` in the observation is
  evidence evaluated at resolution time, not a promise about the wait's condition —
  measured from the last counted commit (else the wait start) against the condition's
  `quiet_ms` for a quiet wait, the runtime default otherwise — so a timed-out `change`
  wait can legitimately carry `quiet: true` (`protocol.md` §5.4).
- Timers use `tokio::time`; tests use `tokio::time::pause()` where possible, otherwise
  real time with generous margins. The observer never sleeps in a loop — it waits on
  a `tokio::sync::Notify`/`watch` updated by the event pump.

## 7. Application registry (`adesk-app-registry`)

- Scans `$XDG_DATA_DIRS/applications`, `$XDG_DATA_HOME/applications` (defaults
  `/usr/local/share`, `/usr/share`, `~/.local/share`), recursively, for `*.desktop`.
- Parses the `[Desktop Entry]` group: `Name`, `Exec`, `Icon`, `Terminal`, `Type`,
  `NoDisplay`, `Hidden`, `Categories`, `StartupWMClass`, `DBusActivatable`,
  plus `TryExec`. `Type != Application`, `NoDisplay=true`, `Hidden=true` are filtered
  from `list_apps` by default.
- App id = desktop file id (`org.mozilla.firefox`, `code`), computed from the path
  relative to the applications dir with `/` → `.`.
- `launch_app` expands `Exec` field codes (`%f %F %u %U %i %c %k`, `%%`), honours
  `Terminal=true` (uses `$TERMINAL` or `x-terminal-emulator`), spawns with
  `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` set, and records `launch_id` + pid.
- Correlation: a toplevel mapping within a configurable window (default 10 s) after a
  launch is associated to the app by, in order: exact pid match, `StartupWMClass` ==
  `app_id` of the toplevel, `app_id`/title substring match. Correlation failure is
  reported (window keeps `app_id: null`), never guessed silently.
- DBusActivatable apps: v1 attempts the `Exec` line as a fallback and reports
  `not_supported` only if the entry has no usable `Exec`.

## 8. Compositor (`adesk-compositor`)

Protocols implemented in v1:

- `wl_compositor`, `wl_subcompositor`, `wl_shm`, `xdg-shell` (+ popups),
  `wl_seat` (keyboard, pointer, touch omitted), `wl_output` (one virtual output),
  `wl_data_device_manager` (clipboard basics), `zwp_linux_dmabuf` (DMA-BUF),
  `xdg-decoration` (server-side only), `wl_drm`/`zwp_linux_explicit_sync` if free.
- Out of scope for v1: XWayland, layer-shell, foreign-toplevel, idle protocols,
  screencopy, fractional scale, multi-seat.

Seat and input:

- Keymap is generated from xkb with layout `us` by default (`--xkb-layout`,
  `--xkb-variant`, `--xkb-model`, `--xkb-rules` flags).
- Input injection uses the same seat path as a real device: set pointer/keyboard focus
  from the window model, send `wl_pointer`/`wl_keyboard` events, and forward to the
  focused client. Key chords are pressed in order and released in reverse.
- The compositor owns no agent semantics: it reports what happened, it does not decide
  when to render or what "quiet" means.

## 9. Server and AGP dispatch (`adesk-server`)

- Startup: start compositor thread → wait for ready → bind Unix socket → install
  signal handlers → serve.
- Per connection: NDJSON reader task + writer task + one dispatcher. Requests are
  handled concurrently, except input methods, which go through a per-connection
  ordered queue so `click` → `type_text` causality is preserved.
- The server owns the `ActionRegistry` (ids are allocated here) and the
  `ObserverService`; both are fed by the event pump task.
- `capture_*` / `observe` map to `RenderWindow` commands; images are encoded
  (`png`) or returned raw (`rgba8`) as `adesk_proto` payloads.
- Shutdown (SIGINT/SIGTERM or `Shutdown` command): stop accepting, fail in-flight
  requests with `shutting_down`, drop the Wayland display (clients get
  `wl_display.error`), remove socket files, exit 0.

## 10. Testing architecture

- `adesk-testkit` starts a real compositor + server in-process on a temp socket and
  temp `XDG_RUNTIME_DIR`, using the pixman renderer. It provides:
  - `TestRuntime` — start/stop, client factory, socket path, event tap.
  - `WaylandTestClient` — `wayland-client`-based: connect, create `wl_surface` +
    `xdg_toplevel`, commit SHM buffers with known patterns, respond to configure,
    destroy windows. This is how compositor behavior is verified without a desktop.
  - `.desktop` fixture writer + a helper binary/process for launch tests.
  - Image assertions (exact pixel, region average, "differs from").
- Layers: unit tests (pure logic: wm policy, observer state machine, registry parser,
  protocol codec) → integration tests (`adesk-compositor/tests/`, `adesk-client/tests/`)
  → end-to-end (`adesk-server/tests/`).
- Determinism: no sleeps longer than needed, deadlines explicit, no network, no GPU.
  The GL renderer is exercised only when `ADESK_TEST_GL=1` and must skip cleanly
  otherwise.
