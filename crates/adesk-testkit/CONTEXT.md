# adesk-testkit — dev-only headless test harness

## Intent

`adesk-testkit` makes the whole ADesk runtime testable without a display, GPU, network or installed application.
It is the only supported way to run ADesk end-to-end tests: an in-process runtime on private temp paths, a real Wayland protocol client, `.desktop` fixtures, a helper process, and deadline-bounded image/event assertions.
**Dev-dependency target only** — no runtime crate may depend on it, and the `adesk-test-app` binary is never shipped.
Status: implemented — `src/` and `tests/` contain no `todo!()`/`unimplemented!()` bodies, and the full `cargo test -p adesk-testkit` suite passes under the dev shell (46 tests + 3 doctests, 0 failures, 0 ignored test functions).

## API Surface

- `TestRuntime` / `TestRuntimeConfig` — start a real compositor thread + AGP server in-process (pixman, `1280x800` default) on a temp `XDG_RUNTIME_DIR` and temp socket; accessors `socket_path()`, `wayland_display()`, `output_size()`, `tiled_rect()`, `compositor()`, `observer()`, `registry()`, `env()`, `config()`, `client()`, `event_tap()`, `wayland_client()`, `capture()`, `wait_for_window()`, `wait_for_window_app()`, `shutdown()`; `Drop` is non-blocking and never hangs.
- `expected_window_geometry(Size) -> Rect` — the tiled rect, single-sourced from `adesk-wm`'s policy; tests must never hard-code output size.
- `WaylandTestClient` / `ToplevelSpec` / `TestWindow` / `ConfiguredSize` / `PopupSpec` / `TestPopup` / `PumpStats` / `Globals` — a `wayland-client`-based client that speaks the real protocol path (SHM buffers, xdg-shell toplevels/popups), commits known fills and exposes deadline-bounded event pumps. `TestWindow::close_requested()` reports a compositor `xdg_toplevel.close` while the surface is still alive; `TestWindow` also exposes `app_id()`, `title()`, `size()`, `fill()`, `damage_hint()`, `is_destroyed()`, `pending_configure()`, `last_configure()`.
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
- **Absolute-path connect.** `WaylandTestClient::connect_in(runtime_dir, display)` takes the socket path explicitly because the process env is global and races under parallel tests; `connect(display_name)` (env-based) exists for external clients.
- **One pixel ground truth.** `FillPattern::at(x, y, size)` is evaluated both by the SHM writer and by `ImageAssert::matches_pattern`, so the client and the assertion can never disagree. Only opaque patterns are accepted (`require_opaque`), committed as Argb8888.
- **No `libc`.** SHM is a `tempfile`-backed anonymous file written through `FileExt::write_all_at`; no `mmap` and no `unsafe`.
- **`Drop` never blocks.** `TestRuntime::drop` sends `RuntimeCommand::Shutdown` synchronously and detached-spawns the graceful server shutdown inside the current tokio runtime; only `shutdown().await` observes errors or applies `shutdown_timeout`.
- **Process env is explicit.** `TestRuntimeConfig::apply_env` (default on) scopes the env for the runtime's lifetime because the app registry launches children with `LaunchEnv::from_process()`; the hazard for parallel tests is documented in `src/env.rs`.
- **Fixtures speak the registry's language.** Share roots (`with_fixture_dir`/`with_app_dirs`) are translated to `<root>/applications` before the server sees them because the registry derives app ids relative to each configured dir, and `Exec` arguments are quoted iff empty or containing ASCII whitespace/`"`/`\` so `adesk_app_registry`'s tokenizer returns them unchanged; both rules live in `src/runtime.rs`/`src/fixtures/mod.rs` module docs.
- **Close is observed, never assumed.** The reader records `xdg_toplevel.close` in the window slot and the client keeps the surface alive, so `TestWindow::close_requested()` lets `tests/e2e_close.rs` prove the AGP `close_window` request path instead of inferring it from the window's disappearance.
- **GL is opt-in and fails loudly.** `test_renderer()` returns `RendererKind::Gl` when `ADESK_TEST_GL=1`, so a broken GL setup fails instead of silently falling back to pixman.
- **Assertions panic, plumbing returns `Result`.** `ImageAssert`/`EventAssert` are assertions (`assert_eq!` semantics with pixel/event detail); everything else returns `TestkitError`.
- **`Timeout.what` is `&'static str`.** `Expected`-based event waits cannot supply a borrowed description, so they document a bounded `Box::leak` of the describe string (test-process bounded); revisit if `Timeout.what` ever becomes a `String`.
- **Tiling is never hard-coded.** `expected_window_geometry` delegates to `adesk_wm::PolicyConfig::tiled_rect()`.

## Test Strategy

- Self-tests live in `tests/`: `runtime.rs` (start/stop, ping, renderer, drop), `wayland_client.rs` (toplevel appears in `list_windows` with the tiling configure, commit → `SurfaceCommit`, captured pixels match the fill, popups, resize), `fixtures.rs` (`.desktop` writing, launch path, helper process), `assertions.rs` (ImageAssert/EventAssert/`wait_until` self-checks), `api_surface.rs` (signature stability), `e2e_launch_observe.rs` (capstone: launch → observe → capture → input → close round trip) and `e2e_close.rs` (cooperating-client proof that `close_window` really sends `xdg_toplevel.close`).
- Unit tests inside `src/assert/`, `src/fixtures/` and `src/bin/adesk-test-app.rs` cover the already-implemented plumbing.
- Run with `./scripts/dev.sh cargo test -p adesk-testkit`; the full suite passes (46 tests + 3 doctests, 0 failures) and no test is `#[ignore]`d or env-gated except the GL paths (which skip cleanly without `ADESK_TEST_GL=1`).
- No test needs a display, GPU, network or installed app; the helper binary is built by cargo (`env!("CARGO_BIN_EXE_adesk-test-app")` is available to this package's integration tests).
- Launch tests mutate the process env; the harness serializes env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.

## Dependencies

- Workspace deps: `adesk-core`, `adesk-proto`, `adesk-wm`, `adesk-observer`, `adesk-app-registry`, `adesk-compositor`, `adesk-client`, `adesk-server`, `wayland-client 0.31`, `wayland-protocols 0.32`, `tokio`, `tempfile`, `image` (png), `thiserror`, `tracing`.
- `cargo check` does not link, so it runs outside the Nix dev shell; building/running tests needs `./scripts/dev.sh`.

## Known Issues

- The module-level doc comments in the frozen acceptance specs (`tests/wayland_client.rs`, `tests/assertions.rs`, `tests/fixtures.rs`, `tests/runtime.rs`, `tests/api_surface.rs`) still describe the Phase-1 skeleton ("bodies are `todo!()`", "expected to fail at runtime until Phase 2"); `tests/wayland_client.rs` also still claims `--test-threads=1` is required. Both are stale — the specs must not be edited, so ignore the comments.

## Notes for Agents

- **`adesk-server` coupling**: `runtime.rs` is the only file that touches `ServerConfig`/`Server::start`/`RunningServer` (contract in its module docs); adapt only `TestRuntime::start_with`/`shutdown` when that contract changes.
- `wayland/mod.rs` documents the reader-thread, lock-order and teardown rules; `wayland/shm.rs` documents the Argb8888 byte order; `wayland/state.rs` holds every `Dispatch` impl.
- The frozen acceptance specs (`tests/api_surface.rs`, `tests/assertions.rs`, `tests/fixtures.rs`, `tests/runtime.rs`, `tests/wayland_client.rs`) must not be edited; add new behavior proof in new test files instead.
