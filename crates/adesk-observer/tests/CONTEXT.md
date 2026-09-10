# adesk-observer/tests — integration & frozen-spec suites

## Intent
Protocol-level scenario tests for the observer crate, plus a public-API smoke suite.
All are deterministic, paused-time tests: no display, GPU, network, filesystem or installed apps.
Assertions are frozen — behaviour must be argued against `docs/protocol.md`, never by editing a spec.
Current state: all suites implemented, green, none ignored.

## API Surface (test binaries)
- `waits.rs` (10): `wait_for_change` / `wait_for_quiet` / `observe` resolution, timeouts, quiet re-arming, event-clock `elapsed_ms`.
- `filters.rs` (8): window / `after_action` / `since_commit` filters, unknown-window/action errors, damage union/clipping, title/focus/popup flags.
- `concurrency.rs` (5): concurrent waiters, `pending_observation` visibility, dropped-waiter cleanup, wakeup without time advancing.
- `resync.rs` (5): watermark advance, stale-snapshot ignore, `state_uncertain` marking, synthetic commit/destroyed events, removed windows.
- `actions.rs` (6): registry monotonic ids from 1, clone-shared id space, action record capture, `last_input_at` updates.
- `api_surface.rs` (5): public API shape (builders, enum mappings, snapshot structs) without running a service.
- `common/mod.rs`: shared `RuntimeEvent` fixtures; each test binary uses a subset, hence its module-level `#[allow(dead_code)]`.

## Constraints
- Time moves only through the paused tokio clock: an explicit `tokio::time::advance`, or auto-advance to a parked waiter's deadline once every task is idle; events carry explicit `seq`/`ts_ms`.
- Never un-ignore or relax a failing spec; a wrong implementation makes paused-time waits hang forever.
- Build/run only through the dev-shell wrapper: `./scripts/dev.sh cargo test -p adesk-observer` (bare cargo fails to link).

## Verified Results (current HEAD)
- 125 passed / 0 failed / 0 ignored: lib unit 85, actions 6, api_surface 5, concurrency 5, filters 8, resync 5, waits 10, doctest 1.
- `cargo test -p adesk-observer -- --list` reports 125 listed tests (same per-binary split above).

## Routing Table
| Area | Owner |
|---|---|
| Crate source (service, waiter, journal, clock, specs) | `../src/` (parent level) |
| Shared event fixtures | `./common/mod.rs` |
