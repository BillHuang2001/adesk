# adesk-server — runtime binary + AGP server

## Intent

`adesk-server` is the composition root of the ADesk runtime and the only process an agent ever talks to.
It starts the compositor thread, binds the AGP Unix socket, pumps `RuntimeEvent`s into the observer and the subscription fan-out, and serves every method of `docs/protocol.md` §5.
It owns the process-lifetime concerns no sibling can own: the socket file, the action→observation wiring, subscription fan-out, signal handling and the ordered shutdown sequence.
It also owns every conversion between sibling crates' overlapping types (`src/translate.rs`), because no sibling depends on another.

Phase 2: every module is implemented (`src/` has no `todo!()`), the public API is unchanged from Phase 1, `cargo test -p adesk-server --lib` is green (130 tests), and the E2E suites in `./tests/` are green as well (184 tests across lib, bin and the eight integration targets, 0 failed, 0 ignored).

## API Surface

Everything below is re-exported at the crate root; `adesk-testkit` is designed against exactly this surface.

- `ServerConfig { socket_path: PathBuf, compositor: CompositorConfig, app_dirs: Option<Vec<PathBuf>> }` (`src/config.rs`)
  - `new(socket_path, compositor)` (the constructor `adesk-testkit`'s harness calls) and `Default` (environment-resolved socket path, compositor defaults); builders `with_socket_path`, `with_compositor`, `with_output_size`, `with_renderer`, `with_xkb`, `with_app_dirs`; `socket_path()`.
  - `default_socket_path()` resolves `$ADESK_SOCKET` → `$XDG_RUNTIME_DIR/adesk.sock` → `<temp_dir>/adesk.sock` (`std::env::temp_dir()` honours `$TMPDIR`); `adesk_client::default_socket_path()` uses the identical chain.
  - `parse_size("WxH")` / `parse_renderer("auto|gl|pixman")` are the CLI value parsers.
- `Server::start(ServerConfig) -> Result<RunningServer, ServerError>` — **async** (`src/server.rs`).
- `RunningServer` (Clone handle; `Debug` prints only the socket path; dropping it does not stop the runtime):
  - `socket_path() -> &Path`, `compositor() -> &CompositorHandle`, `observer() -> &ObserverService`, `registry() -> &Arc<AppRegistry>`, `context() -> &ServerContext`, `shutdown_handle() -> &ShutdownHandle`;
  - `async wait() -> Result<()>` (resolves when the runtime stops serving), `async shutdown() -> Result<()>` (idempotent).
- `ServerContext` (`src/context.rs`): cheap-clone bundle with public fields `config`, `compositor`, `observer`, `registry`, `correlator`, `subscriptions`, `inspect_subscriptions`, `inspection`, `cursor`, `shutdown`, `started_at`; `now_ms()`, `uptime_ms()`, `next_connection_id()`.
- `CursorTracker { set(Point), get() -> Option<Point> }` — last commanded pointer position (the compositor snapshot has no cursor).
- `ShutdownHandle` (`src/shutdown.rs`): `new()`, `initiate() -> bool`, `is_shutting_down()`, `async cancelled()`; `install_signal_handlers`, `run`, `remove_socket_file`.
- `SubscriptionRegistry` / `InspectRegistry` (`src/subscriptions.rs`): `subscribe`, `unsubscribe`, `remove_connection`, `fan_out`/`list`, `len`, `is_empty`; `SubscriptionId = u64`, `EventSink = mpsc::Sender<Frame>`.
- `InspectionCache` (`src/inspection.rs`): implements `adesk_inspector::InspectionSource` over a cached `InspectionSnapshot`; `store`, `snapshot`, async `refresh(&ServerContext)`.
- `ServerError` (`src/error.rs`) with `code() -> ErrorCode` / `payload() -> ErrorPayload`, and the crate alias `Result<T, E = ServerError>` — `Result<T>` and `Result<T, ServerError>` are both valid spellings (startup/bind paths name the error explicitly).
- `SocketListener::bind(&Path)`, `prepare_socket_path`, `Connection::new/run`, `ConnectionWriter::send/try_send`, `Dispatcher::new/dispatch`, `Session`/`InputQueue`, `event_pump::spawn/handle_event/resync`, `images::encode/encode_png`.
- `PROTOCOL_VERSION: u32 = adesk_proto::PROTOCOL_VERSION`, `RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION")`.

## Dispatch coverage (normative: `docs/protocol.md` §5)

One arm per method; unknown method → `unknown_method`; every request gets exactly one response.

| AGP method | Handler |
|---|---|
| `ping` | `dispatch/runtime.rs::ping` |
| `list_apps`, `get_app`, `launch_app` | `dispatch/apps.rs` |
| `list_windows`, `get_window`, `activate_window`, `close_window`, `get_focus` | `dispatch/windows.rs` |
| `capture_window`, `capture_region`, `observe`, `wait_for_change`, `wait_for_quiet` | `dispatch/capture.rs` |
| `pointer_move`, `click`, `double_click`, `mouse_down`, `mouse_up`, `scroll`, `drag`, `keypress`, `key_down`, `key_up`, `type_text` | `dispatch/input.rs` |
| `subscribe_events`, `unsubscribe_events` | `dispatch/events.rs` |
| `inspect_capture`, `inspect_subscribe` | `dispatch/inspect.rs` |

Total: 29 methods, 29 handlers.

## Lifecycle

Startup (`Server::start`, `docs/architecture.md` §9):
1. `adesk_compositor::spawn(CompositorConfig)` (sync, spawns the calloop thread) → `wait_ready()`.
2. Build `AppRegistry` (`RegistryOptions` from `app_dirs`, one `Arc<dyn Clock>` shared with `Correlator`) and `scan()` (blocking → `spawn_blocking`).
3. Bind the Unix socket (`prepare_socket_path` + `SocketListener::bind`).
4. Spawn the event pump (observer + fan-out + `QueryState` resync) and install SIGINT/SIGTERM handlers.
5. Spawn the accept loop; return `RunningServer` once the listener is bound.

Shutdown (`RunningServer::shutdown` / signal → `shutdown::run`), in order:
1. `ShutdownHandle::initiate()` — stop accepting.
2. Fail in-flight requests with `shutting_down`; drain connection tasks.
3. `CompositorHandle::shutdown()` — drops the Wayland display (clients lose their connection).
4. Remove the socket file; complete `wait()` with `Ok(())`; exit code 0.

## Constraints

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; files stay far below the ~1000-line threshold.
- No panics on request/event paths: handlers return `ServerError`; the dispatcher converts it to `ErrorPayload` and keeps the connection open.
- A malformed frame closes **only** that connection; unknown methods answer `unknown_method`; every request gets exactly one response.
- Input methods (§5.5) allocate an `ActionId` via `ObserverService::record_action` **before** sending compositor commands and run through `Session::input()` so they execute in submission order per connection.
- `activate_window` / `close_window` are runtime-native: they change compositor state directly, never synthesize input.
- Observation methods await the observer first, then render; a wait timeout is an `Observation` with `timed_out: true`, never an error.
- Rendering is on demand: `RenderWindow`/`RenderOutput` only when pixels are needed; no screenshot loop, no per-event rendering.
- Error mapping: `adesk_observer::Error::UnknownWindow` → `unknown_window`, `UnknownAction` → `invalid_request`; `adesk_app_registry::Error::UnknownApp` → `unknown_app`, `NoExec` → `not_supported`, `InvalidExec`/`TryExecNotFound`/`Spawn` → `launch_failed`, `InvalidEntry`/`Io` → `internal`; inspector `InvalidFrame`/`InvalidRequest` → `invalid_request`, `Render` → `render_failed`; `adesk_compositor::CompositorError::code()` and `adesk_proto::ProtoError::error_code()` are delegated verbatim, so compositor `unknown_window`/`invalid_request`/`render_failed`/`shutting_down` and protocol `unknown_method`/`protocol_version_mismatch`/`invalid_request` reach the wire unchanged; `Io`/`Join`/`Internal` and everything unclassified → `internal`.
- `tracing` only: one span per request (`request{id method}`), one per connection, one per window lifecycle; never log pixel payloads.
- Timestamps are monotonic ms since `ServerContext::started_at`; no wall clock in observations.
- Dependencies: every version comes from the root `[workspace.dependencies]`; no inline versions.
- Only `adesk-server` may write the AGP socket path and the process-level signal handlers.

## Routing Table

| Area | Owner |
|---|---|
| `ServerConfig`, socket-path defaults, CLI value parsing | `./src/config.rs` |
| `Server::start`, `RunningServer` | `./src/server.rs` |
| `ServerContext`, `CursorTracker` | `./src/context.rs` |
| `ServerError`, AGP error mapping | `./src/error.rs` |
| Socket bind/accept/stale-file handling | `./src/socket.rs` |
| Connection read loop, writer task, NDJSON framing | `./src/connection.rs` |
| Per-connection `Session`, ordered `InputQueue` | `./src/session.rs` |
| Event pump, launch→window correlation stamping, `QueryState` resync, inspection-cache updates | `./src/event_pump.rs` |
| Event + inspector subscription registries, fan-out | `./src/subscriptions.rs` |
| Inspection snapshot cache + async refresh (`InspectionSource`) | `./src/inspection.rs` |
| Image encoding (`ImageBuffer` → `ImagePayload`) | `./src/images.rs` |
| Shutdown token, ordered teardown, SIGINT/SIGTERM | `./src/shutdown.rs` |
| Sibling-type bridges (`StateSnapshot`, `Condition`, `ActionKind`, `RendererName`, markers) | `./src/translate.rs` |
| AGP §5 dispatch table and `RequestContext` | `./src/dispatch/mod.rs` |
| §5.1 runtime (`ping`) | `./src/dispatch/runtime.rs` |
| §5.2 apps (`list_apps`, `get_app`, `launch_app`) | `./src/dispatch/apps.rs` |
| §5.3 windows (`list_windows`, `get_window`, `activate_window`, `close_window`, `get_focus`) | `./src/dispatch/windows.rs` |
| §5.4 capture/observation (`capture_window`, `capture_region`, `observe`, `wait_for_change`, `wait_for_quiet`) | `./src/dispatch/capture.rs` |
| §5.5 input (11 methods) | `./src/dispatch/input.rs` |
| §5.6 subscriptions (`subscribe_events`, `unsubscribe_events`) | `./src/dispatch/events.rs` |
| §5.7 inspector (`inspect_capture`, `inspect_subscribe`) | `./src/dispatch/inspect.rs` |
| CLI binary (`adesk-server`) | `./src/main.rs` |
| E2E test plan and Phase 2 suites | `./tests/` (see `./tests/CONTEXT.md`) |

## Design Decisions

- **`ServerConfig::new(socket_path, compositor)` is a 2-argument constructor; defaults come from `Default`.** The objective pinned the struct, its builders and its default resolution but not the constructor arity; `adesk-testkit`'s harness (designed in parallel) calls `ServerConfig::new(socket_path, compositor).with_app_dirs(..)`, so the server matches the consumer. `Default` keeps the env-resolved socket path and compositor defaults.
- **Dev-dependencies are only what `./tests/` needs** (`adesk-client`, `tempfile`; `adesk-testkit` lands with the suites) — the in-module unit tests use no test-only crates.
- **`Server::start` and `RunningServer::wait`/`shutdown` are async.** The objective's signature sketch omitted `async`, but compositor `wait_ready()`/`shutdown()` are async and the server is a tokio process; `adesk-testkit` must `.await` them. `registry() -> &Arc<AppRegistry>` (not `&AppRegistry`) is pinned so tests can clone the handle.
- **One writer task per connection + bounded queue.** The read loop owns decoding/dispatch; a dedicated writer task owns the write half and is fed by an `mpsc::Sender<Frame>` (`ConnectionWriter`). Responses `send().await` (backpressure); subscription events `try_send` (drop, never block the pump). This keeps every request answered exactly once even while events stream.
- **An unknown method is answered inline by the read loop.** `ProtoError::UnknownMethod` carries only the method name, so the read loop lifts the request id from the raw line and answers `unknown_method` through `error_response` (connection stays open, §1); a line without a usable id is malformed framing and still closes the connection.
- **Every dispatch task races the shutdown token.** The task selects (biased) between `ShutdownHandle::cancelled()` and the handler, so a request already in flight when the runtime stops answers `shutting_down` instead of running to its own timeout.
- **Ordered input queue is a fair `tokio::sync::Mutex` per session.** Input methods may expand into several compositor commands (`click` = move + down + up); the queue guarantees submission order per connection without serializing the whole connection.
- **§5.5 `window_id` never reaches the compositor.** `RuntimeCommand::PointerMove`/`PointerButton`/`PointerAxis`/`KeyEvent` carry only their payload plus `reply`; the handler uses `window_id` for four things: (1) resolve window-relative coordinates from `QueryState` window geometry (`input.rs::window_rect`), (2) `record_action` on the observer, (3) for keyboard methods, activate the window first when `snapshot.keyboard_focus.or(snapshot.active_window_id) != Some(window_id)` (`input.rs::activate_if_needed` → `RuntimeCommand::ActivateWindow`), (4) label an `unknown_window` failure. Pointer methods never activate: the compositor delivers buttons/axes at the current pointer position, so the preceding `PointerMove` must land inside the target window's surface. Both the coordinate lookup and `activate_if_needed` validate the window *before* any compositor command, so an unknown id injects nothing.
- **`ErrorPayload.data` is attached only for the three lookup failures that name an id** (observer `UnknownWindow`/`UnknownAction`, registry `UnknownApp`). The compositor-path `unknown_window` — every §5.5 input method, `get_window`, `activate_window`, `close_window`, `capture_*` — carries `code` + `message` only (`error.rs::payload`).
- **`InspectionSource` is synchronous, the runtime is not.** `adesk-inspector` requires a cheap, non-blocking `inspection_input()`; the server therefore keeps an `InspectionCache` and refreshes it asynchronously (`inspection::refresh` renders the full output and issues `QueryState`) immediately before each `inspect_*` frame. Overlays are composited at full resolution and cropped/downscaled afterwards.
- **`translate.rs` owns every cross-crate conversion** (`StateSnapshot`, `Condition`, `ActionKind`, `RendererName`, `Position` → `Point`, `Observation` + image → `ObserveResult`). This is deliberate: it is the single file to change if a sibling changes shape, and it documents the mismatches rather than hiding them. The `Observation` bridges are pure pairings/mappings — the server never rewrites `timed_out`, `quiet` or the resolved condition before responding.
- **Lag handling is `QueryState` + `resync`, not best-effort patching.** On `broadcast::error::RecvError::Lagged(n)` the pump issues `QueryState`, translates the snapshot and calls `ObserverService::resync`, which marks affected windows uncertain (`docs/architecture.md` §2).
- **Shutdown token is a `tokio::sync::watch<bool>`**, so `cancelled()` cannot miss an already-flagged shutdown (no `Notify` registration race) and `initiate()` is idempotent.
- **Socket lifecycle is RAII + explicit**: `SocketListener::drop` unlinks the socket file only while the path still holds the `(device, inode)` pair it bound, so a successor runtime that rebound the same path keeps its socket; `shutdown::run` removes the file so it is gone before `wait()` resolves. A live socket is never clobbered (`prepare_socket_path` probes with a connect).
- **`adesk-render` and `adesk-wm` are declared dependencies even though dispatch reaches them only through the compositor handle.** They are part of the composition contract (workspace map) and keep the server's dependency list aligned with the crates it composes.
- **`serde_json` is a direct dependency** (beyond the objective's list) because `ErrorPayload::with_data` carries structured error data (e.g. `{"window_id": 99}`) and `ResultPayload` wraps `serde_json::Value`. `serde` itself is not a direct dependency: the server never derives its own wire types.
- **The event pump is the matching half of the launch → window correlation.** `launch_app` records the `LaunchRecord` *and* sends `RuntimeCommand::NoteLaunch` to the compositor (best-effort, after the successful spawn); `event_pump::correlate_window` feeds a `WindowCreated` with `launch_id: None` to `Correlator::correlate` as a `WindowCandidate` (window id, pid, app_id, title) and hands the *same* stamped event to the observer, the inspection cache and the fan-out, so all three agree. It returns a `Cow`: only a correlated `WindowCreated` is cloned. Uncorrelated windows pass through untouched (never guessed), events that already carry a `launch_id` (e.g. the ones the compositor stamped itself) are never re-correlated, and the lock is poison-recovering. This projects a server-side verdict onto the AGP/observer view; the compositor stamps its own broadcast through its launch ledger.

## Sibling integration notes (contracts the server must satisfy)

- **adesk-compositor**: `spawn` is sync; `wait_ready()` is async and cached; result-bearing `RuntimeCommand`s carry `oneshot::Sender<adesk_core::Result<..>>`; `QueryState` is infallible; `NoteLaunch` acknowledges `()` and is best-effort (`launch_app` logs a failure and still returns the launch). `StateSnapshot` is `adesk_compositor::StateSnapshot`, which is **not** the observer's snapshot — bridge via `translate::observer_snapshot`.
- **adesk-observer**: record the action before the command; `resync` after lag; waits return observations (timeouts are not errors); `Err(UnknownWindow)` → `unknown_window`, `Err(UnknownAction)` → `invalid_request`; render images *after* the wait; `ActionKind::as_str()` is the AGP method name; a wait whose condition is not `Quiet` carries no per-request quiet threshold, so its `quiet` evidence flag uses the observer's `default_quiet_ms` (250 — `Server::start` calls `ObserverService::new()` and there is no `with_config` call site).
- **adesk-app-registry**: registry and correlator must share one `Arc<dyn Clock>`; after a successful `launch` emit `AppLaunched {launch_id, app_id, pid}` on the compositor's broadcast and `correlator.record_launch(record, &app_info)`; the pump then correlates each `WindowCreated` that lacks a `launch_id` (`Correlated` → stamp `launch_id` on the projected event; `Uncorrelated` → pass through, never guess); `scan()` is blocking; child reaping is the server's job; set `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` on launches via `LaunchEnv`.
- **adesk-inspector**: implement `InspectionSource` (done by `InspectionCache`); full-output render before overlays; `inspect_capture` = `Inspector::new(overlays).render_request(input, request)` then PNG; `inspect_subscribe` = same per frame, throttled, one `Inspector` per subscription; map inspector errors with `adesk_core::Error::from`.
- **adesk-proto**: `ObserveResult` serializes as `{"observation": {..., "image": ...}}`; `ImagePayload::from_rgba8`/`from_png` are the only constructors; `Method::from_parts` is the decode entry point; defaults (`timeout_ms=5000`, `quiet_ms=250`, `observe.include_image=true`, waits `false`, `format=png`) are the proto crate's, the server must not redefine them.
- **adesk-client**: `ping` validates `protocol_version` (report `PROTOCOL_VERSION` exactly); error responses must not close the connection; waits never attach pixels unless `include_image` is set; its `default_socket_path()` chain matches this crate's exactly.

## Known Issues

- **Signal handlers are installed by `Server::start`**, including in test processes; repeated installation is harmless (`tokio::signal` supports multiple listeners), but tests must not send SIGINT to the test runner.
- **Window-creating E2E is not covered yet**: the suites in `./tests/` run against an empty runtime (no Wayland client ever connects), so tiling, focus transitions and input delivery are not exercised here; that coverage now lives in the `adesk-testkit` and `adesk-agent` suites. Launch correlation *is* covered by injecting a synthetic `WindowCreated` into the compositor's broadcast.
- **A `quiet` event subscription is accepted but never delivers.** `subscribe_events` accepts `quiet` (only `inspect_frame` fails `EventKind::is_subscribable()`), yet no quiet frame is ever produced: the pump fans out `RuntimeEvent`s only, `SubscriptionRegistry::fan_out` filters with `EventKind::matches`, which never matches `Quiet`, and no code path constructs `EventPayload::Quiet` (the only server-synthesized event kind is `inspect_frame`, `dispatch/inspect.rs`). This matches `docs/protocol.md` §5.6 (no v1 emitter; quietness is pull-style via `wait_for_quiet`/`observe`). Pinned by the unit test `subscriptions::tests::protocol_only_kinds_never_match_runtime_events`; no E2E in `./tests/` covers a quiet subscription.
- **No E2E case exercises keyboard methods without `window_id`** (the no-focus path that must answer `invalid_request`, `input.rs::activate_if_needed` returns early and the compositor reports "no window has keyboard focus"), and no case exercises a *successful* injection — every §5.5 E2E call targets an unknown window.

## Test Strategy

- **Unit level (in-module):** `config::parse_size/parse_renderer/default_socket_path`, `translate` (every bridge, both directions), `images::encode` (png/rgba8/scale), `subscriptions` (id allocation, filtering, removal on disconnect), `session::InputQueue` (FIFO), `shutdown::ShutdownHandle` (idempotence, `cancelled`), `inspection::InspectionCache`, `error` (compositor/proto mapping tables), `socket::SocketFileId` (drop identity check), `connection::request_id_from_line`, `event_pump::correlate_window` (stamping, pass-through, no re-correlation, other event kinds, poisoned lock); `cargo test -p adesk-server --lib` is green.
- **E2E level (`./tests/`):** `common::TestRuntime` starts one real runtime per test (`Server::start`) on a private temp socket with the pixman renderer, driven through `adesk-client` and the raw NDJSON `RawClient`; full plan in `./tests/CONTEXT.md`. No test may require a display, GPU, network or installed application (`subscriptions.rs`'s launch fixture spawns `true` from `PATH`).
- **Validation commands (always through the dev shell):** `./scripts/dev.sh cargo check -p adesk-server --all-targets`, `./scripts/dev.sh cargo test -p adesk-server`, `./scripts/dev.sh cargo clippy -p adesk-server --all-targets --no-deps -- -D warnings`, `./scripts/dev.sh cargo doc -p adesk-server --no-deps` — all warning-free.

## Notes for Agents

- Do not add AGP methods or fields outside `docs/protocol.md`; the dispatcher must stay total over `adesk_proto::Method`.
- Keep the public API stable: `adesk-testkit` and `adesk-agent` are written against it. If a change is unavoidable, report it to the parent instead of editing siblings.
- The crate has no blanket `allow` attributes: keep `clippy -D warnings` and `cargo doc` clean without adding new ones.
