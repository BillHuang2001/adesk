# tests — the harness testing itself

## Intent

Integration tests that prove `adesk-testkit` works: runtime lifecycle, the real Wayland protocol path, fixtures/launch, assertions, the additive harness API and public-API stability.
Run them with `./scripts/dev.sh cargo test -p adesk-testkit` (the crate declares no Cargo `[features]`, so `--all-features` is a no-op); none of them needs a display, GPU, network or installed application, and every wait is deadline-bounded.
Measured: 77 passed / 0 failed / 3 ignored (the 3 ignored are doc-code fences).

## API Surface

| File | Covers |
|---|---|
| `runtime.rs` | start/stop, `ping`, pixman default, tiled rect, drop |
| `wayland_client.rs` | toplevel in `list_windows`, tiling configure, `SurfaceCommit`, captured pixels match the fill, popups, resize |
| `fixtures.rs` | `.desktop` writing/listing, registry launch path, helper process |
| `assertions.rs` | `ImageAssert`/`EventAssert`/`wait_until` self-checks |
| `api_surface.rs` | signature stability, protocol version, GL gating |
| `harness_additions.rs` | additive harness API: `TestAppSpec::with_exec`/`exec`/`title`, `TestRuntimeConfig::with_agp_socket`, `with_viewer(false)` |
| `e2e_launch_observe.rs` | capstone: fixture → runtime → launch → capture → close → input → temporal observation; the close leg is a real `xdg_toplevel.close` proof (helper launched `--exit-on-close`, no `--exit-after`), also asserting the window is gone from `list_windows` and `get_window` errors |
| `input_capture.rs` | real-seat input capture: pointer enter/motion coordinates from the tiled geometry, button press/release order, vertical axis + frame, ctrl+c chord press/reverse-release, keyboard/pointer enter and leave transitions |
| `shm_pool.rs` | SHM pool release regression: ten fresh frames in a row reuse released ranges (capture still shows the last fill), and six windows destroyed in turn each get their final buffer's range back — both exhaust the 16 MiB pool unless `wl_buffer.release` is attributed by buffer object id; the release is awaited event-driven (`wait_for_release` via `pump_until`, preceded by `drain_notifications`), never a fixed sleep |
| `clipboard.rs` | two-client clipboard (strict offer delivery): selection round trip, first-offer supersede, unadvertised mime, bounded no-selection timeout, no payload leakage into events, and the publication→focus-change ordering barrier |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
- The harness serializes process-env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required; binaries that launch nothing use `with_apply_env(false)` and do not take the lock (`wayland_client.rs`, `shm_pool.rs`, `clipboard.rs`, `input_capture.rs`).
- The frozen acceptance specs (`api_surface.rs`, `assertions.rs`, `fixtures.rs`, `runtime.rs`, `wayland_client.rs`) must not be edited; add new behavior proof in new test files.
- `clipboard.rs` asserts strict delivery: `REQUIRE_CLIPBOARD_DELIVERY = true`, so a missing selection offer is a hard failure; the flag-false tolerant fallback is a configured escape hatch, never a passing path in the shipped configuration.
- `clipboard.rs` publishes only through its `publish` helper, which proves the compositor dispatched the `set_selection` before the test moves the focus: a commit on the publishing connection awaited as its `RuntimeEvent::SurfaceCommit`. `roundtrip()` is a flush plus one reader-cycle notification, never a barrier, and a publication dispatched after the focus moved is dropped by Smithay silently.
- `clipboard.rs`'s no-offer reads use a short per-test deadline (`NO_OFFER_TIMEOUT = 250 ms`) so the suite stays fast; genuine transfers use `READ_TIMEOUT = 2 s`.

## Known Issues

- None beyond the crate-level `tests/fixtures.rs` stdout noise (`Io error: Broken pipe`) documented in `../CONTEXT.md`.
