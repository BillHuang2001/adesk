//! [`ObserverService`] — the runtime's temporal memory and the AGP wait engine.
//!
//! Threading model (`docs/architecture.md` §1, §6):
//!
//! - The service is a cheap, cloneable handle (`Arc` inside). One clone lives in
//!   the server's event-pump task and calls [`ObserverService::handle_event`] for
//!   every broadcast [`RuntimeEvent`]; every request task holds another clone.
//! - Shared state sits behind a `std::sync::Mutex`. Critical sections are short
//!   and **never held across `.await`** — the only await points are
//!   `tokio::sync::watch::Receiver::changed()` and `tokio::time::sleep_until`.
//! - Wakeups use a `watch<u64>` generation counter that `handle_event` and
//!   `resync` bump. A waiter subscribes *before* inspecting state and re-checks
//!   after every wakeup, so no wakeup can be lost and no wait ever polls.
//! - Time is `tokio::time` in the event `ts_ms` domain (see `clock.rs`), which
//!   makes every wait deterministic under `tokio::time::pause()`.

// Phase 1 architecture skeleton: method bodies are `todo!()`, so the fields they
// will read look unused. Remove this allow together with the last `todo!()` in
// this file (see `CONTEXT.md` → Status).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use adesk_core::{ActionId, Observation, Position, Rect, RuntimeEvent, WindowId};
use tokio::sync::watch;

use crate::actions::{ActionKind, ActionRecord, ActionRegistry};
use crate::clock::Clock;
use crate::error::Result;
use crate::journal::EventJournal;
use crate::spec::{Condition, ObserveSpec, QuietSpec, WaitSpec};
use crate::state::{ObserverSnapshot, ResyncReport, StateSnapshot, WindowTemporalState};
use crate::waiter::{WaitCondition, WaitPlan};
use crate::{DEFAULT_JOURNAL_CAPACITY, DEFAULT_QUIET_MS};

/// Tunables of an [`ObserverService`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverConfig {
    /// Counted events retained for waiter seeding (see `journal.rs`).
    pub journal_capacity: usize,
    /// Quiet threshold used for the `quiet` evidence flag of non-quiet waits.
    pub default_quiet_ms: u64,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            journal_capacity: DEFAULT_JOURNAL_CAPACITY,
            default_quiet_ms: DEFAULT_QUIET_MS,
        }
    }
}

/// The temporal observation engine (cloneable handle).
#[derive(Debug, Clone)]
pub struct ObserverService {
    inner: Arc<Inner>,
}

impl Default for ObserverService {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared service state. `pub(crate)` so `waiter.rs`/tests can inspect it.
#[derive(Debug)]
pub(crate) struct Inner {
    /// Tunables.
    pub(crate) config: ObserverConfig,
    /// Per-window temporal state + journal + watermark.
    pub(crate) state: Mutex<ServiceState>,
    /// Clock domain bridge (own mutex: never taken while `state` is held).
    pub(crate) clock: Mutex<Clock>,
    /// Recorded actions.
    pub(crate) actions: ActionRegistry,
    /// Event generation counter; bumped by every `handle_event`/`resync`.
    pub(crate) events: watch::Sender<u64>,
}

/// Everything behind the state mutex.
#[derive(Debug)]
pub(crate) struct ServiceState {
    /// Temporal state by window id (ascending).
    pub(crate) windows: BTreeMap<WindowId, WindowTemporalState>,
    /// Counted-event journal (waiter seeding + filter-relative counts).
    pub(crate) journal: EventJournal,
    /// Global event sequence watermark (highest `seq` processed).
    pub(crate) watermark: u64,
}

impl ServiceState {
    /// Highest commit sequence across all known windows.
    pub(crate) fn global_last_commit_seq(&self) -> u64 {
        self.windows
            .values()
            .map(|window| window.last_commit_seq)
            .max()
            .unwrap_or(0)
    }
}

/// RAII registration of an active waiter on a window; dropped on resolution or
/// cancellation (client disconnect), so `pending_observation` can never leak.
#[derive(Debug)]
pub(crate) struct PendingGuard {
    inner: Arc<Inner>,
    window_id: Option<WindowId>,
}

impl PendingGuard {
    /// Registers a waiter in the target window's `pending_observation`.
    pub(crate) fn register(
        inner: &Arc<Inner>,
        window_id: Option<WindowId>,
        since_seq: u64,
        started_at: u64,
    ) -> Self {
        let _ = (inner, window_id, since_seq, started_at);
        todo!("Phase 2: set/merge pending_observation under the state lock")
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let _ = self;
        todo!("Phase 2: release pending_observation under the state lock")
    }
}

impl ObserverService {
    /// Service with [`ObserverConfig::default`].
    pub fn new() -> Self {
        Self::with_config(ObserverConfig::default())
    }

