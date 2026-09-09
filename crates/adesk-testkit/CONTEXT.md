# adesk-testkit — dev-only headless test harness

## Intent

`adesk-testkit` makes the whole ADesk runtime testable without a display, GPU, network or installed application.
It is the only supported way to run ADesk end-to-end tests: an in-process runtime on private temp paths, a real Wayland protocol client, `.desktop` fixtures, a helper process, and deadline-bounded image/event assertions.
**Dev-dependency target only** — no runtime crate may depend on it, and the `adesk-test-app` binary is never shipped.
Status: Phase-1 architecture skeleton — the public API and its documentation are final; behavior bodies are `todo!()` and are implemented in Phase 2.

## API Surface

- `TestRuntime` / `TestRuntimeConfig` — start a real compositor thread + AGP server in-process (pixman, `1280x800` default) on a temp `XDG_RUNTIME_DIR` and temp socket; accessors `socket_path()`, `wayland_display()`, `output_size()`, `tiled_rect()`, `compositor()`, `observer()`, `registry()`, `env()`, `config()`, `client()`, `event_tap()`, `wayland_client()`, `capture()`, `wait_for_window()`, `wait_for_window_app()`, `shutdown()`; `Drop` is non-blocking and never hangs.
- `expected_window_geometry(Size) -> Rect` — the tiled rect, single-sourced from `adesk-wm`'s policy; tests must never hard-code output size.
- `WaylandTestClient` / `ToplevelSpec` / `TestWindow` / `ConfiguredSize` / `PopupSpec` / `TestPopup` / `PumpStats` / `Globals` — a `wayland-client`-based client that speaks the real protocol path (SHM buffers, xdg-shell toplevels/popups), commits known fills and exposes deadline-bounded event pumps.
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
| Standalone validation harness (stubs unlanded siblings) | `check-standalone.sh` |

## Design Decisions

- **Reader-thread Wayland client.** `wayland-client` is synchronous while the runtime is tokio; a reader thread owns the `EventQueue` and dispatches into `Arc<Mutex<ClientState>>`, feeding a `tokio::sync::mpsc` channel. Public pumps await that channel with a deadline; teardown uses `UnixStream::shutdown` plus a bounded join, so nothing can block forever.
- **Absolute-path connect.** `WaylandTestClient::connect_in(runtime_dir, display)` takes the socket path explicitly because the process env is global and races under parallel tests; `connect(display_name)` (env-based) exists for external clients.
- **One pixel ground truth.** `FillPattern::at(x, y, size)` is evaluated both by the SHM writer and by `ImageAssert::matches_pattern`, so the client and the assertion can never disagree. Only opaque patterns are accepted (`require_opaque`), committed as Argb8888.
- **No `libc`.** SHM is a `tempfile`-backed anonymous file written through `FileExt::write_all_at`; no `mmap` and no `unsafe`.
- **`Drop` never blocks.** `TestRuntime::drop` sends `RuntimeCommand::Shutdown` synchronously and detached-spawns the graceful server shutdown inside the current tokio runtime; only `shutdown().await` observes errors or applies `shutdown_timeout`.
- **Process env is explicit.** `TestRuntimeConfig::apply_env` (default on) scopes the env for the runtime's lifetime because the app registry launches children with `LaunchEnv::from_process()`; the hazard for parallel tests is documented in `src/env.rs`.
- **GL is opt-in and fails loudly.** `test_renderer()` returns `RendererKind::Gl` when `ADESK_TEST_GL=1`, so a broken GL setup fails instead of silently falling back to pixman.
- **Assertions panic, plumbing returns `Result`.** `ImageAssert`/`EventAssert` are assertions (`assert_eq!` semantics with pixel/event detail); everything else returns `TestkitError`.
- **`Timeout.what` is `&'static str`.** `Expected`-based event waits cannot supply a borrowed description, so they document a bounded `Box::leak` of the describe string (test-process bounded); revisit if `Timeout.what` ever becomes a `String`.
- **Tiling is never hard-coded.** `expected_window_geometry` delegates to `adesk_wm::PolicyConfig::tiled_rect()`.

