# tests — the harness testing itself

## Intent

Integration tests that prove `adesk-testkit` works: runtime lifecycle, the real Wayland protocol path, fixtures/launch, assertions, and public-API stability.
The suite is implemented and passes under the dev shell (`cargo test -p adesk-testkit`).

## API Surface

| File | Covers |
|---|---|
| `runtime.rs` | start/stop, `ping`, pixman default, tiled rect, drop |
| `wayland_client.rs` | toplevel in `list_windows`, tiling configure, `SurfaceCommit`, captured pixels match the fill, popups, resize |
| `fixtures.rs` | `.desktop` writing/listing, registry launch path, helper process |
| `assertions.rs` | `ImageAssert`/`EventAssert`/`wait_until` self-checks |
| `api_surface.rs` | signature stability, protocol version, GL gating |
| `e2e_launch_observe.rs` | capstone: fixture → runtime → launch → capture → close → input → temporal observation |
| `e2e_close.rs` | `close_window` really sends `xdg_toplevel.close` (proved via `TestWindow::close_requested`) |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
- The harness serializes process-env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.
- The frozen acceptance specs (`api_surface.rs`, `assertions.rs`, `fixtures.rs`, `runtime.rs`, `wayland_client.rs`) must not be edited; add new behavior proof in new test files.

## Known Issues

- The module-level doc comments in the frozen specs still describe the Phase-1 skeleton ("bodies are `todo!()`", "expected to fail at runtime until Phase 2"), and `wayland_client.rs` still claims `--test-threads=1` is required. Both are stale — the specs must not be edited, so ignore the comments.