    /// Service with explicit tunables.
    pub fn with_config(config: ObserverConfig) -> Self {
        let _ = config;
        todo!("Phase 2: build Inner with empty ServiceState, Clock and watch channel")
    }

    // ---------------------------------------------------------------- event pump

    /// Consumes one compositor event. Called by the server's event-pump task for
    /// every broadcast event, in `seq` order.
    ///
    /// Contract (must stay cheap — `SurfaceCommit` is high-frequency):
    ///
    /// - advance the watermark to `max(watermark, event.seq())`;
    /// - re-anchor the clock at `event.ts_ms()`;
    /// - `WindowCreated`: insert [`WindowTemporalState::created`] if absent
    ///   (existing state is kept, only `state_uncertain` is cleared);
    /// - `WindowDestroyed`: drop the window's state (new waits then report
    ///   `unknown_window`) and journal the destruction so in-flight waiters resolve;
    /// - `SurfaceCommit`: `last_commit_seq = max(.., commit_seq)`, `last_commit_at =
    ///   ts_ms`, `last_damage` = this commit's damage (replaced), `quiet_since =
    ///   Some(ts_ms)`, `commit_count += 1`;
    /// - `WindowActivated` / `TitleChanged` / `FocusChanged` / `Popup*`: journal
    ///   them (they count as changes);
    /// - window-scoped events for unknown windows create state on demand
    ///   (lagged streams are self-healing) — a later `resync` corrects it;
    /// - `AppLaunched`: watermark only, never counted;
    /// - push a [`CountedEvent`] for counted kinds and bump the generation;
    /// - never log above `trace`.
    pub fn handle_event(&self, event: &RuntimeEvent) {
        let _ = (self, event);
        todo!("Phase 2: update ServiceState + journal, then bump the event generation")
    }

    /// Resyncs after a broadcast `Lagged` error (`docs/architecture.md` §1).
    ///
    /// The server translates the compositor's `QueryState` reply into
    /// [`StateSnapshot`] and calls this instead of silently continuing.
    ///
    /// Algorithm:
    ///
    /// 1. adopt `max(watermark, snapshot.seq)` and re-anchor the clock at
    ///    `snapshot.ts_ms`;
    /// 2. drop journal entries with `seq <= snapshot.seq` (the snapshot supersedes
    ///    them);
    /// 3. windows in the snapshot but unknown to the observer: insert with
    ///    `last_commit_seq` from the snapshot, `last_commit_at = snapshot.ts_ms`
    ///    (best effort), geometry set, `state_uncertain = true`, and journal a
    ///    synthetic `Created` at `snapshot.seq`;
    /// 4. known windows: adopt a higher `last_commit_seq` (journal a synthetic
    ///    empty-damage `Commit` at `snapshot.seq` and re-anchor `quiet_since`),
    ///    refresh geometry, set `state_uncertain = true`;
    /// 5. windows the observer knows but the snapshot does not: remove them and
    ///    journal a synthetic `Destroyed` at `snapshot.seq`;
    /// 6. bump the generation so every waiter re-evaluates.
    ///
    /// Waiters stay correct: they only ever *count* events, so synthesized events
    /// keep `change`/`quiet` conditions honest, and the per-window
    /// `state_uncertain` flag is the evidence that the history has a hole.
    pub fn resync(&self, snapshot: StateSnapshot) -> ResyncReport {
        let _ = (self, snapshot);
        todo!("Phase 2: implement the six resync steps and return ResyncReport")
    }

    // -------------------------------------------------------------------- actions

    /// Records an agent action and returns its id (`after_action` handle).
    ///
    /// The stored `seq` is the current watermark and `ts_ms` the current clock
    /// value, so `seq > action_seq` means "after the action was accepted".
    /// Input kinds ([`ActionKind::is_input`]) also stamp `last_input_at` on the
    /// target window; runtime-native operations (`activate_window`,
    /// `close_window`) never do (design invariant 3).
    pub fn record_action(
        &self,
        kind: ActionKind,
        window_id: Option<WindowId>,
        position: Option<Position>,
    ) -> ActionId {
        let _ = (self, kind, window_id, position);
        todo!("Phase 2: read watermark + clock, record in the registry, stamp last_input_at")
    }

    /// Looks up a recorded action.
    pub fn action(&self, action_id: ActionId) -> Option<ActionRecord> {
        self.inner.actions.get(action_id)
    }

    /// The `after_action` filter point of a recorded action.
    pub fn action_seq(&self, action_id: ActionId) -> Option<u64> {
        self.inner.actions.seq_of(action_id)
    }

