# adesk-testkit — dev-only headless test harness

## Intent

`adesk-testkit` makes the whole ADesk runtime testable without a display, GPU, network or installed application.
It is the only supported way to run ADesk end-to-end tests: an in-process runtime on private temp paths, a real Wayland protocol client, `.desktop` fixtures, a helper process, and deadline-bounded image/event assertions.
**Dev-dependency target only** — no runtime crate may depend on it, and the `adesk-test-app` binary is never shipped.
Status: implemented — `src/` and `tests/` contain no `todo!()`/`unimplemented!()` bodies, and the full `cargo test -p adesk-testkit --all-features` suite passes under the dev shell (66 tests + 3 doctests, 0 failures; the 3 ignored doctests are doc-code fences — no behavioural skips).

## API Surface

- `TestRuntime` / `TestRuntimeConfig` — start a real compositor thread + AGP server in-process (pixman, `1280x800` default) on a temp `XDG_RUNTIME_DIR` and temp socket; accessors `socket_path()`, `wayland_display()`, `output_size()`, `tiled_rect()`, `compositor()`, `observer()`, `registry()`, `env()`, `config()`, `client()`, `event_tap()`, `wayland_client()`, `capture()`, `wait_for_window()`, `wait_for_window_app()`, `shutdown()`; `Drop` is non-blocking and never hangs.
- `expected_window_geometry(Size) -> Rect` — the tiled rect, single-sourced from `adesk-wm`'s policy; tests must never hard-code output size.
- `WaylandTestClient` / `ToplevelSpec` / `TestWindow` / `ConfiguredSize` / `PopupSpec` / `TestPopup` / `PumpStats` / `Globals` (negotiated versions, incl. `seat_version()` / `data_device_manager_version()`) — a `wayland-client`-based client that speaks the real protocol path (SHM buffers, xdg-shell toplevels/popups, seat, data device), commits known fills and exposes deadline-bounded event pumps. `TestWindow::close_requested()` reports a compositor `xdg_toplevel.close` while the surface is still alive; `TestWindow` also exposes `app_id()`, `title()`, `size()`, `fill()`, `damage_hint()`, `is_destroyed()`, `pending_configure()`, `last_configure()`.
- Input recording: `PointerEvent` / `KeyboardEvent` / `AxisKind` / `ButtonState` / `KeyState` / `ModifiersState` and `BTN_LEFT` / `KEY_LEFTCTRL` / `KEY_C`; `WaylandTestClient::{pointer_events, keyboard_events, clear_input_events, wait_for_pointer_event(timeout, what, pred), wait_for_keyboard_event, wait_for_pointer_button(button, state, timeout), wait_for_key(keycode, state, timeout), last_modifiers}` — records `wl_pointer` enter/leave/motion/button/axis/frame and `wl_keyboard` keymap/enter/leave/key/modifiers/repeat_info exactly as delivered (surface-local coordinates; raw evdev codes); every wait is deadline-bounded. See `src/wayland/input.rs`.
- Clipboard: `DEFAULT_SELECTION_TIMEOUT` (10 s) and `WaylandTestClient::{set_selection(mime, bytes), clear_selection(), read_selection(mime), read_selection_with_timeout(mime, timeout), selection_offer_count(), wait_for_selection_offer(count, timeout)}` — `wl_data_device_manager` v3 selection transfers between real clients; `read_selection*` returns the exact bytes, or `Ok(None)` when the offer does not advertise the mime. See `src/wayland/clipboard.rs`.
- `FixtureDir` / `DesktopEntryFixture` / `TestAppSpec` / `TestApp` / `helper_bin_path` — `.desktop` fixtures in a temp `XDG_DATA_DIRS` share root and a helper process that opens a real toplevel.
- `TestEnv` / `EnvScope` — isolated `XDG_RUNTIME_DIR` / `XDG_DATA_DIRS` / `XDG_DATA_HOME` / `WAYLAND_DISPLAY` / `ADESK_SOCKET` plus RAII restore of the process env.
- `FillPattern` / `DEFAULT_FILL` — the single ground truth for SHM pixels and image assertions.
- `ImageAssert` / `EventAssert` / `Expected` — assertions that panic with detailed messages.
- `wait_until` / `wait_until_async` / `poll_until` / `block_until` — the only place the harness sleeps; every wait is deadline-bounded.
- `gl_enabled` / `require_gl` / `test_renderer` / `GL_ENV_VAR` — GL gating via `ADESK_TEST_GL=1`.
- `TestkitError` / `Result` — one error enum; `Timeout { what, timeout }` is the canonical bounded-wait failure.

