# adesk-observer — temporal observation engine
## Intent
The observer is the runtime's memory of *what happened when*: it consumes the compositor's `RuntimeEvent` stream and answers the long waits of `docs/protocol.md` §5.4 (`observe`, `wait_for_change`, `wait_for_quiet`).
It is a pure, dependency-light consumer: no Smithay, no compositor state, no rendering, no filesystem/socket I/O.
`docs/architecture.md` §6 is the binding contract; `docs/core-api.md` fixes the types it must use (`RuntimeEvent`, `Observation`, `ActionId`, `WindowId`, `Region`) — never fork them.
Central semantics: *surface activity is known, semantic completion is inferred* — quiet is evidence the agent reasons about, never a promise.
## API Surface
Everything is re-exported flat at the crate root; `adesk_observer::<Name>`.
### `ObserverService` (`src/service.rs`) — cheap `Clone` handle over `Arc<Inner>`, `Send + Sync + 'static`
- `new()`, `with_config(ObserverConfig)`, `Default`.
- Event pump: `handle_event(&RuntimeEvent)` (sync, cheap, called per broadcast event in `seq` order), `resync(StateSnapshot) -> ResyncReport` (broadcast-lag recovery).
- Actions: `record_action(kind, Option<WindowId>, Option<Position>) -> ActionId`, `action(ActionId) -> Option<ActionRecord>`, `action_seq(ActionId) -> Option<u64>`, `action_registry() -> ActionRegistry`.
- Waits (async): `wait_for_change(WaitSpec)`, `wait_for_quiet(QuietSpec)`, `observe(ObserveSpec)`, all `-> Result<Observation>`.
- Queries (sync, never wait): `snapshot() -> ObserverSnapshot`, `window_state(WindowId) -> Option<WindowTemporalState>`, `window_ids() -> Vec<WindowId>`, `watermark() -> u64`, `now_ms() -> u64`, `is_quiet(WindowId, quiet_ms) -> Option<bool>`.
### Specs (`src/spec.rs`)
- `WaitSpec { window_id, since_commit, timeout_ms }`, `QuietSpec { window_id, quiet_ms, timeout_ms, after_action }`, `ObserveSpec { window_id, after_action, until, timeout_ms }`; all with `Default` (protocol defaults), `new()` and chainable setters; `ObserveSpec::default().until` is `Condition::Quiet { quiet_ms: 250 }`.
- `Condition { Change, Quiet { quiet_ms }, Timeout }` + `quiet_threshold_ms()` (public helper: `Quiet` → its own `quiet_ms`; `Change`/`Timeout` → the `DEFAULT_QUIET_MS` constant).
- Image parameters (`include_image`, `region`, `max_dimension`) are deliberately absent: the server renders after the wait resolves.
### Actions (`src/actions.rs`)
- `ActionKind` (13 variants, `ALL`, `is_input()`, `as_str()` = AGP method name), `ActionRecord { id, kind, window_id, position, seq, ts_ms }`.
- `ActionRegistry`: cloneable, thread-safe, ids monotonic from 1, `record`, `get`, `seq_of`, `contains`, `len`, `is_empty`, `last`, `records`.
### State (`src/state.rs`)
- `WindowTemporalState { window_id, last_commit_seq, last_commit_at, last_damage, last_meaningful_change_at, last_input_at, quiet_since, commit_count, pending_observation, geometry, state_uncertain }` + `is_quiet(now_ms, quiet_ms)`.
- `PendingObservation { since_seq, started_at }`, `ObserverSnapshot { seq, ts_ms, windows, actions_len, journal_len, events_dropped }`.
- Resync input: `StateSnapshot { seq, ts_ms, windows }`, `WindowSnapshot { window_id, last_commit_seq, geometry, popup_count }`, `ResyncReport { snapshot_seq, windows_added, windows_removed, marked_uncertain, events_dropped }`.
### Errors (`src/error.rs`)
- `Error { UnknownWindow(WindowId), UnknownAction(ActionId), Internal(String) }`, `Result<T>`, `From<Error> for adesk_core::Error` (`unknown_window`, `invalid_request`, `internal`).
### Constants (`src/lib.rs`)
- `DEFAULT_JOURNAL_CAPACITY = 4096`, `DEFAULT_TIMEOUT_MS = 5000`, `DEFAULT_QUIET_MS = 250`.
## Constraints
- Dependencies: `adesk-core`, `tokio` (sync/time), `thiserror`, `tracing` only; versions come from the root `[workspace.dependencies]`. Dev-only: `tokio` + `test-util`.
- Never hold the state mutex across an `.await`. The only await points are `watch::Receiver::changed()` and `tokio::time::sleep_until`.
- Never poll, never sleep in a loop, never spawn background tasks inside the service.
- `handle_event` is on the hot path (`SurfaceCommit` at animation rates): counters, damage union, timestamps only — no rendering, no logging above `trace`.
- No panics on request/event paths; no `todo!()` anywhere in the crate.
- Public API is what this file documents; everything else stays `pub(crate)`. Production code stays under ~1000 lines per file; cohesive inline test modules may exceed that (see Notes for Agents).
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`.
## Routing Table
| Area | Owner |
|---|---|
| Event ingestion, per-window state machine, watermark | `./src/service.rs` |
| Wait loop, `PendingGuard`, query methods | `./src/service.rs` |
| Filters, accumulator, condition evaluation, `WaitPlan` | `./src/waiter.rs` |
| Action registry, `ActionKind`, `ActionRecord` | `./src/actions.rs` |
| Counted-event journal, `CountedEvent`/`CountedKind` | `./src/journal.rs` |
| Clock domain bridge (`ts_ms` ↔ tokio timers) | `./src/clock.rs` |
| `WaitSpec`/`QuietSpec`/`ObserveSpec`/`Condition` | `./src/spec.rs` |
| `WindowTemporalState`, snapshots, resync types | `./src/state.rs` |
| Crate error + `adesk_core::Error` mapping | `./src/error.rs` |
| Behaviour test suites (34/34 frozen specs green) | `./tests/waits.rs`, `./tests/filters.rs`, `./tests/concurrency.rs`, `./tests/resync.rs`, `./tests/actions.rs` |
| Public API smoke test | `./tests/api_surface.rs` |
| Event fixtures for tests | `./tests/common/mod.rs` |
| Consumer: AGP dispatch, event pump, rendering after waits | `../adesk-server/` (sibling — read-only, escalate changes) |
| `RuntimeEvent`, `Observation`, ids, geometry, `Error` | `../adesk-core/` (dependency — read-only) |
| End-to-end harness that drives this crate | `../adesk-testkit/` (sibling — read-only) |
## Design Decisions
- **Cloneable facade, one `Arc<Inner>`.** The event-pump task and every request task share one service; no channels between them are needed for state, only the broadcast stream from the compositor.
  Ingestion is push-only: the observer has no broadcast subscription of its own — `handle_event` (one event per call, in `seq` order) and `resync` (lag recovery) are its only ingestion points, both driven by the server.
- **Wakeups via `tokio::sync::watch<u64>` generation counter**, not `Notify`: a waiter subscribes *before* inspecting state and re-checks after every `changed()`, so no wakeup can be lost and no condition can be missed. `handle_event` and `resync` bump it.
- **Clock anchoring.** Event `ts_ms` is monotonic ms since *compositor* start; the observer starts later. `Clock` keeps `(anchor_ts_ms, tokio::time::Instant)` and re-anchors forward on every event/snapshot, so `now_ms()` lives in the event domain and deadlines are `tokio::time::sleep_until(clock.deadline(t))`. Under `tokio::time::pause()` the clock freezes at the last event timestamp — deterministic tests with no real sleeps.
- **Journal of 4096 counted events** matches the broadcast capacity floor (`docs/architecture.md` §1): a waiter can always be seeded over the same horizon after which a lagging subscriber would be told `Lagged`. Per-window aggregates (`commit_count`, `last_commit_seq`, `last_commit_at`) stay exact after eviction; only filter-relative counts degrade, and `state_uncertain` marks the hole.
- **`since_commit` filters commits only.** Lifecycle, title, focus and popup events are not commit-numbered and always count — a new window is a change even when `since_commit` is set.
- **`after_action` is a `seq` watermark captured at record time** (the highest processed event seq when the server accepted the action), so `seq > action_seq` means "after the action was accepted". An unknown action id is `Error::UnknownAction` — the observer never guesses a causal point.
- **No window filter is a global observation.** `window_id: None` is legal for every wait (never `UnknownWindow`): the wait counts events from all windows, echoes `window_id: None` in the `Observation`, and reports the global maximum as `last_commit_seq`; only an explicit `Some(id)` the observer never saw (or that was destroyed) errors. The `Observation` also echoes the requested `after_action` id verbatim (`None` when none was given).
- **Wait-start filter point.** `Filters::min_seq` is `after_action.seq` when set, otherwise the watermark at wait start: waits without `after_action` count only events after the wait began (a seeded lifecycle event must not resolve them instantly), so journal seeding matters only for `after_action`/`since_commit`.
- **Stale snapshots are ignored.** A `resync` whose `snapshot.seq < watermark` changes nothing (no pruning, no window removal, no backward watermark) and returns a report carrying the current watermark.
- **Quiet timer anchor** = `max(after_action.ts_ms, last counted commit ts)`, so a wait right after an action does not resolve instantly, and every counted commit re-arms it.
- **`quiet` is an evidence flag, not a promise.** It is `now_ms - quiet_anchor >= threshold` at resolution, where `threshold` is the condition's `quiet_ms` for quiet conditions and `DEFAULT_QUIET_MS` otherwise. The anchor is `max(plan.anchor_ts, last counted commit ts)` for condition evaluation and quiet-deadline scheduling (they must agree, or a quiet wait can wake early and never resolve); the resolve-time evidence flag uses `Accumulator::quiet_anchor(started_at)` (filter's last counted commit, else the wait start). `timed_out` is true only when the deadline expired before the condition was met. `Condition::Timeout` reaches its horizon by definition, so it reports `timed_out: false` and whatever accumulated (animation sampling).
- **Unknown window → error; destroyed window → resolution.** A wait on a window the observer never saw returns `Error::UnknownWindow`. If the window is destroyed *while* a wait is pending, the waiter resolves with the destruction counted in `destroyed_windows` (state is dropped, so later waits error).
- **`focus_changed` rule.** `FocusChanged` carries only the newly focused window, so only "focus moved to X" is observable: a counted `FocusChanged` (targeting the filtered window, or any when unfiltered) gives `Some(true)`; otherwise `None`. A focus-away event is not window-scoped to the filtered window and is rejected by the filter, so `Some(false)` is not derivable from the wire event.
- **Counted kinds.** `SurfaceCommit`, `WindowCreated`, `WindowDestroyed`, `WindowActivated`, `TitleChanged`, `FocusChanged`, `PopupAppeared`, `PopupDisappeared`. `AppLaunched` is *not* counted (process launch, no window, no GUI state) — it only advances the watermark.
- **`last_damage` is the most recent commit's damage** (replaced, not accumulated); per-waiter unions come from the journal and are `Region::simplified()`, clipped to the window geometry when known (geometry arrives via `resync`). `quiet_since` is the timestamp of the last counted commit; per-waiter `quiet_ms` thresholds are applied against it.
- **`pending_observation`** holds the earliest active waiter (`since_seq` = lowest filter point, `started_at` = its start), maintained by an RAII guard so client disconnects cannot leak it.
- **Resync is snapshot-driven and synthesized-event-based**: adopt `max(watermark, snapshot.seq)`, prune the journal through it, insert missed windows (`state_uncertain`, geometry set, synthetic `Created`), adopt advanced `last_commit_seq` as a synthetic empty-damage `Commit` at `snapshot.seq`, drop windows missing from the snapshot with a synthetic `Destroyed`, bump the generation. Waiters therefore stay correct: they only count events.
- **The observer never renders.** `include_image`/`region`/`max_dimension` stay in `adesk-server`, which renders *after* the wait resolves so the image reflects the settled state.
- **`ActionRegistry` is unbounded** (an `after_action` may be referenced long after the fact); at ~64 bytes/action a long agent run is still negligible. `len()` is exposed for monitoring; pruning would break `after_action` references.
## Server integration (`adesk-server` wiring)
- **Event pump**: one task holds an `ObserverService` clone and a `broadcast::Receiver<RuntimeEvent>`; it calls `handle_event(&event)` for every event in order. On `RecvError::Lagged`, send `RuntimeCommand::QueryState`, translate the reply into `adesk_observer::StateSnapshot`, and call `resync` — never continue silently.
- **Actions**: on accepting an input/`activate_window`/`close_window` request, call `record_action(...)` *before* sending the compositor command and return the id as `action_id`. `after_action` in later requests is that id.
- **Waits**: request tasks call `wait_for_change`/`wait_for_quiet`/`observe` directly (they are `Send` futures, cancellation-safe). `Err(Error::UnknownWindow)` → AGP `unknown_window`; `Err(Error::UnknownAction)` → AGP `invalid_request`; otherwise the `Observation` is returned as-is. Timeouts are observations (`timed_out: true`), never `timeout` errors.
- **Images**: when `include_image` is set, the server issues `RenderWindow` *after* the wait resolves and attaches the payload to `ObserveResult`.
- **Queries**: `list_windows`-adjacent temporal data comes from `snapshot()`/`window_state()`; `ping`/inspector can use `watermark()`, `now_ms()`, `snapshot().journal_len`.
- **`quiet` event subscriptions** (AGP §5.6) are a server loop over `wait_for_quiet` per subscription — the observer has no push API of its own.
## Test Strategy
- Unit tests live inline in modules for the pure pieces (filters, condition predicates, journal eviction, registry allocation, clock anchoring, service state machine) — `#[cfg(test)]`, 85 total; tokio needed only for the service/waiter tests.
- Integration tests are the protocol-level scenarios in `./tests/`: `waits.rs` (10 specs), `filters.rs` (8), `concurrency.rs` (5), `resync.rs` (5), `actions.rs` (6). All bodies are implemented and green; no spec is ignored.
- Determinism: `#[tokio::test(start_paused = true)]` + events with explicit `seq`/`ts_ms` from `./tests/common/mod.rs`; time only moves via `tokio::time::advance`. No real sleeps, no display, no GPU, no network, no installed apps.
- Do **not** un-ignore a failing spec to "fix" it: a wrong implementation makes paused-time waits hang forever. Assertions in `./tests/*.rs` are frozen — behaviour must be argued against `docs/protocol.md`, never against the specs.
- `./tests/api_surface.rs` pins the public API shape (builders, enum mappings, snapshot structs) without running a service.
- Run: `./scripts/dev.sh cargo test -p adesk-observer`.
## Known Issues
- `after_action` pointing at an action older than the retained journal yields degraded filter-relative counts (aggregates stay exact); the affected windows are flagged `state_uncertain`.
- Damage clipping needs window geometry, which only `resync` provides; before the first resync, `changed_regions` are unclipped (damage is already window-relative).
- The `quiet` evidence flag for non-quiet conditions uses `ObserverConfig::default_quiet_ms`, not the server's per-request value; the server can override per request by using a `Quiet` condition.
- `Clock::now_ms()` truncates to whole milliseconds, so a deadline can fire up to ~1 ms early — inherent to the event-ts domain, consistent with "quiet is evidence, never a promise".
- Popups have no state of their own: `PopupAppeared`/`PopupDisappeared` are owner-window counted events and `WindowSnapshot::popup_count` is accepted by `resync` but ignored.
  A popup-driven `SurfaceCommit` carries the owner's `window_id` with popup-relative damage, so it advances the owner's `commit_seq`/`commit_count`, re-arms `wait_for_quiet`, and its damage is clipped against the owner's geometry — damage from a popup outside the window can be clipped away or misattributed.
