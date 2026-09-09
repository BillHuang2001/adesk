# tests — the harness testing itself

## Intent

Integration tests that prove `adesk-testkit` works: runtime lifecycle, the real Wayland protocol path, fixtures/launch, assertions, and public-API stability.
Phase 1 requires the suite to compile; Phase 2 must make it pass.

## API Surface

| File | Covers |
|---|---|
| `runtime.rs` | start/stop, `ping`, pixman default, tiled rect, drop |
| `wayland_client.rs` | toplevel in `list_windows`, tiling configure, `SurfaceCommit`, captured pixels match the fill, popups, resize |
| `fixtures.rs` | `.desktop` writing/listing, registry launch path, helper process |
| `assertions.rs` | `ImageAssert`/`EventAssert`/`wait_until` self-checks |
| `api_surface.rs` | signature stability, protocol version, GL gating |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Tests that depend on the process env (Wayland binding via `XDG_RUNTIME_DIR`, launch) must run with `--test-threads=1`.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
