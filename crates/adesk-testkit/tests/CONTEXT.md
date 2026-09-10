# tests — the harness testing itself

## Intent

Integration tests that prove `adesk-testkit` works: runtime lifecycle, the real Wayland protocol path, fixtures/launch, assertions, and public-API stability.
The suite is implemented and passes under the dev shell (`cargo test -p adesk-testkit`).
Current state: `cargo test -p adesk-testkit --all-features` passes 66 tests + 3 doctests, 0 failures (3 ignored doc-code fences).

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
| `input_capture.rs` | real-seat input capture: pointer enter/motion coordinates from the tiled geometry, button press/release order, vertical axis + frame, ctrl+c chord press/reverse-release, keyboard/pointer enter and leave transitions |
| `clipboard.rs` | two-client clipboard: selection round trip, first-offer supersede, unadvertised mime, bounded no-selection timeout, no payload leakage into events |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
- The harness serializes process-env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.
- The frozen acceptance specs (`api_surface.rs`, `assertions.rs`, `fixtures.rs`, `runtime.rs`, `wayland_client.rs`) must not be edited; add new behavior proof in new test files.
- `clipboard.rs` is strict/tolerant gated by `REQUIRE_CLIPBOARD_DELIVERY` (today `false`): the strict round-trip assertions engage automatically when selection offers arrive; until adesk-compositor wires data-device focus, the tolerant branch asserts the documented bounded no-offer `Timeout` instead.