## Constraints

- Dev-only: it is a `[dev-dependencies]` entry, never a runtime dependency; `adesk-test-app` is a test helper binary.
- No test may require a display, GPU, network or a specific installed application; the default renderer is pixman and GL paths must skip cleanly without `ADESK_TEST_GL=1`.
- Every wait, pump and shutdown has an explicit deadline; a hung runtime fails the test with `Timeout`, it never blocks forever.
- Assertions panic; all other fallible calls return `Result<_, TestkitError>`.
- Real protocol path only: the Wayland client drives the compositor exactly like an ordinary application and must never reach into compositor internals.
- Crate-wide `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay well under ~1000 lines.
- Dependency versions come from the root `[workspace.dependencies]` only.

## Routing Table

| Area | Owner |
|---|---|
| Runtime lifecycle, config, accessors | `src/runtime.rs` |
| Isolated XDG dirs, process-env scoping | `src/env.rs` |
| Wayland test client: protocol path, SHM, windows | `src/wayland/` |
| Client-side input recording (pointer/keyboard history + waits) | `src/wayland/input.rs` |
| Clipboard / data-device helpers (selection, pipe pairs) | `src/wayland/clipboard.rs` |
| `.desktop` fixtures, helper process | `src/fixtures/` |
| Helper binary `adesk-test-app` | `src/bin/adesk-test-app.rs` |
| Image and event assertions | `src/assert/` |
| Fill patterns (pixel ground truth) | `src/fill.rs` |
| Deadline-bounded waiting | `src/wait.rs` |
| GL capability gating | `src/gate.rs` |
| Error type | `src/error.rs` |
| Self-tests of the harness | `tests/` |

## Design Decisions

- **Reader-thread Wayland client.** `wayland-client` is synchronous while the runtime is tokio; a reader thread owns the `EventQueue` and dispatches into `Arc<Mutex<ClientState>>`, feeding a `tokio::sync::mpsc` channel. Public pumps await that channel with a deadline; teardown uses `UnixStream::shutdown` plus a bounded join, so nothing can block forever.
- **Input recording is seat-driven and reader-thread-owned.** The client binds `wl_seat` (negotiated to at most v11) and creates `wl_pointer`/`wl_keyboard` over the real protocol path; the reader thread appends every event to append-only histories in `ClientState`, and the `wait_for_*` accessors scan those histories in `block_until` slices — they never drain the pump channel, so a concurrent `pump_until`/`roundtrip` cannot lose notifications. `clear_input_events()` empties the histories but keeps the latest input serial (it belongs to the seat's input stream, not the history). Recording rules: `src/wayland/input.rs`.
- **Clipboard pipes are `rustix::pipe::pipe()` `OwnedFd` pairs.** MSRV 1.80 predates `std::io::pipe` and the crate is `#![forbid(unsafe_code)]`, so rustix is the only honest pipe source; `rustix = { version = "1", features = ["pipe"] }` is declared in the root manifest like every other dependency.
- **Selection transfers are small and fully bounded.** `set_selection` payloads are written synchronously on the reader thread when the compositor sends `wl_data_source.send` (documented ≤64 KiB rule) and the fd is closed right after; `read_selection_with_timeout` is two deadline-bounded phases (offer wait, then pipe-to-EOF) — worst case 2×timeout, the failing phase named in the `Timeout`; a receive worker that misses its deadline is detached (bounded by process exit). Keymap fds are dropped immediately and never mmap'ed. Details: `src/wayland/clipboard.rs`.
- **Absolute-path connect.** `WaylandTestClient::connect_in(runtime_dir, display)` takes the socket path explicitly because the process env is global and races under parallel tests; `connect(display_name)` (env-based) exists for external clients.
- **One pixel ground truth.** `FillPattern::at(x, y, size)` is evaluated both by the SHM writer and by `ImageAssert::matches_pattern`, so the client and the assertion can never disagree. Only opaque patterns are accepted (`require_opaque`), committed as Argb8888.
- **No `libc`.** SHM is a `tempfile`-backed anonymous file written through `FileExt::write_all_at`; no `mmap` and no `unsafe`.
- **`Drop` never blocks.** `TestRuntime::drop` sends `RuntimeCommand::Shutdown` synchronously and detached-spawns the graceful server shutdown inside the current tokio runtime; only `shutdown().await` observes errors or applies `shutdown_timeout`.
- **Process env is explicit.** `TestRuntimeConfig::apply_env` (default on) scopes the env for the runtime's lifetime because a launched child's `XDG_RUNTIME_DIR` is read from the *server process* env (`adesk-server/src/dispatch/apps.rs` builds `LaunchEnv` from `std::env::var_os("XDG_RUNTIME_DIR")`); the hazard for parallel tests is documented in `src/env.rs`.
- **Fixtures speak the registry's language.** Share roots (`with_fixture_dir`/`with_app_dirs`) are translated to `<root>/applications` before the server sees them because the registry derives app ids relative to each configured dir, and `Exec` arguments are quoted iff empty or containing ASCII whitespace/`"`/`\` so `adesk_app_registry`'s tokenizer returns them unchanged; both rules live in `src/runtime.rs`/`src/fixtures/mod.rs` module docs.
- **Close is observed, never assumed.** The reader records `xdg_toplevel.close` in the window slot and the client keeps the surface alive, so `TestWindow::close_requested()` lets `tests/e2e_close.rs` prove the AGP `close_window` request path instead of inferring it from the window's disappearance.
- **GL is opt-in and fails loudly.** `test_renderer()` returns `RendererKind::Gl` when `ADESK_TEST_GL=1`, so a broken GL setup fails instead of silently falling back to pixman.
- **Assertions panic, plumbing returns `Result`.** `ImageAssert`/`EventAssert` are assertions (`assert_eq!` semantics with pixel/event detail); everything else returns `TestkitError`.
- **`Timeout.what` is `&'static str`.** `Expected`-based event waits cannot supply a borrowed description, so they document a bounded `Box::leak` of the describe string (test-process bounded); revisit if `Timeout.what` ever becomes a `String`.
- **Tiling is never hard-coded.** `expected_window_geometry` delegates to `adesk_wm::PolicyConfig::tiled_rect()`.

