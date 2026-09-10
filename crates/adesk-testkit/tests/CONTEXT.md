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
| `clipboard.rs` | two-client clipboard (strict offer delivery): selection round trip, first-offer supersede, unadvertised mime, bounded no-selection timeout, no payload leakage into events |

## Constraints

- No display, GPU, network or installed application; every wait is deadline-bounded.
- Use only the public `adesk_testkit` API; `pub(crate)` internals are invisible here.
- Never hard-code the output size: use `expected_window_geometry` / `TestRuntime::tiled_rect()`.
- The harness serializes process-env-scoped runtimes in one test binary itself, so no `--test-threads=1` is required.
- The frozen acceptance specs (`api_surface.rs`, `assertions.rs`, `fixtures.rs`, `runtime.rs`, `wayland_client.rs`) must not be edited; add new behavior proof in new test files.
- `clipboard.rs` asserts strict delivery: `REQUIRE_CLIPBOARD_DELIVERY = true`, so a missing selection offer is a hard failure; the flag-false tolerant fallback is a configured escape hatch, never a passing path in the shipped configuration.

## Known Issues

- Clipboard tests can lose a publication under cold parallel load: `set_selection` only flushes, and the following awaited AGP `activate_window` travels a different channel, so the compositor may apply the focus change before it dispatches the Wayland request; Smithay then silently denies `set_selection` from a client without keyboard focus (no error, no `cancelled`).
- `clipboard.rs::second_set_selection_invalidates_the_first_offer` has no barrier between publication and the focus change at either publication (`set_selection` at lines 313/331, `activate_window(reader)` at lines 314/333); `crates/adesk-compositor/tests/clipboard.rs::second_set_selection_supersedes_the_first_offer` has the same shape, where its `publish` helper wrongly documents `roundtrip()` as a sync-callback barrier.
- `roundtrip()` is not a barrier — flush plus one buffered reader-cycle notification — so `set_selection` + `roundtrip()` does not close the race either.
- Failure signatures: a lost first publication leaves the reader's `selection_offer_count` at 0, so `selection_delivery` panics after its `wait_for_selection_offer(1, OFFER_PROBE)` timeout; a lost replacement makes `wait_for_selection_offer(2, OFFER_PROBE)` time out or the payload assertion read the re-announced old selection.
- Fix pattern: before moving focus off the publisher, prove the publication was dispatched/accepted — a commit on the publisher's own connection (`commit_frame`/`commit_pending`) awaited as its `RuntimeEvent::SurfaceCommit` through `EventAssert` (as in `set_selection_is_accepted_on_a_focused_client_and_leaks_no_payload`), or the publisher's own same-client echo `wait_for_selection_offer` (unambiguous only for the first publication).
