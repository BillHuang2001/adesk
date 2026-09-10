# adesk-testkit — dev-only headless test harness

## Intent

`adesk-testkit` makes the whole ADesk runtime testable without a display, GPU, network or installed application.
It is the only supported way to run ADesk end-to-end tests: an in-process runtime on private temp paths, a real Wayland protocol client, `.desktop` fixtures, a helper process, and deadline-bounded image/event assertions.
**Dev-dependency target only** — no runtime crate may depend on it, and the `adesk-test-app` binary is never shipped.
Status: complete — `src/` and `tests/` contain no `todo!()`/`unimplemented!()` bodies, and no behavioural test is skipped (the only `ignore`d doc blocks are doc-code fences).

## API Surface

- `TestRuntime` / `TestRuntimeConfig` — start a real compositor thread + AGP server in-process (pixman, `1280x800` default) on a temp `XDG_RUNTIME_DIR` and temp socket; accessors `socket_path()`, `wayland_display()`, `output_size()`, `tiled_rect()`, `compositor()`, `observer()`, `registry()`, `env()`, `config()`, `client()`, `event_tap()`, `wayland_client()`, `capture()`, `wait_for_window()`, `wait_for_window_app()`, `shutdown()`; `Drop` is non-blocking and never hangs.
- `expected_window_geometry(Size) -> Rect` — the tiled rect, single-sourced from `adesk-wm`'s policy; tests must never hard-code output size.
- `WaylandTestClient` / `ToplevelSpec` / `TestWindow` / `ConfiguredSize` / `PopupSpec` / `TestPopup` / `PumpStats` / `Globals` (negotiated versions, incl. `seat_version()` / `data_device_manager_version()`) — a `wayland-client`-based client that speaks the real protocol path (SHM buffers, xdg-shell toplevels/popups, seat, data device), commits known fills and exposes deadline-bounded event pumps. `TestWindow::close_requested()` reports a compositor `xdg_toplevel.close` while the surface is still alive; `TestWindow` also exposes `app_id()`, `title()`, `size()`, `fill()`, `damage_hint()`, `is_destroyed()`, `pending_configure()`, `last_configure()`, `commit_frame(fill)`, `commit_pending()`.
- Input recording: `PointerEvent` / `KeyboardEvent` / `AxisKind` / `ButtonState` / `KeyState` / `ModifiersState` and `BTN_LEFT` / `KEY_LEFTCTRL` / `KEY_C`; `WaylandTestClient::{pointer_events, keyboard_events, clear_input_events, wait_for_pointer_event(timeout, what, pred), wait_for_keyboard_event, wait_for_pointer_button(button, state, timeout), wait_for_key(keycode, state, timeout), last_modifiers}` — records `wl_pointer` enter/leave/motion/button/axis/frame and `wl_keyboard` keymap/enter/leave/key/modifiers/repeat_info exactly as delivered (surface-local coordinates; raw evdev codes); every wait is deadline-bounded. See `src/wayland/input.rs`.
- Clipboard: `DEFAULT_SELECTION_TIMEOUT` (10 s) and `WaylandTestClient::{set_selection(mime, bytes), clear_selection(), read_selection(mime), read_selection_with_timeout(mime, timeout), selection_offer_count(), wait_for_selection_offer(count, timeout)}` — `wl_data_device_manager` selection transfers between real clients (v1 carries the whole path; the bound version is negotiated and reported by `Globals::data_device_manager_version()`); `read_selection*` returns the exact bytes, or `Ok(None)` when the offer does not advertise the mime. See `src/wayland/clipboard.rs`.
- `FixtureDir` / `DesktopEntryFixture` / `TestAppSpec` / `TestApp` / `helper_bin_path` — `.desktop` fixtures in a temp `XDG_DATA_DIRS` share root and a helper process that opens a real toplevel.
- `TestEnv` / `EnvScope` — isolated `XDG_RUNTIME_DIR` / `XDG_DATA_DIRS` / `XDG_DATA_HOME` / `WAYLAND_DISPLAY` / `ADESK_SOCKET` plus RAII restore of the process env.
- `FillPattern` / `DEFAULT_FILL` — the single ground truth for SHM pixels and image assertions.
- `ImageAssert` / `EventAssert` / `Expected` — assertions that panic with detailed messages.
- `wait_until` / `wait_until_async` / `poll_until` / `block_until` — the only place the harness sleeps; every wait is deadline-bounded.
- `gl_enabled` / `require_gl` / `test_renderer` / `GL_ENV_VAR` — GL gating via `ADESK_TEST_GL=1`.
- `TestkitError` / `Result` — one error enum; `Timeout { what, timeout }` is the canonical bounded-wait failure.