## Test Strategy

- Self-tests live in `tests/`: `runtime.rs` (start/stop, ping, renderer, drop), `wayland_client.rs` (toplevel appears in `list_windows` with the tiling configure, commit → `SurfaceCommit`, captured pixels match the fill, popups, resize), `input_capture.rs` (real-seat pointer/keyboard recording: motion surface coordinates match AGP injection, press/release ordering, axis + frame, `ctrl+c` chord press/reverse-release, focus enters), `clipboard.rs` (selection round trip between two clients, second `set_selection` supersedes the first offer, unadvertised mime → `Ok(None)`, bounded no-selection timeout, no payload leak into events), `fixtures.rs` (`.desktop` writing, launch path, helper process), `assertions.rs` (ImageAssert/EventAssert/`wait_until` self-checks), `api_surface.rs` (signature stability), `e2e_launch_observe.rs` (capstone: launch → observe → capture → input → close round trip) and `e2e_close.rs` (cooperating-client proof that `close_window` really sends `xdg_toplevel.close`).
- Unit tests inside `src/assert/`, `src/fixtures/` and `src/bin/adesk-test-app.rs` cover the already-implemented plumbing.
- Run with `./scripts/dev.sh cargo test -p adesk-testkit --all-features`; the full suite passes (66 tests + 3 doctests, 0 failures) and no test is `#[ignore]`d; the only gate is the GL paths (skip cleanly without `ADESK_TEST_GL=1`).
- No test needs a display, GPU, network or installed app; the helper binary is built by cargo (`env!("CARGO_BIN_EXE_adesk-test-app")` is available to this package's integration tests).
- Launch tests mutate the process env; the harness serializes env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.

## Dependencies

- Workspace deps: `adesk-core`, `adesk-proto`, `adesk-wm`, `adesk-observer`, `adesk-app-registry`, `adesk-compositor`, `adesk-client`, `adesk-server`, `wayland-client 0.31`, `wayland-protocols 0.32`, `tokio`, `tempfile`, `image` (png), `thiserror`, `tracing`, `rustix 1` (feature `pipe`, clipboard pipes).
- `cargo check` does not link, so it runs outside the Nix dev shell; building/running tests needs `./scripts/dev.sh`.

## Known Issues

- `tests/fixtures.rs` prints `Io error: Broken pipe (os error 32)` on stdout while still passing — log noise from the helper-process path, not a failure.
- Clipboard publication is accepted only while the publishing client holds keyboard focus: Smithay checks the keyboard focus at dispatch time and ignores the quoted serial, silently dropping the request otherwise — so `set_selection` returning `Ok(())` proves only "requests on the wire", never delivery; prove it with `selection_offer_count`/`wait_for_selection_offer` plus a read.
- Ordering hazard in the clipboard tests: `roundtrip()` is a flush plus one reader cycle (not a `wl_display.sync` barrier), so a `set_selection` flushed immediately before an `activate_window` can be dispatched after the focus moved away and be silently dropped; `tests/clipboard.rs::second_set_selection_invalidates_the_first_offer` re-activates the reader with no barrier after publishing the replacement, which presents as either a re-announced old payload (payload assertion fails) or a `selection offer count` timeout — never a silent pass.

## Notes for Agents

- **`adesk-server` coupling**: `runtime.rs` is the only file that touches `ServerConfig`/`Server::start`/`RunningServer` (contract in its module docs); adapt only `TestRuntime::start_with`/`shutdown` when that contract changes.
- **Consumer wiring**: the crate is declared once in the root `[workspace.dependencies]` (`Cargo.toml:31`); a consumer adds it under `[dev-dependencies]`. `adesk-agent` does, gated by its `e2e` feature (`crates/adesk-agent/Cargo.toml`), and its `tests/e2e_runtime.rs` (14 tests) drives a real runtime through the harness.
- **Launch needs `apply_env(true)`; a `WaylandTestClient` does not.** The server reads the *process* `XDG_RUNTIME_DIR` to build a launched child's env, so AGP `launch_app` only works while the runtime's env is applied (and the runtime then holds the process-env lock for its lifetime — a second env-scoped runtime in the same test fails with `Timeout` after `PROCESS_ENV_LOCK_TIMEOUT` = 60 s). `wayland_client()` connects by absolute path, so windows can be mapped with `apply_env(false)`, which releases the lock as soon as the server is up.
- **Pointer button/axis need pointer focus first.** The compositor drops `wl_pointer.button`/`axis` unless a prior move established pointer focus (Smithay's default grab); input tests always move the pointer onto the target window first.
- **Window ids are per-runtime, not per-test.** In a fresh `TestRuntime` the first mapped toplevel is `WindowId(1)` and the second `WindowId(2)` (counter in `adesk-wm`, allocated on the first buffer commit; nothing else consumes ids). Popups use a separate `u64` popup-id space, never a `WindowId`, and never appear in `list_windows` (only `WindowInfo.popup_count`); they surface as `PopupAppeared/Disappeared` events and `Observation.popups_appeared/disappeared` keyed to the owning window.
- **Capture forms.** `TestRuntime::capture(id)` returns an unscaled RGBA8 `ImageBuffer` at the window's natural size; AGP `observe` attaches a PNG `ImagePayload` inside the observation (decode with `ObserveResult::decode_image()`), and `max_dimension` downscales by longest edge with half-up rounding, never upscaling.
- `wayland/mod.rs` documents the reader-thread, lock-order and teardown rules; `wayland/input.rs` the recording rules; `wayland/clipboard.rs` the fd lifecycle; `wayland/shm.rs` the Argb8888 byte order; `wayland/state.rs` holds the core `Dispatch` impls (`wayland/clipboard.rs` adds the data-device ones beside its helpers).
- The frozen acceptance specs (`tests/api_surface.rs`, `tests/assertions.rs`, `tests/fixtures.rs`, `tests/runtime.rs`, `tests/wayland_client.rs`) must not be edited; add new behavior proof in new test files instead.