    /// The underlying registry (for the inspector overlay / diagnostics).
    pub fn action_registry(&self) -> ActionRegistry {
        self.inner.actions.clone()
    }

    // ----------------------------------------------------------------------- waits

    /// `wait_for_change` (AGP §5.4): resolve on the first counted event.
    ///
    /// `since_commit` restricts commits to `commit_seq > since_commit`; lifecycle
    /// events always count. Timeout resolves with `timed_out: true` and whatever
    /// accumulated. Unknown `window_id` is an error, not a timeout.
    pub async fn wait_for_change(&self, spec: WaitSpec) -> Result<Observation> {
        let _ = (self, spec);
        todo!("Phase 2: build WaitPlan for Change and delegate to run_wait")
    }

    /// `wait_for_quiet` (AGP §5.4): resolve once `quiet_ms` have elapsed without a
    /// counted commit, re-arming on every counted commit.
    ///
    /// The quiet timer is anchored at `max(action.ts_ms, last counted commit)`, so
    /// a wait right after an action does not resolve instantly. `timeout_ms` still
    /// bounds the wait; `after_action` must be a known action.
    pub async fn wait_for_quiet(&self, spec: QuietSpec) -> Result<Observation> {
        let _ = (self, spec);
        todo!("Phase 2: build WaitPlan for Quiet and delegate to run_wait")
    }

    /// `observe` (AGP §5.4): wait for an explicit [`Condition`].
    ///
    /// `Condition::Timeout` samples the full timeout and reports `timed_out:
    /// false`. Images are attached by the server after this call returns
    /// (`include_image`), which is why no image parameter exists here.
    pub async fn observe(&self, spec: ObserveSpec) -> Result<Observation> {
        let _ = (self, spec);
        todo!("Phase 2: build WaitPlan from Condition and delegate to run_wait")
    }

    /// The shared wait loop.
    ///
    /// 1. validate the window filter (known window or [`Error::UnknownWindow`]) and
    ///    resolve `after_action` (or [`Error::UnknownAction`]);
    /// 2. subscribe to the generation `watch` **before** seeding;
    /// 3. register a [`PendingGuard`];
    /// 4. seed an [`Accumulator`] from `journal.since(filters.min_seq)`;
    /// 5. loop: evaluate [`condition_met`] / deadline, else
    ///    `select! { rx.changed(), sleep_until(clock.deadline(deadline_ms)) }`;
    /// 6. resolve via [`Accumulator::resolve`] with the current window state.
    async fn run_wait(&self, plan: WaitPlan) -> Result<Observation> {
        let _ = (self, plan);
        todo!("Phase 2: implement the event-driven wait loop (no polling, no sleeps)")
    }

    // ---------------------------------------------------------------------- queries

    /// Point-in-time snapshot of all temporal state, for `list_windows`-adjacent
    /// queries that must not wait.
    pub fn snapshot(&self) -> ObserverSnapshot {
        let _ = self;
        todo!("Phase 2: clone windows + watermark + journal diagnostics")
    }

    /// Temporal state of one window (`None` once it is destroyed).
    pub fn window_state(&self, window_id: WindowId) -> Option<WindowTemporalState> {
        let _ = (self, window_id);
        todo!("Phase 2: state-lock lookup")
    }

    /// Current global event watermark (highest `seq` processed).
    pub fn watermark(&self) -> u64 {
        let _ = self;
        todo!("Phase 2: state-lock read of watermark")
    }

    /// Current time in the event `ts_ms` domain.
    pub fn now_ms(&self) -> u64 {
        let _ = self;
        todo!("Phase 2: clock-lock now_ms()")
    }

    /// Ids of the windows the observer currently tracks (ascending).
    pub fn window_ids(&self) -> Vec<WindowId> {
        let _ = self;
        todo!("Phase 2: state-lock keys")
    }

    /// `true` when `window_id` has been quiet for `quiet_ms` at `now_ms`.
    pub fn is_quiet(&self, window_id: WindowId, quiet_ms: u64) -> Option<bool> {
        let _ = (self, window_id, quiet_ms);
        todo!("Phase 2: window_state + WindowTemporalState::is_quiet")
    }
}

/// Convenience: window geometry for damage clipping, when the observer knows it.
#[allow(dead_code)]
pub(crate) fn window_geometry(state: Option<&WindowTemporalState>) -> Option<Rect> {
    state.and_then(|window| window.geometry)
}

/// Convenience: `Condition` → internal [`WaitCondition`].
#[allow(dead_code)]
pub(crate) fn wait_condition(condition: Condition) -> WaitCondition {
    match condition {
        Condition::Change => WaitCondition::Change,
        Condition::Quiet { quiet_ms } => WaitCondition::Quiet { quiet_ms },
        Condition::Timeout => WaitCondition::Timeout,
    }
}