## Constraints

- Dev-only: it is a `[dev-dependencies]` entry, never a runtime dependency; `adesk-test-app` is a test helper binary.
- No test may require a display, GPU, network or a specific installed application; the default renderer is pixman and GL paths must skip cleanly without `ADESK_TEST_GL=1`; no behavioural test is `#[ignore]`d (the ignored doctests are doc-code fences).
- Every wait, pump and shutdown has an explicit deadline; a hung runtime fails the test with `Timeout`, it never blocks forever.
- Assertions panic; all other fallible calls return `Result<_, TestkitError>`.
- Real protocol path only: the Wayland client drives the compositor exactly like an ordinary application and must never reach into compositor internals.
- Crate-wide `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files follow the workspace size rule (~1000 lines is the concern threshold — `src/fixtures/mod.rs` is the one file above it, because its fixture unit tests live in the same file).
- Dependency versions come from the root `[workspace.dependencies]` only.
- Rustfmt-clean under the workspace defaults (`./scripts/dev.sh cargo fmt -p adesk-testkit --check`).

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

- Self-tests live in `tests/`: `runtime.rs` (start/stop, ping, renderer, drop), `wayland_client.rs` (toplevel appears in `list_windows` with the tiling configure, commit → `SurfaceCommit`, captured pixels match the fill, popups, resize), `shm_pool.rs` (SHM pool regression: ten fresh frames in a row reuse released ranges, and six destroyed windows' final buffers are reclaimed — both exhaust the 16 MiB pool unless a `wl_buffer.release` is attributed by buffer identity), `input_capture.rs` (real-seat pointer/keyboard recording: motion surface coordinates match AGP injection, press/release ordering, axis + frame, `ctrl+c` chord press/reverse-release, focus enters), `clipboard.rs` (selection round trip between two clients, second `set_selection` supersedes the first offer, unadvertised mime → `Ok(None)`, bounded no-selection timeout, no payload leak into events; every publication is ordered by the `publish` barrier described in that file's module docs), `fixtures.rs` (`.desktop` writing, launch path, helper process), `assertions.rs` (ImageAssert/EventAssert/`wait_until` self-checks), `api_surface.rs` (signature stability), `e2e_launch_observe.rs` (capstone: launch → observe → capture → input → close round trip) and `e2e_close.rs` (cooperating-client proof that `close_window` really sends `xdg_toplevel.close`).
- Unit tests inside `src/assert/`, `src/wayland/`, `src/fixtures/` and `src/bin/adesk-test-app.rs` cover the harness plumbing.
- Run with `./scripts/dev.sh cargo test -p adesk-testkit`; the crate declares no Cargo `[features]`, so `--all-features` is a no-op. GL-dependent paths are gated by the `ADESK_TEST_GL=1` environment variable (`src/gate.rs`), which skips cleanly when unset.
- No test is `#[ignore]`d; the doc-fence inventory is 3 `no_run` (`src/lib.rs`, `src/gate.rs`, `src/fixtures/mod.rs`) + 3 `ignore` + 5 `text`.
- No test needs a display, GPU, network or installed app; the helper binary is built by cargo (`env!("CARGO_BIN_EXE_adesk-test-app")` is available to this package's integration tests).
- Launch tests mutate the process env; the harness serializes env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.

## Performance Notes (measured)

Per-binary wall time (single `cargo test` run) and what dominates it:
- `tests/e2e_launch_observe.rs` 3.63 s — ~3 s is `HELPER_LIFETIME = 3 s` (`tests/e2e_launch_observe.rs:88`): the fixture helper ignores `xdg_toplevel.close` and only exits on its own `--exit-after` deadline, so `close_window` → `window_destroyed` costs the whole lifetime; the rest is the 250 ms `QUIET_MS` observation plus one runtime spin-up.
- `tests/shm_pool.rs` 2.59 s — 16 fixed `pump_for(RELEASE_PUMP = 100 ms)` calls (`tests/shm_pool.rs:32`, used at `:104`, `:115`, `:154`) ≈ 1.6 s, plus two runtimes that serialise because both use `with_apply_env(true)` (`tests/shm_pool.rs:45`) although nothing is launched.
- `tests/clipboard.rs` 2.02 s — a single test deliberately waits out `READ_TIMEOUT = 2 s` (`read_selection_without_any_selection_times_out_bounded`, `tests/clipboard.rs:454`); the other four run in parallel (`with_apply_env(false)`) and finish well inside it.
- `tests/wayland_client.rs` 1.15 s — 7 tests each boot a runtime (~160 ms each) and all seven serialise on the process-env lock because the file uses `with_apply_env(true)` (`tests/wayland_client.rs:35`) even though nothing is launched and the client connects by absolute path.
- `tests/fixtures.rs` 0.40 s — two of four tests are env-scoped (serialised) and spawn the helper process.
`TestRuntime::start_with` spin-up is the floor (~0.1-0.15 s, inferred from the serialised `wayland_client` tests) and every integration test here that maps a window, plus every test in a consuming crate, pays it.

## Redundancy

- The "create toplevel → wait configure → apply → commit" helper is copy-pasted across four of this crate's own test binaries: `tests/wayland_client.rs:43` (`mapped`), `tests/input_capture.rs:127` (`map_toplevel`), `tests/clipboard.rs:150` (`map_peer`), `tests/shm_pool.rs:55` (`mapped`).
- `tests/clipboard.rs`'s peer harness (`Peer` `:135`, `give_input_serial` `:193`, `publish` `:220`, `teardown` `:320`) is near-1:1 duplicated by `crates/adesk-compositor/tests/clipboard.rs` (`:110`, `:307`, `:332`, `:364`); both encode the same "prove the publication was dispatched before moving focus" barrier.
- Overlap with `crates/adesk-compositor/tests`: tiling-configure/capture/destroy (`window_lifecycle.rs`), input recording (`input_delivery.rs`), selection round trip (`clipboard.rs`), popup lifecycle (`popups.rs`), `close_window` → `close_requested` (`output_composition.rs`). The compositor suites assert at the `RuntimeCommand`/`RuntimeEvent` layer, so no file is redundant — the two clipboard suites are the closest to a merge.

## Dependencies

- Workspace deps: `adesk-core`, `adesk-proto`, `adesk-wm`, `adesk-observer`, `adesk-app-registry`, `adesk-compositor`, `adesk-client`, `adesk-server`, `wayland-client 0.31`, `wayland-protocols 0.32`, `tokio`, `tempfile`, `image` (png), `thiserror`, `tracing`, `rustix 1` (feature `pipe`, clipboard pipes).
- `cargo check` does not link, so it runs outside the Nix dev shell; building/running tests needs `./scripts/dev.sh`.

## Known Issues

- Dead public items with no call site anywhere in the workspace (not exercised by any test, not pinned by the frozen specs, so removable): `FillPattern::gradient_v` (`src/fill.rs:75`), `EventAssert::wait_ordered` (`src/assert/event.rs:356`), `TestEnv::wayland_socket_path` (`src/env.rs:125`), `wait_until_async` (`src/wait.rs:42`) and `poll_until` (`src/wait.rs:69`, both still re-exported from `src/lib.rs:92`), and `WaylandTestClient::pump_until` (`src/wayland/mod.rs:631`; tests use `pump_for`/`roundtrip` instead). The `FillPattern::GradientV` variant itself is *not* dead — `from_cli_arg`/`at` still use it.
- `TestkitError::AlreadyShutdown` (`src/error.rs:30`) and `TestkitError::ImageMismatch` (`src/error.rs:65`) are never constructed anywhere (shutdown is idempotent and returns `Ok`; pixel comparisons panic while PNG writes return `TestkitError::Io`).
- `tests/fixtures.rs` prints `Io error: Broken pipe (os error 32)` on stdout while still passing — log noise from the helper-process path, not a failure.

## Notes for Agents

- **`adesk-server` coupling**: `runtime.rs` is the only file that touches `ServerConfig`/`Server::start`/`RunningServer` (contract in its module docs); adapt only `TestRuntime::start_with`/`shutdown` when that contract changes.
- **No `adesk-server` default-resolution logic is reused or copied.** The harness always passes explicit values — `TestEnv::agp_socket()` (`<root>/runtime/adesk.sock`), `env.wayland_display()`, and `RendererKind`/`Size` values held directly in `TestRuntimeConfig` (never parsed from strings) — so it never calls `adesk_server::default_socket_path()`, `parse_renderer` or `parse_size` and has no `$ADESK_SOCKET`/`$XDG_RUNTIME_DIR/adesk.sock` fallback chain of its own. `env.rs` only *sets* `ADESK_SOCKET` (propagation, not resolution) and `wayland/mod.rs::connect` resolves the Wayland socket from `$XDG_RUNTIME_DIR` (a different socket). The `adesk-test-app` helper's `--size`/`--fill`/`--exit-after` parsers are bin-local and unrelated to the server CLI grammar.
- **Image work is confined to the harness's own pixel ground truth.** `FillPattern::at` (`src/fill.rs`) generates pixels, `wayland/shm.rs` writes them into SHM buffers, and `src/assert/image.rs` compares them and is the only place that touches the `image` crate (encode-only PNG). The crate composes no frames, decodes no PNG and contains no region crop/downscale/blend math: `TestRuntime::capture` is a single-window AGP `capture_window` whose PNG payload is decoded by `adesk_client` (`decode_image`), not by testkit.
- **Consumer wiring**: the crate is declared once in the root `[workspace.dependencies]` (`Cargo.toml:31`); a consumer adds it under `[dev-dependencies]`. `adesk-agent` does, gated by its `e2e` feature (`crates/adesk-agent/Cargo.toml`), and its `tests/e2e_runtime.rs` drives a real runtime through the harness.
- **The `adesk-test-app` CLI grammar has a verbatim downstream copy.** `TestAppSpec::desktop_entry`/`TestApp::spawn` (and `helper_bin_path`) hardcode the helper name `adesk-test-app` (`src/fixtures/mod.rs:477,523,654`), which a downstream crate cannot redirect to its own example binary. `crates/adesk-agent/examples/adesk-e2e-app.rs` therefore re-implements the same CLI grammar, `CliArgs`/`parse`/`parse_size`/`parse_exit_after` and connect→create→commit handshake (~70% verbatim of that example's code body); the grammar is a contract with `TestAppSpec::cli_args` and must stay in sync. Adding an exec-path override / reusable app runner to `TestAppSpec`/`TestApp` would remove the copy.
- **Launch needs `apply_env(true)`; a `WaylandTestClient` does not.** The server reads the *process* `XDG_RUNTIME_DIR` to build a launched child's env, so AGP `launch_app` only works while the runtime's env is applied (and the runtime then holds the process-env lock for its lifetime — a second env-scoped runtime in the same test fails with `Timeout` after `PROCESS_ENV_LOCK_TIMEOUT` = 60 s). `wayland_client()` connects by absolute path, so windows can be mapped with `apply_env(false)`, which releases the lock as soon as the server is up.
- **A publication and a focus change must be ordered.** `set_selection` only flushes (`Ok(())` means "on the wire"), and `roundtrip()` is a flush plus one buffered reader-cycle notification — *not* a barrier — so a test that publishes and then awaits an AGP `activate_window` lets the two channels race; Smithay drops a `set_selection` from a client that no longer holds the keyboard focus, silently and without a `cancelled`. Prove dispatch first: a commit on the publishing connection (`commit_frame`/`commit_pending`) awaited as its `RuntimeEvent::SurfaceCommit` is the pattern `tests/clipboard.rs::publish` uses.
- **Pointer button/axis need pointer focus first.** The compositor drops `wl_pointer.button`/`axis` unless a prior move established pointer focus (Smithay's default grab); input tests always move the pointer onto the target window first.
- **Window ids are per-runtime, not per-test.** In a fresh `TestRuntime` the first mapped toplevel is `WindowId(1)` and the second `WindowId(2)` (counter in `adesk-wm`, allocated on the first buffer commit; nothing else consumes ids). Popups use a separate `u64` popup-id space, never a `WindowId`, and never appear in `list_windows` (only `WindowInfo.popup_count`); they surface as `PopupAppeared/Disappeared` events and `Observation.popups_appeared/disappeared` keyed to the owning window.
- **Capture forms.** `TestRuntime::capture(id)` returns an unscaled RGBA8 `ImageBuffer` at the window's natural size; AGP `observe` attaches a PNG `ImagePayload` inside the observation (decode with `ObserveResult::decode_image()`), and `max_dimension` downscales by longest edge with half-up rounding, never upscaling.
- `wayland/mod.rs` documents the reader-thread, lock-order and teardown rules; `wayland/input.rs` the recording rules; `wayland/clipboard.rs` the fd lifecycle; `wayland/shm.rs` the Argb8888 byte order; `wayland/state.rs` holds the core `Dispatch` impls (`wayland/clipboard.rs` adds the data-device ones beside its helpers).
- The frozen acceptance specs (`tests/api_surface.rs`, `tests/assertions.rs`, `tests/fixtures.rs`, `tests/runtime.rs`, `tests/wayland_client.rs`) must not be edited; add new behavior proof in new test files instead.
- **Harness capability gaps that push consumers to re-implement** (verified against the consumer suites): no exec-path override on `TestAppSpec` — both program-resolution sites hardcode `helper_bin_path(HELPER_APP)` (`src/fixtures/mod.rs:477` `desktop_entry`, `:523` `TestApp::spawn`) and `with_arg` only appends helper arguments, so `adesk-agent` ships a 296-line fixture example plus ~80 lines of plumbing to work around it; no raw/NDJSON AGP client (so `crates/adesk-server/tests/common/mod.rs` carries a 209-line `RawClient` and `crates/adesk-client/tests/common` a 256-line `MockServer`); no `ServerContext` accessor; no viewer configuration or `viewer_socket_path()` — every `TestRuntime` also binds an extra VAP socket because `ServerConfig::new` defaults `viewer.enabled = true`; no sync-body/`block_on` variant of `TestRuntime` (which forces the server suites' sync `common/mod.rs`); no AGP `ErrorCode` assertion helper; no `TestAppSpec::title()` accessor.
- `adesk-observer`, `adesk-inspector`, `adesk-client` and `adesk-app-registry` test suites use this crate not at all, by design: their subjects are pure or mock-driven and this crate's runtime/Wayland/image machinery does not apply. Do not try to route them through it.
- **No shared map-and-await scaffolding, no shared deadline constant.** The only public "wait for a mapped window" entry points are `TestRuntime::wait_for_window` / `wait_for_window_app` (both just `EventAssert::wait_for_expected(WindowCreated[For])`); nothing binds "create toplevel → wait_for_configure → apply_configure → commit_frame → await `WindowCreated` → await `WindowActivated`". Each consumer re-implements that sequence as a private helper (`tests/wayland_client.rs:43` `mapped`, `tests/shm_pool.rs:55` `mapped`, `tests/input_capture.rs:127` `map_toplevel`, `tests/clipboard.rs:150` `map_peer`, plus in-crate `src/wayland/input.rs:307` / `src/wayland/clipboard.rs:596` `mapped_window`). Likewise every consuming file defines its own `const DEADLINE = Duration::from_secs(10)` (the per-test-only `OFFER_PROBE`/`READ_TIMEOUT`/`RELEASE_PUMP`/`QUIET_MS` are also local); the only public timeouts are `DEFAULT_SELECTION_TIMEOUT` (10 s), `DEFAULT_SHUTDOWN_TIMEOUT` (5 s) and `DEFAULT_POLL_INTERVAL` (10 ms).
