# tests — the harness testing itself

## Intent

Integration tests that prove `adesk-testkit` works: runtime lifecycle, the real Wayland protocol path, fixtures/launch, assertions, and public-API stability.
The suite is implemented and passes under the dev shell (`cargo test -p adesk-testkit`).
Current state: the suite's source count is 62 `#[test]`/`#[tokio::test]` functions; the crate-level CONTEXT records under Known Issues that the 66-test / 49-test figures quoted elsewhere do not match that count.
No test is `#[ignore]`d; the only ignored doctests are the 3 doc-code fences in `src/assert/`.

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
| `clipboard.rs` | two-client clipboard (strict offer delivery): selection round trip, first-offer supersede, unadvertised mime, bounded no-selection timeout, no payload leakage into events |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
- The harness serializes process-env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.
- The frozen acceptance specs (`api_surface.rs`, `assertions.rs`, `fixtures.rs`, `runtime.rs`, `wayland_client.rs`) must not be edited; add new behavior proof in new test files.
- `clipboard.rs` asserts strict delivery: `REQUIRE_CLIPBOARD_DELIVERY = true`, so a missing selection offer is a hard failure; the flag-false tolerant fallback is a configured escape hatch, never a passing path in the shipped configuration.

## Known Issues

- `e2e_launch_observe.rs:159-161` claims window↔launch attribution is not implemented ("does not yet attribute windows to it, so the event carries `launch_id: None`"), but `fixtures.rs:142` asserts `launch_id == Some(launched.launch_id)` for the same `launch_app` flow, and `adesk-server`'s dispatch docs say `WindowCreated` carries `launch_id` (ledger / `event_pump::correlate_window`) — the comment is stale and the capstone omits a correlation assertion `fixtures.rs` already proves.
- `e2e_launch_observe.rs` phase 5 cannot observe `xdg_toplevel.close`: the helper ignores close and self-exits after `HELPER_LIFETIME`, so its `WindowDestroyed` wait would pass even if the compositor never sent the event; the real close-event proof is `e2e_close.rs` (both files document this split).
- `fixtures.rs:105` calls its runtime "the only env-applying runtime in this binary", but `fixtures.rs:168` also applies the env (the module docs at `fixtures.rs:10-17` correctly say two; comment-only inaccuracy).
- No test in this suite is `#[ignore]`d, `#[cfg]`-gated, GL-skipped, or has an early-return skip: `ADESK_TEST_GL` / `gl_enabled` / `require_gl` appear only inside `api_surface.rs` consistency assertions, and panics in test bodies cannot be swallowed (no test spawns tasks or threads).
