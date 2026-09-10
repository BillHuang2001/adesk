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
- `resync.rs` (5): watermark advance, stale-snapshot ignore, `state_uncertain` marking, synthetic commit/destroyed events, removed windows; shares the `change_waiter_in_flight` helper (`:16`).
- `actions.rs` (6): registry monotonic ids from 1, clone-shared id space, action record capture, `last_input_at` updates.
- `api_surface.rs` (5): public API shape (builders, enum mappings, snapshot structs) without running a service.
- `common/mod.rs`: shared `RuntimeEvent` fixtures; each test binary uses a subset, hence its module-level `#[allow(dead_code)]`.

## Constraints
- Time moves only through the paused tokio clock: an explicit `tokio::time::advance`, or auto-advance to a parked waiter's deadline once every task is idle; events carry explicit `seq`/`ts_ms`.
- Never un-ignore or relax a failing spec; a wrong implementation makes paused-time waits hang forever.
- `actions.rs::record_action_captures_watermark_and_clock` runs on the real clock (plain `#[test]`), so it bounds the two clock reads (`before <= record.ts_ms <= after`) instead of asserting an exact value; the exact capture is proved deterministically by its inline twin in `../src/service.rs` under `start_paused`.
- Build/run only through the dev-shell wrapper: `./scripts/dev.sh cargo test -p adesk-observer` (bare cargo fails to link).

## Verified Results (current HEAD)
- 102 passed / 0 failed / 0 ignored: lib unit 62, actions 6, api_surface 5, concurrency 5, filters 8, resync 5, waits 10, doctest 1.
- `cargo test -p adesk-observer -- --list` reports the same per-binary split.

## Notes for Agents
- No test anywhere in the crate uses a wall-clock sleep or a real timer.
  Every wait-bearing suite is `#[tokio::test(start_paused = true)]` and resolves through tokio's auto-advance (all tasks idle → virtual time jumps to the deadline) or an explicit `tokio::time::advance`, so per-test wall time is sub-millisecond; the crate's cost is compile + link, not execution.
- The only real-clock spec is `actions.rs::record_action_captures_watermark_and_clock` (plain `#[test]`, `:42`); it uses the bounded `before <= record.ts_ms <= after` form, so no scheduling gap can flake it.
- `waits.rs` (349 lines, 10 paused tests) is the largest file here but has negligible runtime — its weight is a per-target compile cost only.
- `common/mod.rs` is `mod common;`-included by all six test binaries, so it is compiled six times (each binary uses a subset, hence its module-level `#[allow(dead_code)]`).
  Six separate integration binaries each compile+link the lib; merging targets (e.g. the two all-sync suites `actions.rs` + `api_surface.rs`) would cut link steps.
- Fixture builders are duplicated across Rust's test-module boundary and cannot be shared without exposing a `pub` test-support module: `rect`/`region` exist in `./common/mod.rs:10,15`, `../src/service.rs:825,829` and `../src/journal.rs:273,277`; the `RuntimeEvent` builders (`created`/`destroyed`/`commit`/`title_changed`/`app_launched`) exist in both `./common/mod.rs` and the `../src/service.rs` inline module; `CountedEvent` builders `event` (`../src/waiter.rs:348`) and `counted` (`../src/journal.rs:286`) are twins.
- The inline `../src/service.rs` suite now owns the state machine, resync synthesis, guard bookkeeping and a few wait scenarios; the frozen integration specs are the authoritative protocol acceptance layer. Exactly one pair shares an exact name: `record_action_captures_watermark_and_clock` (`actions.rs:42` ↔ `../src/service.rs:1188`) — the frozen copy bounds the real-clock reads, the inline copy pins the clock with `start_paused`.
- The `Box::pin(wait)` + `select! { biased; …; yield_now }` "park the waiter" idiom is duplicated ~16× (8 in `waits.rs`, 3 in `concurrency.rs`, 5 in `../src/service.rs`); `filters.rs` instead uses `tokio::spawn` + a single `yield_now` (6×) and `resync.rs` factors it into the `change_waiter_in_flight` helper (`:16`).

## Routing Table
| Area | Owner |
|---|---|
| Crate source (service, waiter, journal, clock, specs) | `../src/` (parent level) |
| Shared event fixtures | `./common/mod.rs` |