## Test Strategy

- Self-tests live in `tests/`: `runtime.rs` (start/stop, ping, renderer, drop), `wayland_client.rs` (toplevel appears in `list_windows` with the tiling configure, commit → `SurfaceCommit`, captured pixels match the fill, popups), `fixtures.rs` (`.desktop` writing, launch path, helper process), `assertions.rs` (ImageAssert/EventAssert/`wait_until` self-checks), `api_surface.rs` (signature stability).
- Unit tests inside `src/assert/`, `src/fixtures/` cover the already-implemented plumbing.
- Phase-1 gate: `bash crates/adesk-testkit/check-standalone.sh` must exit 0 (compiles all targets). Phase 2 must make the integration tests pass.
- No test needs a display, GPU, network or installed app; the helper binary is built by cargo (`env!("CARGO_BIN_EXE_adesk-test-app")` is available to this package's integration tests).
- Launch tests mutate the process env: keep them in one test function or run that test binary with `--test-threads=1`.

## Dependencies

- Workspace deps: `adesk-core`, `adesk-proto`, `adesk-wm`, `adesk-observer`, `adesk-app-registry`, `adesk-compositor`, `adesk-client`, `adesk-server`, `wayland-client 0.31`, `wayland-protocols 0.32`, `tokio`, `tempfile`, `image` (png), `thiserror`, `tracing`.
- `cargo check` does not link, so it runs outside the Nix dev shell; building/running tests needs `./scripts/dev.sh`.

## Known Issues

- `crates/adesk-server/` is not landed, so the root workspace (`members = ["crates/*"]`) does not load and `cargo check -p adesk-testkit` fails before compiling anything. Use `bash crates/adesk-testkit/check-standalone.sh`; delete it once every sibling has a manifest.
- Sibling-API mismatches found while building the harness (report, do not patch in-tree): `adesk-client` does not compile against landed `adesk-proto` (`src/wire.rs` imports `adesk_proto::{Request, Response}` and calls `Codec::new()`, while proto exposes `RequestFrame`/`ResponseFrame` and `Codec` as a trait); `adesk-compositor` is stale against landed `adesk-wm` (`WindowManager::new` now takes `PolicyConfig`, `resolve_position` takes `Position` by value and returns `Option<Point>`).
- `adesk_app_registry::{desktop_file_id, is_desktop_file}` and `FillPattern::{to_cli_arg, from_cli_arg}` are still `todo!()` siblings; fixture ids and the helper CLI therefore have documented Phase-2 dependencies.

## Notes for Agents

- **`adesk-server` contract designed against** (see `src/runtime.rs` module docs): `ServerConfig::new(socket_path, CompositorConfig)`, `ServerConfig::with_app_dirs(Vec<PathBuf>)`, `Server::start(ServerConfig).await -> Result<RunningServer, ServerError>`, `RunningServer::{socket_path, compositor, observer, registry}()`, `RunningServer::shutdown(self).await -> Result<(), ServerError>`; `registry()` may return `&Arc<AppRegistry>` (deref-coerces). If the landed server differs, adapt only `TestRuntime::start_with`/`shutdown`.
- `check-standalone.sh` embeds a stub `adesk-server` and a stub `adesk-client` (signatures copied from the real client) and applies two documented temp-workspace patches to `adesk-compositor/src/wm.rs`. It is validation-only; never copy those stubs into the tree.
- `runtime.rs` is the only file coupled to `adesk-server`; everything else is independent of it.
- `wayland/mod.rs` documents the reader-thread, lock-order and teardown rules; `wayland/shm.rs` documents the Argb8888 byte order; `wayland/state.rs` holds every `Dispatch` impl.