- `FocusChanged { window_id: None }` is invisible to window-filtered waits (the filter requires `Some(window_id)`); when an unfiltered wait counts it, `Observation::focus_changed` becomes `Some(true)` even though no window received focus.
- `last_meaningful_change_at` is updated by every counted non-commit event but is not read by any production code path; it exists for server/inspector state queries only.
- A window-scoped event for an unknown window (commit, title, activation, popup, focus) creates its `WindowTemporalState` on demand, so `window_ids()`/`snapshot()` can contain a window the observer never saw a `WindowCreated` for; a later `resync` removes it if the snapshot does not cover it.
## Status
Phase 2 (implementation) complete — zero `todo!()` in the crate; every module body, wait loop, resync path and frozen spec body is implemented.
Validation (`./scripts/dev.sh`): `cargo check -p adesk-observer --all-targets` warning-free; `cargo clippy -p adesk-observer --all-targets -- -D warnings` clean; `cargo test -p adesk-observer` → 125 passed, 0 failed, 0 ignored (85 lib unit + 5 api_surface + 34 frozen specs + 1 doctest).
Skeleton-phase module-level `#![allow(dead_code)]` blocks are gone; only `tests/common/mod.rs` keeps one (shared fixture module, each test binary uses a subset) plus two item-level test-only allows (see Notes for Agents).
## Notes for Agents
- `src/service.rs` is ~838 production lines plus a ~990-line cohesive inline test module; the single-file layout is deliberate (one state machine, one lock, tests next to the code they pin). Do not split it to satisfy the ~1000-line soft threshold.
- Item-level `#[allow(dead_code)]` remains on `Clock::until` and `EventJournal::oldest_seq` — test-only accessors; remove them only together with the tests that use them.
- Waiters are cancellation-safe and `Send`: dropping the future unregisters the `PendingGuard`; never add a code path that registers a waiter without holding the guard.
- `adesk_core::Observation` has **no `ts_ms` field**: resolution time is `elapsed_ms` (from the wait start) plus the `seq`/`last_commit_seq` watermarks. The crate exposes no event-history API; `snapshot()` returns counts only (`journal_len`, `events_dropped`).
- `WaitSpec` has no `after_action`; action-correlated change waits use `ObserveSpec::new(Condition::Change).after_action(id)`.
- Window-filtered `Observation::last_commit_seq` is the window's *absolute* commit watermark (non-zero even when `commits == 0`); unfiltered it is the global max across windows.
- An already-quiet window does not resolve `wait_for_quiet` instantly: with no counted commit in the filter the anchor is the wait start (or the action `ts_ms`), so resolution is `anchor + quiet_ms` — or immediate with `quiet: false` when `after_action` is already older than `quiet_ms`.
- `Observation::quiet` is the evidence flag above, *not* "condition met": `docs/protocol.md` §5.4 and the field's doc in `adesk-core` phrase it as the latter, but a timed-out `change` wait can legitimately carry `quiet: true`.
## Dependencies
- `adesk-core` (landed): `RuntimeEvent`, `Observation`, `WindowId`, `ActionId`, `Position`, `Rect`, `Region`, `Error`/`ErrorCode`.
- `tokio`: `sync` (`watch`), `time` (`Instant`, `sleep_until`); dev-only `test-util` for paused time.
- `thiserror` for the crate error, `tracing` for spans (`request{id method}` in the server; the observer logs at `trace`).
