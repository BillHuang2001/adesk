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
- `actions.rs::record_action_captures_watermark_and_clock` is the only spec that runs on the real clock (plain `#[test]`, no paused runtime): its final `record.ts_ms == observer.now_ms()` equality compares two reads of the *real* monotonic clock, so a scheduling gap of ≥ 1 ms between them fails intermittently under load (`left: 0, right: 3` was observed once). The spec is frozen, so the flake is recorded in `../CONTEXT.md` (Known Issues) — do not edit the assertion.
- Build/run only through the dev-shell wrapper: `./scripts/dev.sh cargo test -p adesk-observer` (bare cargo fails to link).

## Verified Results (current HEAD)
- 125 passed / 0 failed / 0 ignored: lib unit 85, actions 6, api_surface 5, concurrency 5, filters 8, resync 5, waits 10, doctest 1.
- `cargo test -p adesk-observer -- --list` reports 125 listed tests (same per-binary split above).

## Notes for Agents
- No test anywhere in the crate uses a wall-clock sleep or a real timer.
  Every wait-bearing suite is `#[tokio::test(start_paused = true)]` and resolves through tokio's auto-advance (all tasks idle → virtual time jumps to the deadline) or an explicit `tokio::time::advance`, so per-test wall time is sub-millisecond.
  The crate's cost is compile + link, not execution.
- The only real-clock spec is `actions.rs::record_action_captures_watermark_and_clock` (plain `#[test]`, `:42`) — the flake recorded in `../CONTEXT.md` (Known Issues).
- `waits.rs` (349 lines, 10 paused tests) is the largest file here but has negligible runtime — its weight is a per-target compile cost only.
- `common/mod.rs` is `mod common;`-included by all six test binaries, so it is compiled six times (each binary uses a subset, hence its module-level `#[allow(dead_code)]`).
  Six separate integration binaries each compile+link the lib; merging targets (e.g. the two all-sync suites `actions.rs` + `api_surface.rs`) would cut link steps.
- Fixture builders are duplicated across Rust's test-module boundary and cannot be shared without exposing a `pub` test-support module: `rect`/`region` exist in `./common/mod.rs:10,15`, `../src/service.rs:853,857` and `../src/journal.rs:258,262`; the `RuntimeEvent` builders (`created`/`destroyed`/`commit`/`title_changed`/`popup_appeared`/`app_launched`) exist in both `./common/mod.rs` and the `../src/service.rs` inline module; `CountedEvent` builders `event` (`../src/waiter.rs:321`) and `counted` (`../src/journal.rs:271`) are twins.
- ~27 of the 39 integration tests have a near 1:1 inline twin in `../src/service.rs` or `../src/waiter.rs`/`../src/actions.rs`; four pairs share the exact name: `resync_ignores_stale_snapshots` (`resync.rs:223` ↔ `../src/service.rs:1093`), `wait_for_quiet_rearms_on_every_commit` (`waits.rs:166` ↔ `../src/service.rs:1629`), `dropped_waiter_clears_pending_observation` (`concurrency.rs:105` ↔ `../src/service.rs:1715`), `record_action_captures_watermark_and_clock` (`actions.rs:42` ↔ `../src/service.rs:1230`).
  The integration copies are the frozen, authoritative specs; the inline copies are the modifiable ones.
- The `Box::pin(wait)` + `select! { biased; …; yield_now }` "park the waiter" idiom is duplicated ~19× (8 in `waits.rs`, 3 in `concurrency.rs`, 8 in `../src/service.rs`); `filters.rs` instead uses `tokio::spawn` + a single `yield_now` (6×) and `resync.rs` already factors it into the `change_waiter_in_flight` helper (`:16`).

## Notes for Agents
- No test anywhere in the crate uses a wall-clock sleep or a real timer.
  Every wait-bearing suite is `#[tokio::test(start_paused = true)]` and resolves through tokio's auto-advance (all tasks idle → virtual time jumps to the deadline) or an explicit `tokio::time::advance`, so per-test wall time is sub-millisecond.
  The crate's cost is compile + link, not execution.
- The only real-clock spec is `actions.rs::record_action_captures_watermark_and_clock` (plain `#[test]`, `:42`) — the flake recorded in `../CONTEXT.md` (Known Issues).
- `waits.rs` (349 lines, 10 paused tests) is the largest file here but has negligible runtime — its weight is a per-target compile cost only.
- `common/mod.rs` is `mod common;`-included by all six test binaries, so it is compiled six times (each binary uses a subset, hence its module-level `#[allow(dead_code)]`).
  Six separate integration binaries each compile+link the lib; merging targets (e.g. the two all-sync suites `actions.rs` + `api_surface.rs`) would cut link steps.
- Fixture builders are duplicated across Rust's test-module boundary and cannot be shared without exposing a `pub` test-support module: `rect`/`region` exist in `./common/mod.rs:10,15`, `../src/service.rs:853,857` and `../src/journal.rs:258,262`; the `RuntimeEvent` builders (`created`/`destroyed`/`commit`/`title_changed`/`popup_appeared`/`app_launched`) exist in both `./common/mod.rs` and the `../src/service.rs` inline module; `CountedEvent` builders `event` (`../src/waiter.rs:321`) and `counted` (`../src/journal.rs:271`) are twins.
- ~27 of the 39 integration tests have a near 1:1 inline twin in `../src/service.rs` or `../src/waiter.rs`/`../src/actions.rs`; four pairs share the exact name: `resync_ignores_stale_snapshots` (`resync.rs:223` ↔ `../src/service.rs:1093`), `wait_for_quiet_rearms_on_every_commit` (`waits.rs:166` ↔ `../src/service.rs:1629`), `dropped_waiter_clears_pending_observation` (`concurrency.rs:105` ↔ `../src/service.rs:1715`), `record_action_captures_watermark_and_clock` (`actions.rs:42` ↔ `../src/service.rs:1230`).
  The integration copies are the frozen, authoritative specs; the inline copies are the modifiable ones.
- The `Box::pin(wait)` + `select! { biased; …; yield_now }` "park the waiter" idiom is duplicated ~19× (8 in `waits.rs`, 3 in `concurrency.rs`, 8 in `../src/service.rs`); `filters.rs` instead uses `tokio::spawn` + a single `yield_now` (6×) and `resync.rs` already factors it into the `change_waiter_in_flight` helper (`:16`).

## Routing Table
| Area | Owner |
|---|---|
| Crate source (service, waiter, journal, clock, specs) | `../src/` (parent level) |
| Shared event fixtures | `./common/mod.rs` |
