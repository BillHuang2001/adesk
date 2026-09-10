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

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use adesk_core::{ActionId, Observation, Position, Rect, Region, RuntimeEvent, WindowId};
use tokio::sync::watch;

use crate::actions::{ActionKind, ActionRecord, ActionRegistry};
use crate::clock::Clock;
use crate::error::{Error, Result};
use crate::journal::{CountedEvent, CountedKind, EventJournal};
use crate::spec::{Condition, ObserveSpec, QuietSpec, WaitSpec};
use crate::state::{
    ObserverSnapshot, PendingObservation, ResyncReport, StateSnapshot, WindowTemporalState,
};
use crate::waiter::{condition_met, Accumulator, Filters, ResolveContext, WaitCondition, WaitPlan};
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
    /// Active waiters per window: `(filters.min_seq, started_at) → refcount`.
    ///
    /// A multiset, because concurrent waiters can share the exact same filter
    /// point; `WindowTemporalState::pending_observation` is the minimum key.
    pub(crate) pending: BTreeMap<WindowId, BTreeMap<(u64, u64), u32>>,
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

/// Locks the state mutex, ignoring poisoning.
///
/// A poisoned lock (another thread panicked while holding it) still holds usable
/// data; panicking on every later request/event would be far worse than reading
/// it.
fn lock_state(inner: &Inner) -> MutexGuard<'_, ServiceState> {
    inner
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks the clock mutex, ignoring poisoning (see [`lock_state`]).
///
/// Always acquired *without* holding [`lock_state`], so the two locks can never
/// deadlock against each other.
fn lock_clock(inner: &Inner) -> MutexGuard<'_, Clock> {
    inner
        .clock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Window state, created on demand when the event stream is self-healing.
fn window_on_demand(
    state: &mut ServiceState,
    window_id: WindowId,
    ts_ms: u64,
) -> &mut WindowTemporalState {
    state
        .windows
        .entry(window_id)
        .or_insert_with(|| WindowTemporalState::created(window_id, ts_ms))
}

/// Recomputes `windows[id].pending_observation` from the active-waiter multiset.
///
/// The multiset is ordered by `(since_seq, started_at)`, so its first key is the
/// earliest active waiter. A window that was destroyed while waiters are pending
/// simply has nothing left to annotate.
fn refresh_pending_observation(state: &mut ServiceState, window_id: WindowId) {
    let earliest = state
        .pending
        .get(&window_id)
        .and_then(|waiters| waiters.keys().next().copied())
        .map(|(since_seq, started_at)| PendingObservation {
            since_seq,
            started_at,
        });
    if let Some(window) = state.windows.get_mut(&window_id) {
        window.pending_observation = earliest;
    }
}

/// RAII registration of an active waiter on a window; dropped on resolution or
/// cancellation (client disconnect), so `pending_observation` can never leak.
#[derive(Debug)]
pub(crate) struct PendingGuard {
    inner: Arc<Inner>,
    window_id: Option<WindowId>,
    key: (u64, u64),
}

impl PendingGuard {
    /// Registers a waiter in the target window's `pending_observation`.
    pub(crate) fn register(
        inner: &Arc<Inner>,
        window_id: Option<WindowId>,
        since_seq: u64,
        started_at: u64,
    ) -> Self {
        let key = (since_seq, started_at);
        if let Some(window_id) = window_id {
            let mut state = lock_state(inner);
            let waiters = state.pending.entry(window_id).or_default();
            *waiters.entry(key).or_insert(0) += 1;
            refresh_pending_observation(&mut state, window_id);
        }
        Self {
            inner: Arc::clone(inner),
            window_id,
            key,
        }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let Some(window_id) = self.window_id else {
            return;
        };
        let mut state = lock_state(&self.inner);
        let mut empty = false;
        if let Some(waiters) = state.pending.get_mut(&window_id) {
            if let Some(count) = waiters.get_mut(&self.key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    waiters.remove(&self.key);
                }
            }
            empty = waiters.is_empty();
        }
        if empty {
            state.pending.remove(&window_id);
        }
        refresh_pending_observation(&mut state, window_id);
    }
}

impl ObserverService {
    /// Service with [`ObserverConfig::default`].
    pub fn new() -> Self {
        Self::with_config(ObserverConfig::default())
    }

    /// Service with explicit tunables.
    pub fn with_config(config: ObserverConfig) -> Self {
        let journal = EventJournal::new(config.journal_capacity);
        Self {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(ServiceState {
                    windows: BTreeMap::new(),
                    journal,
                    watermark: 0,
                    pending: BTreeMap::new(),
                }),
                clock: Mutex::new(Clock::new()),
                actions: ActionRegistry::new(),
                // The generation starts at 0; waiters subscribe to it before they
                // inspect state, so an event that races the seed still wakes them.
                events: watch::channel(0).0,
            }),
        }
    }

    // ---------------------------------------------------------------- event pump

    /// Consumes one compositor event. Called by the server's event-pump task for
    /// every broadcast event, in `seq` order.
    ///
    /// Contract (must stay cheap — `SurfaceCommit` is high-frequency):
    ///
    /// - advance the watermark to `max(watermark, event.seq())`;
    /// - re-anchor the clock at `event.ts_ms()`;
    /// - `WindowCreated`: insert `WindowTemporalState::created` if absent
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
    /// - push a `CountedEvent` for counted kinds and bump the generation;
    /// - never log above `trace`.
    pub fn handle_event(&self, event: &RuntimeEvent) {
        let seq = event.seq();
        let ts_ms = event.ts_ms();
        let counted = CountedEvent::from_runtime_event(event);
        let is_counted = counted.is_some();

        // Clock first, under its own mutex: never held together with `state`.
        lock_clock(&self.inner).observe_ts(ts_ms);

        {
            let mut state = lock_state(&self.inner);
            state.watermark = state.watermark.max(seq);
            match event {
                RuntimeEvent::WindowCreated { window_id, .. } => {
                    match state.windows.entry(*window_id) {
                        // State already exists (e.g. created on demand after a lag):
                        // keep its history and only close the uncertainty hole.
                        Entry::Occupied(mut entry) => entry.get_mut().state_uncertain = false,
                        Entry::Vacant(entry) => {
                            entry.insert(WindowTemporalState::created(*window_id, ts_ms));
                        }
                    }
                }
                RuntimeEvent::WindowDestroyed { window_id, .. } => {
                    state.windows.remove(window_id);
                }
                RuntimeEvent::SurfaceCommit {
                    window_id,
                    commit_seq,
                    damage,
                    ..
                } => {
                    let window = window_on_demand(&mut state, *window_id, ts_ms);
                    window.last_commit_seq = window.last_commit_seq.max(*commit_seq);
                    window.last_commit_at = ts_ms;
                    window.last_damage = damage.clone();
                    window.quiet_since = Some(ts_ms);
                    window.commit_count = window.commit_count.saturating_add(1);
                    window.last_meaningful_change_at = ts_ms;
                    window.state_uncertain = false;
                }
                RuntimeEvent::WindowActivated { window_id, .. }
                | RuntimeEvent::TitleChanged { window_id, .. }
                | RuntimeEvent::PopupAppeared { window_id, .. }
                | RuntimeEvent::PopupDisappeared { window_id, .. } => {
                    let window = window_on_demand(&mut state, *window_id, ts_ms);
                    window.last_meaningful_change_at = ts_ms;
                    window.state_uncertain = false;
                }
                RuntimeEvent::FocusChanged { window_id, .. } => {
                    if let Some(window_id) = window_id {
                        let window = window_on_demand(&mut state, *window_id, ts_ms);
                        window.last_meaningful_change_at = ts_ms;
                        window.state_uncertain = false;
                    }
                }
                // Process launch: no window, no GUI state change. Watermark + clock
                // only, and `from_runtime_event` already returns `None` for it.
                RuntimeEvent::AppLaunched { .. } => {}
            }
            // A window whose state was dropped and re-created (or created on demand
            // while a waiter is already registered) must keep its active waiters
            // visible. The lookup is a no-op whenever nothing is pending.
            if let Some(window_id) = event.window_id() {
                if state.pending.contains_key(&window_id) {
                    refresh_pending_observation(&mut state, window_id);
                }
            }
            if let Some(counted) = counted {
                state.journal.push(counted);
            }
        }

        // `send_modify` cannot fail while any receiver exists, and the generation is
        // the only wakeup source for waiters.
        self.inner
            .events
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        tracing::trace!(
            seq,
            ts_ms,
            counted = is_counted,
            "observer: event processed"
        );
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
        // Re-anchor the clock first, under its own mutex. `Clock::observe_ts` only
        // moves forward, so a stale snapshot cannot move it backwards either.
        lock_clock(&self.inner).observe_ts(snapshot.ts_ms);

        let mut state = lock_state(&self.inner);

        // A reply that raced newer events must never move the watermark backwards,
        // prune the journal or drop windows: it changes nothing at all.
        if snapshot.seq < state.watermark {
            tracing::trace!(
                snapshot_seq = snapshot.seq,
                watermark = state.watermark,
                "observer: stale snapshot ignored"
            );
            return ResyncReport {
                snapshot_seq: state.watermark,
                ..ResyncReport::default()
            };
        }

        // 1. adopt the snapshot watermark.
        state.watermark = state.watermark.max(snapshot.seq);

        // 2. the snapshot supersedes everything at or below its sequence.
        let events_dropped = state.journal.prune_through(snapshot.seq);

        let mut report = ResyncReport {
            snapshot_seq: state.watermark,
            events_dropped,
            ..ResyncReport::default()
        };
        let mut synthetic: Vec<CountedEvent> = Vec::new();

        // 3./4. every window the snapshot covers.
        let mut covered: BTreeSet<WindowId> = BTreeSet::new();
        for window in &snapshot.windows {
            covered.insert(window.window_id);
            match state.windows.get_mut(&window.window_id) {
                None => {
                    // 3. Missed `WindowCreated`: seed the state from the snapshot.
                    let mut fresh = WindowTemporalState::created(window.window_id, snapshot.ts_ms);
                    fresh.last_commit_seq = window.last_commit_seq;
                    if window.last_commit_seq > 0 {
                        // Best effort: the commit timestamp is unknown, the snapshot
                        // time is the closest evidence we have.
                        fresh.last_commit_at = snapshot.ts_ms;
                        fresh.quiet_since = Some(snapshot.ts_ms);
                    }
                    fresh.geometry = Some(window.geometry);
                    fresh.state_uncertain = true;
                    state.windows.insert(window.window_id, fresh);
                    synthetic.push(CountedEvent {
                        seq: snapshot.seq,
                        ts_ms: snapshot.ts_ms,
                        window_id: Some(window.window_id),
                        kind: CountedKind::Created,
                    });
                    report.windows_added.push(window.window_id);
                }
                Some(existing) => {
                    // 4. Missed commits: adopt the higher commit sequence and make it
                    // visible to waiters as an empty-damage commit.
                    if window.last_commit_seq > existing.last_commit_seq {
                        existing.last_commit_seq = window.last_commit_seq;
                        existing.last_commit_at = snapshot.ts_ms;
                        existing.quiet_since = Some(snapshot.ts_ms);
                        existing.last_meaningful_change_at = snapshot.ts_ms;
                        existing.commit_count = existing.commit_count.saturating_add(1);
                        synthetic.push(CountedEvent {
                            seq: snapshot.seq,
                            ts_ms: snapshot.ts_ms,
                            window_id: Some(window.window_id),
                            kind: CountedKind::Commit {
                                commit_seq: window.last_commit_seq,
                                damage: Region::empty(),
                            },
                        });
                    }
                    existing.geometry = Some(window.geometry);
                    existing.state_uncertain = true;
                }
            }
            report.marked_uncertain.push(window.window_id);
        }

        // 5. windows the observer knows but the snapshot does not: they were
        // destroyed during the lag.
        let gone: Vec<WindowId> = state
            .windows
            .keys()
            .copied()
            .filter(|window_id| !covered.contains(window_id))
            .collect();
        for window_id in gone {
            state.windows.remove(&window_id);
            synthetic.push(CountedEvent {
                seq: snapshot.seq,
                ts_ms: snapshot.ts_ms,
                window_id: Some(window_id),
                kind: CountedKind::Destroyed,
            });
            report.windows_removed.push(window_id);
        }

        for event in synthetic {
            state.journal.push(event);
        }
        drop(state);

        // 6. every waiter must re-evaluate against the reconciled state.
        self.inner
            .events
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        tracing::trace!(
            snapshot_seq = report.snapshot_seq,
            added = report.windows_added.len(),
            removed = report.windows_removed.len(),
            uncertain = report.marked_uncertain.len(),
            events_dropped = report.events_dropped,
            "observer: resynced"
        );
        report
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
        let now_ms = self.now_ms();
        let watermark = lock_state(&self.inner).watermark;
        let id = self
            .inner
            .actions
            .record(kind, window_id, position, watermark, now_ms);

        if let Some(window_id) = window_id {
            let mut state = lock_state(&self.inner);
            // Self-healing, exactly like events: an action may name a window the
            // observer has not seen yet (e.g. right after a launch).
            let window = window_on_demand(&mut state, window_id, now_ms);
            if kind.is_input() {
                window.last_input_at = Some(now_ms);
            }
        }
        tracing::trace!(action = ?id, kind = kind.as_str(), "observer: action recorded");
        id
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
        let started_at = self.now_ms();
        let plan = WaitPlan {
            window_id: spec.window_id,
            after_action: None,
            // Without `after_action` the filter starts at the watermark: a seeded
            // lifecycle event must not resolve the wait instantly.
            filters: Filters::new(spec.window_id, self.watermark(), spec.since_commit),
            condition: WaitCondition::Change,
            timeout_ms: spec.timeout_ms,
            anchor_ts: started_at,
            started_at,
        };
        self.run_wait(plan).await
    }

    /// `wait_for_quiet` (AGP §5.4): resolve once `quiet_ms` have elapsed without a
    /// counted commit, re-arming on every counted commit.
    ///
    /// The quiet timer is anchored at `max(action.ts_ms, last counted commit)`, so
    /// a wait right after an action does not resolve instantly. `timeout_ms` still
    /// bounds the wait; `after_action` must be a known action.
    pub async fn wait_for_quiet(&self, spec: QuietSpec) -> Result<Observation> {
        let started_at = self.now_ms();
        let (after_action, min_seq, anchor_ts) =
            self.action_anchor(spec.after_action, started_at)?;
        let plan = WaitPlan {
            window_id: spec.window_id,
            after_action,
            filters: Filters::new(spec.window_id, min_seq, None),
            condition: WaitCondition::Quiet {
                quiet_ms: spec.quiet_ms,
            },
            timeout_ms: spec.timeout_ms,
            anchor_ts,
            started_at,
        };
        self.run_wait(plan).await
    }

    /// `observe` (AGP §5.4): wait for an explicit [`Condition`].
    ///
    /// `Condition::Timeout` samples the full timeout and reports `timed_out:
    /// false`. Images are attached by the server after this call returns
    /// (`include_image`), which is why no image parameter exists here.
    pub async fn observe(&self, spec: ObserveSpec) -> Result<Observation> {
        let started_at = self.now_ms();
        let (after_action, min_seq, anchor_ts) =
            self.action_anchor(spec.after_action, started_at)?;
        let plan = WaitPlan {
            window_id: spec.window_id,
            after_action,
            filters: Filters::new(spec.window_id, min_seq, None),
            condition: wait_condition(spec.until),
            timeout_ms: spec.timeout_ms,
            anchor_ts,
            started_at,
        };
        self.run_wait(plan).await
    }

    /// Resolves the `after_action` filter point.
    ///
    /// Returns `(after_action, filters.min_seq, anchor_ts)`. An unknown id is
    /// [`Error::UnknownAction`] — the observer never guesses a causal point.
    /// Without an action the filter point is the watermark at wait start and the
    /// quiet anchor is the wait start itself.
    fn action_anchor(
        &self,
        after_action: Option<ActionId>,
        started_at: u64,
    ) -> Result<(Option<ActionId>, u64, u64)> {
        match after_action {
            Some(action_id) => {
                let record = self
                    .inner
                    .actions
                    .get(action_id)
                    .ok_or(Error::UnknownAction(action_id))?;
                Ok((Some(action_id), record.seq, record.ts_ms))
            }
            None => Ok((None, self.watermark(), started_at)),
        }
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
        let inner = &self.inner;

        // (a) A wait on a window the observer never saw (or no longer tracks) is a
        // request error, not a timeout.
        if let Some(window_id) = plan.window_id {
            if !lock_state(inner).windows.contains_key(&window_id) {
                return Err(Error::UnknownWindow(window_id));
            }
        }

        // (b) Subscribe *before* seeding. An event that lands between the seed and
        // the first `changed()` still marks the generation seen here, so the wait
        // cannot miss a wakeup.
        let mut events = inner.events.subscribe();

        // (c) Announce the waiter for `pending_observation`; the guard is dropped on
        // resolution *and* on cancellation (client disconnect).
        let _guard =
            PendingGuard::register(inner, plan.window_id, plan.filters.min_seq, plan.started_at);

        // (d) Seed from the journal; every iteration absorbs whatever the journal
        // holds beyond `cursor`, so a seeded event is never counted twice.
        let mut accumulator = Accumulator::new(plan.filters.clone());
        let mut cursor = plan.filters.min_seq;

        loop {
            let generation = *events.borrow();
            let now = lock_clock(inner).now_ms();

            let (met, anchor) = {
                let state = lock_state(inner);
                for event in state.journal.since(cursor) {
                    if event.seq > cursor {
                        cursor = event.seq;
                    }
                    if !plan.filters.counts(event) {
                        continue;
                    }
                    let clip =
                        window_geometry(event.window_id.and_then(|id| state.windows.get(&id)));
                    accumulator.absorb(event, clip);
                }
                // Condition evaluation and quiet-deadline scheduling share this
                // anchor; if they disagreed, a quiet wait could wake early and
                // never resolve.
                let anchor = plan.anchor_ts.max(accumulator.last_commit_ts.unwrap_or(0));
                (
                    condition_met(plan.condition, &accumulator, plan.anchor_ts, now),
                    anchor,
                )
            };

            if met {
                // An event handled between the clock read and the state read could
                // have re-armed a quiet timer; re-evaluate instead of resolving on
                // stale evidence.
                if *events.borrow() != generation {
                    continue;
                }
            }

            let timed_out = if met {
                false
            } else if now >= plan.deadline_ms() {
                // Reaching the horizon *is* the condition for `Condition::Timeout`
                // (animation sampling), so it never reports `timed_out`.
                !matches!(plan.condition, WaitCondition::Timeout)
            } else {
                // (e) No polling: wait for the next event generation or the next
                // relevant deadline, whichever comes first. The sleep is re-created
                // every iteration so a forward clock re-anchor is honoured.
                let horizon = plan
                    .quiet_deadline_ms(anchor)
                    .map_or(plan.deadline_ms(), |quiet| quiet.min(plan.deadline_ms()));
                let sleep_until = lock_clock(inner).deadline(horizon);
                tokio::select! {
                    changed = events.changed() => {
                        if changed.is_err() {
                            // Unreachable while this task holds an `Arc<Inner>`
                            // (the sender lives in it); never spin on it either.
                            tracing::trace!("observer: event generation sender dropped");
                        }
                    }
                    () = tokio::time::sleep_until(sleep_until) => {}
                }
                continue;
            };

            let now_ms = lock_clock(inner).now_ms();
            let observation = {
                let state = lock_state(inner);
                let window_state = plan
                    .window_id
                    .and_then(|window_id| state.windows.get(&window_id))
                    .cloned();
                let ctx = ResolveContext {
                    window_id: plan.window_id,
                    after_action: plan.after_action,
                    started_at: plan.started_at,
                    now_ms,
                    watermark: state.watermark,
                    // A quiet condition carries its own threshold; the other
                    // conditions report evidence against the configured default.
                    // The single mapping lives on `WaitCondition`.
                    quiet_threshold_ms: plan
                        .condition
                        .quiet_threshold_ms(inner.config.default_quiet_ms),
                    window_state: window_state.as_ref(),
                    global_last_commit_seq: state.global_last_commit_seq(),
                    timed_out,
                };
                accumulator.resolve(&ctx)
            };
            tracing::trace!(
                window = ?plan.window_id,
                timed_out,
                elapsed_ms = observation.elapsed_ms,
                "observer: wait resolved"
            );
            return Ok(observation);
        }
    }

    // ---------------------------------------------------------------------- queries

    /// Point-in-time snapshot of all temporal state, for `list_windows`-adjacent
    /// queries that must not wait.
    pub fn snapshot(&self) -> ObserverSnapshot {
        let ts_ms = self.now_ms();
        let state = lock_state(&self.inner);
        ObserverSnapshot {
            seq: state.watermark,
            ts_ms,
            windows: state.windows.values().cloned().collect(),
            actions_len: self.inner.actions.len(),
            journal_len: state.journal.len(),
            events_dropped: state.journal.dropped(),
        }
    }

    /// Temporal state of one window (`None` once it is destroyed).
    pub fn window_state(&self, window_id: WindowId) -> Option<WindowTemporalState> {
        lock_state(&self.inner).windows.get(&window_id).cloned()
    }

    /// Window geometry when the observer knows it: `None` for an untracked window
    /// or before the first [`ObserverService::resync`] (which is the only source
    /// of geometry).
    ///
    /// Cheaper than [`ObserverService::window_state`] for callers that only need
    /// geometry: it clones just the [`Rect`] instead of the whole
    /// [`WindowTemporalState`] (including `last_damage`).
    pub fn window_geometry(&self, window_id: WindowId) -> Option<Rect> {
        lock_state(&self.inner)
            .windows
            .get(&window_id)
            .and_then(|window| window.geometry)
    }

    /// Current global event watermark (highest `seq` processed).
    pub fn watermark(&self) -> u64 {
        lock_state(&self.inner).watermark
    }

    /// Current time in the event `ts_ms` domain.
    pub fn now_ms(&self) -> u64 {
        lock_clock(&self.inner).now_ms()
    }

    /// Ids of the windows the observer currently tracks (ascending).
    pub fn window_ids(&self) -> Vec<WindowId> {
        lock_state(&self.inner).windows.keys().copied().collect()
    }

    /// `true` when `window_id` has been quiet for `quiet_ms` at `now_ms`.
    pub fn is_quiet(&self, window_id: WindowId, quiet_ms: u64) -> Option<bool> {
        let now_ms = self.now_ms();
        self.window_state(window_id)
            .map(|window| window.is_quiet(now_ms, quiet_ms))
    }
}

/// Convenience: window geometry for damage clipping, when the observer knows it.
pub(crate) fn window_geometry(state: Option<&WindowTemporalState>) -> Option<Rect> {
    state.and_then(|window| window.geometry)
}

/// Convenience: `Condition` → internal [`WaitCondition`].
pub(crate) fn wait_condition(condition: Condition) -> WaitCondition {
    match condition {
        Condition::Change => WaitCondition::Change,
        Condition::Quiet { quiet_ms } => WaitCondition::Quiet { quiet_ms },
        Condition::Timeout => WaitCondition::Timeout,
    }
}

#[cfg(test)]
mod tests {
    //! Tests that assert a clock-derived value (`now_ms()`, `record.ts_ms`,
    //! `snapshot().ts_ms`) run under `#[tokio::test(start_paused = true)]`: the
    //! observer's clock is the *real* monotonic clock in an unpaused test, so an
    //! exact equality read back after the fact would race with elapsed time (the
    //! frozen `tests/actions.rs` spec has exactly that shape, see CONTEXT.md
    //! Known Issues). Paused time freezes the clock at the last event `ts_ms`,
    //! which is what these tests assert.
    use super::*;
    use crate::state::WindowSnapshot;
    use adesk_core::{AppId, LaunchId};
    use std::time::Duration;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn region(rects: &[Rect]) -> Region {
        let mut region = Region::empty();
        for rect in rects {
            region.push(*rect);
        }
        region
    }

    fn created(seq: u64, ts_ms: u64, id: u64) -> RuntimeEvent {
        RuntimeEvent::WindowCreated {
            seq,
            ts_ms,
            window_id: WindowId(id),
            app_id: Some(AppId::from("test.app")),
            pid: Some(4242),
            launch_id: None,
            title: None,
        }
    }

    fn destroyed(seq: u64, ts_ms: u64, id: u64) -> RuntimeEvent {
        RuntimeEvent::WindowDestroyed {
            seq,
            ts_ms,
            window_id: WindowId(id),
        }
    }

    fn commit(seq: u64, ts_ms: u64, id: u64, commit_seq: u64, damage: &[Rect]) -> RuntimeEvent {
        RuntimeEvent::SurfaceCommit {
            seq,
            ts_ms,
            window_id: WindowId(id),
            commit_seq,
            damage: region(damage),
        }
    }

    fn title_changed(seq: u64, ts_ms: u64, id: u64) -> RuntimeEvent {
        RuntimeEvent::TitleChanged {
            seq,
            ts_ms,
            window_id: WindowId(id),
            title: Some("hello".to_owned()),
        }
    }

    fn popup_appeared(seq: u64, ts_ms: u64, id: u64, popup_id: u64) -> RuntimeEvent {
        RuntimeEvent::PopupAppeared {
            seq,
            ts_ms,
            window_id: WindowId(id),
            popup_id,
        }
    }

    fn app_launched(seq: u64, ts_ms: u64) -> RuntimeEvent {
        RuntimeEvent::AppLaunched {
            seq,
            ts_ms,
            launch_id: LaunchId(1),
            app_id: AppId::from("test.app"),
            pid: Some(4242),
        }
    }

    fn window_snapshot(id: u64, last_commit_seq: u64) -> WindowSnapshot {
        WindowSnapshot {
            window_id: WindowId(id),
            last_commit_seq,
            geometry: rect(0, 0, 1280, 800),
            popup_count: 0,
        }
    }

    fn state_snapshot(seq: u64, ts_ms: u64, windows: Vec<WindowSnapshot>) -> StateSnapshot {
        StateSnapshot {
            seq,
            ts_ms,
            windows,
        }
    }

    /// Counted kinds retained in the journal, in order.
    fn journal_kinds(observer: &ObserverService) -> Vec<(u64, CountedKind)> {
        lock_state(&observer.inner)
            .journal
            .since(0)
            .map(|event| (event.seq, event.kind.clone()))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn new_service_starts_empty_and_honours_config() {
        let observer = ObserverService::new();
        let snapshot = observer.snapshot();
        assert_eq!(
            snapshot,
            ObserverSnapshot {
                seq: 0,
                ts_ms: 0,
                windows: Vec::new(),
                actions_len: 0,
                journal_len: 0,
                events_dropped: 0,
            }
        );
        assert_eq!(observer.watermark(), 0);
        assert_eq!(observer.now_ms(), 0);
        assert_eq!(observer.window_ids(), Vec::new());
        assert_eq!(observer.window_state(WindowId(7)), None);
        assert_eq!(observer.is_quiet(WindowId(7), 10), None);

        // The configured journal capacity is what the journal actually uses.
        let observer = ObserverService::with_config(ObserverConfig {
            journal_capacity: 2,
            default_quiet_ms: DEFAULT_QUIET_MS,
        });
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&title_changed(2, 1, 7));
        observer.handle_event(&popup_appeared(3, 2, 7, 9));
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.journal_len, 2);
        assert_eq!(snapshot.events_dropped, 1);
        assert_eq!(snapshot.seq, 3);
    }

    #[test]
    fn window_geometry_reads_geometry_without_full_state() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        assert_eq!(
            observer.window_geometry(WindowId(7)),
            None,
            "no geometry before the first resync"
        );
        assert_eq!(
            observer.window_geometry(WindowId(99)),
            None,
            "untracked window"
        );

        observer.resync(state_snapshot(2, 10, vec![window_snapshot(7, 3)]));
        assert_eq!(
            observer.window_geometry(WindowId(7)),
            Some(rect(0, 0, 1280, 800))
        );
        assert_eq!(observer.window_geometry(WindowId(99)), None);
    }

    #[test]
    fn handle_event_commit_updates_temporal_state() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&commit(2, 40, 7, 5, &[rect(0, 0, 4, 4)]));

        let state = observer.window_state(WindowId(7)).expect("tracked");
        assert_eq!(state.window_id, WindowId(7));
        assert_eq!(state.last_commit_seq, 5);
        assert_eq!(state.last_commit_at, 40);
        assert_eq!(state.last_damage, region(&[rect(0, 0, 4, 4)]));
        assert_eq!(state.quiet_since, Some(40));
        assert_eq!(state.commit_count, 1);
        assert_eq!(state.last_meaningful_change_at, 40);
        assert!(!state.state_uncertain);
        assert_eq!(observer.watermark(), 2);
        assert_eq!(observer.snapshot().journal_len, 2);

        // Damage is replaced (not accumulated) and `last_commit_seq` never regresses.
        observer.handle_event(&commit(3, 50, 7, 4, &[rect(10, 10, 2, 2)]));
        let state = observer.window_state(WindowId(7)).expect("tracked");
        assert_eq!(state.last_commit_seq, 5, "max(commit_seq)");
        assert_eq!(state.last_commit_at, 50);
        assert_eq!(state.last_damage, region(&[rect(10, 10, 2, 2)]));
        assert_eq!(state.quiet_since, Some(50));
        assert_eq!(state.commit_count, 2);
    }

    #[test]
    fn handle_event_self_heals_unknown_windows() {
        let observer = ObserverService::new();
        observer.handle_event(&commit(1, 10, 7, 3, &[rect(0, 0, 1, 1)]));

        let state = observer
            .window_state(WindowId(7))
            .expect("created on demand");
        assert_eq!(state.last_commit_seq, 3);
        assert_eq!(state.commit_count, 1);
        assert_eq!(observer.watermark(), 1);

        observer.handle_event(&title_changed(2, 20, 8));
        assert_eq!(
            observer
                .window_state(WindowId(8))
                .expect("created on demand")
                .last_meaningful_change_at,
            20
        );
        assert_eq!(observer.window_ids(), vec![WindowId(7), WindowId(8)]);
    }

    #[test]
    fn handle_event_destroyed_drops_state_but_keeps_the_journal() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&destroyed(2, 10, 7));

        assert_eq!(observer.window_state(WindowId(7)), None);
        assert_eq!(observer.window_ids(), Vec::new());
        assert_eq!(observer.watermark(), 2);
        assert_eq!(
            journal_kinds(&observer),
            vec![(1, CountedKind::Created), (2, CountedKind::Destroyed)],
            "in-flight waiters must still be able to count the destruction"
        );
        assert_eq!(observer.snapshot().journal_len, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn app_launched_only_advances_the_watermark() {
        let observer = ObserverService::new();
        observer.handle_event(&app_launched(7, 70));

        assert_eq!(observer.watermark(), 7);
        assert_eq!(observer.now_ms(), 70);
        assert_eq!(observer.snapshot().journal_len, 0, "never counted");
        assert_eq!(observer.window_ids(), Vec::new());
    }

    #[test]
    fn handle_event_clears_state_uncertain_but_keeps_history() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&commit(2, 10, 7, 1, &[rect(0, 0, 4, 4)]));
        observer.handle_event(&created(3, 0, 8));
        observer.resync(state_snapshot(
            20,
            100,
            vec![window_snapshot(7, 1), window_snapshot(8, 0)],
        ));
        assert!(observer.window_state(WindowId(7)).unwrap().state_uncertain);
        assert!(observer.window_state(WindowId(8)).unwrap().state_uncertain);

        // A real event for the window clears the flag...
        observer.handle_event(&title_changed(21, 110, 7));
        let state = observer.window_state(WindowId(7)).expect("tracked");
        assert!(!state.state_uncertain);
        assert_eq!(state.commit_count, 1, "existing history is kept");
        assert_eq!(state.last_commit_seq, 1);
        assert_eq!(state.geometry, Some(rect(0, 0, 1280, 800)));

        // ...and so does a re-`WindowCreated` (existing state is not reset).
        observer.handle_event(&created(22, 120, 8));
        let state = observer.window_state(WindowId(8)).expect("tracked");
        assert!(!state.state_uncertain);
        assert_eq!(state.geometry, Some(rect(0, 0, 1280, 800)));
    }

    #[tokio::test(start_paused = true)]
    async fn resync_ignores_stale_snapshots() {
        let observer = ObserverService::new();
        observer.handle_event(&created(5, 50, 7));

        let report = observer.resync(state_snapshot(1, 10, Vec::new()));

        assert_eq!(
            report,
            ResyncReport {
                snapshot_seq: 5,
                ..ResyncReport::default()
            }
        );
        assert_eq!(observer.watermark(), 5, "never moves backwards");
        assert!(observer.window_state(WindowId(7)).is_some(), "no pruning");
        assert_eq!(observer.snapshot().journal_len, 1);
        assert_eq!(observer.now_ms(), 50, "clock never moves backwards");
    }

    #[tokio::test(start_paused = true)]
    async fn resync_adds_unknown_windows_with_a_synthetic_created() {
        let observer = ObserverService::new();
        let report = observer.resync(state_snapshot(10, 100, vec![window_snapshot(7, 3)]));

        assert_eq!(report.snapshot_seq, 10);
        assert_eq!(report.windows_added, vec![WindowId(7)]);
        assert_eq!(report.windows_removed, Vec::new());
        assert_eq!(report.marked_uncertain, vec![WindowId(7)]);
        assert_eq!(report.events_dropped, 0);

        let state = observer.window_state(WindowId(7)).expect("added");
        assert!(state.state_uncertain);
        assert_eq!(state.last_commit_seq, 3);
        assert_eq!(state.last_commit_at, 100, "best effort snapshot time");
        assert_eq!(state.quiet_since, Some(100));
        assert_eq!(state.commit_count, 0, "the snapshot proves no commit here");
        assert_eq!(state.geometry, Some(rect(0, 0, 1280, 800)));
        assert_eq!(journal_kinds(&observer), vec![(10, CountedKind::Created)]);
        assert_eq!(observer.watermark(), 10);
        assert_eq!(observer.now_ms(), 100, "clock re-anchored at the snapshot");
    }

    #[test]
    fn resync_adopts_advanced_commits_with_a_synthetic_commit() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&commit(2, 10, 7, 1, &[rect(0, 0, 4, 4)]));

        let report = observer.resync(state_snapshot(50, 500, vec![window_snapshot(7, 9)]));

        assert_eq!(report.windows_added, Vec::new());
        assert_eq!(report.marked_uncertain, vec![WindowId(7)]);
        assert_eq!(report.events_dropped, 2, "created + commit superseded");

        let state = observer.window_state(WindowId(7)).expect("tracked");
        assert_eq!(state.last_commit_seq, 9);
        assert_eq!(state.last_commit_at, 500);
        assert_eq!(state.quiet_since, Some(500));
        assert_eq!(state.commit_count, 2);
        assert!(state.state_uncertain);
        assert_eq!(
            journal_kinds(&observer),
            vec![(
                50,
                CountedKind::Commit {
                    commit_seq: 9,
                    damage: Region::empty(),
                }
            )],
            "the synthesized commit is what in-flight waiters count"
        );

        // An equal `last_commit_seq` is not a change: no synthetic commit.
        let report = observer.resync(state_snapshot(60, 600, vec![window_snapshot(7, 9)]));
        assert_eq!(report.events_dropped, 1);
        assert_eq!(observer.window_state(WindowId(7)).unwrap().commit_count, 2);
        assert_eq!(journal_kinds(&observer), Vec::new());
    }

    #[test]
    fn resync_removes_missing_windows_with_a_synthetic_destroyed() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let report = observer.resync(state_snapshot(20, 200, Vec::new()));

        assert_eq!(report.windows_removed, vec![WindowId(7)]);
        assert_eq!(report.windows_added, Vec::new());
        assert_eq!(report.marked_uncertain, Vec::new());
        assert_eq!(observer.window_state(WindowId(7)), None);
        assert_eq!(observer.window_ids(), Vec::new());
        assert_eq!(journal_kinds(&observer), vec![(20, CountedKind::Destroyed)]);
    }

    #[test]
    fn resync_prunes_the_journal_through_the_snapshot() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&commit(2, 10, 7, 1, &[rect(0, 0, 4, 4)]));
        observer.handle_event(&commit(3, 20, 7, 2, &[rect(0, 0, 4, 4)]));

        // A non-stale snapshot supersedes every retained event (they are all at or
        // below the watermark the snapshot carries).
        let report = observer.resync(state_snapshot(3, 30, vec![window_snapshot(7, 2)]));

        assert_eq!(report.events_dropped, 3, "seq 1..=3 superseded");
        assert_eq!(report.windows_added, Vec::new());
        assert_eq!(report.marked_uncertain, vec![WindowId(7)]);
        assert_eq!(
            journal_kinds(&observer),
            Vec::new(),
            "pruned, and no synthetic event for an unchanged commit sequence"
        );
        assert_eq!(observer.watermark(), 3);
        assert_eq!(observer.window_state(WindowId(7)).unwrap().commit_count, 2);
    }

    #[test]
    fn handle_event_and_resync_bump_the_generation() {
        let observer = ObserverService::new();
        let mut events = observer.inner.events.subscribe();
        assert!(!events.has_changed().expect("sender alive"));

        observer.handle_event(&created(1, 0, 7));
        assert!(events.has_changed().expect("sender alive"));

        events.borrow_and_update();
        assert!(!events.has_changed().expect("sender alive"));

        observer.resync(state_snapshot(5, 50, vec![window_snapshot(7, 0)]));
        assert!(events.has_changed().expect("sender alive"));
    }

    /// The inline twin of the frozen `tests/actions.rs::record_action_captures_watermark_and_clock`
    /// spec: with the clock pinned, the recorded timestamp is exactly the last
    /// event `ts_ms` (25) rather than "25 plus however long the test has been running".
    #[tokio::test(start_paused = true)]
    async fn record_action_captures_watermark_and_clock() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&title_changed(2, 25, 7));

        let id = observer.record_action(
            ActionKind::Click,
            Some(WindowId(7)),
            Some(Position::Normalized { x: 0.25, y: 0.75 }),
        );

        let record = observer.action(id).expect("recorded");
        assert_eq!(record.id, id);
        assert_eq!(record.kind, ActionKind::Click);
        assert_eq!(record.window_id, Some(WindowId(7)));
        assert_eq!(
            record.position,
            Some(Position::Normalized { x: 0.25, y: 0.75 })
        );
        assert_eq!(record.seq, 2, "watermark at record time");
        assert_eq!(record.ts_ms, 25, "clock at record time");
        assert_eq!(observer.action_seq(id), Some(2));
        assert_eq!(observer.snapshot().actions_len, 1);
        assert_eq!(observer.action_registry().len(), 1);
    }

    #[test]
    fn input_actions_stamp_last_input_at_on_the_target_window_only() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&title_changed(2, 15, 7));

        let id = observer.record_action(ActionKind::TypeText, Some(WindowId(7)), None);
        let ts_ms = observer.action(id).expect("recorded").ts_ms;

        let state = observer.window_state(WindowId(7)).expect("tracked");
        assert_eq!(state.last_input_at, Some(ts_ms));
        assert_eq!(
            state.last_meaningful_change_at, 15,
            "input is not a state change"
        );
        assert_eq!(
            observer.window_state(WindowId(8)),
            None,
            "other windows untouched"
        );
    }

    #[test]
    fn runtime_native_actions_leave_last_input_at_untouched() {
        let observer = ObserverService::new();
        let _ = observer.record_action(ActionKind::ActivateWindow, Some(WindowId(7)), None);

        let state = observer
            .window_state(WindowId(7))
            .expect("created on demand");
        assert_eq!(state.last_input_at, None, "not synthesized input");
        assert_eq!(state.commit_count, 0);

        let _ = observer.record_action(ActionKind::CloseWindow, Some(WindowId(7)), None);
        assert_eq!(
            observer.window_state(WindowId(7)).unwrap().last_input_at,
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn snapshot_reports_watermark_clock_and_counts() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&created(2, 0, 8));
        observer.handle_event(&commit(3, 30, 8, 1, &[rect(0, 0, 2, 2)]));
        let _ = observer.record_action(ActionKind::Click, Some(WindowId(8)), None);

        let snapshot = observer.snapshot();
        assert_eq!(snapshot.seq, 3);
        assert_eq!(snapshot.ts_ms, 30);
        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|window| window.window_id)
                .collect::<Vec<_>>(),
            vec![WindowId(7), WindowId(8)],
            "ascending by window id"
        );
        assert_eq!(snapshot.actions_len, 1);
        assert_eq!(snapshot.journal_len, 3);
        assert_eq!(snapshot.events_dropped, 0);
        assert_eq!(observer.is_quiet(WindowId(7), 250), Some(false));
        assert_eq!(
            observer.is_quiet(WindowId(8), 0),
            Some(true),
            "quiet_since is set"
        );
    }

    #[test]
    fn pending_guard_merges_refcounts_and_clears() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        let inner = Arc::clone(&observer.inner);

        let first = PendingGuard::register(&inner, Some(WindowId(7)), 5, 10);
        let duplicate = PendingGuard::register(&inner, Some(WindowId(7)), 5, 10);
        let earlier = PendingGuard::register(&inner, Some(WindowId(7)), 3, 20);

        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            Some(PendingObservation {
                since_seq: 3,
                started_at: 20
            }),
            "the earliest active waiter wins"
        );

        drop(earlier);
        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            Some(PendingObservation {
                since_seq: 5,
                started_at: 10
            })
        );

        // Same key, so the refcount keeps it alive until the last guard goes.
        drop(first);
        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            Some(PendingObservation {
                since_seq: 5,
                started_at: 10
            })
        );
        drop(duplicate);
        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            None
        );
        assert!(lock_state(&observer.inner).pending.is_empty(), "no leak");

        // A global (unfiltered) waiter has no window to annotate.
        let global = PendingGuard::register(&inner, None, 1, 2);
        assert!(lock_state(&observer.inner).pending.is_empty());
        drop(global);
        assert!(lock_state(&observer.inner).pending.is_empty());
    }

    #[test]
    fn pending_guard_drop_survives_a_destroyed_window() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        let guard = PendingGuard::register(&observer.inner, Some(WindowId(7)), 1, 0);
        observer.handle_event(&destroyed(2, 10, 7));

        drop(guard);

        assert!(lock_state(&observer.inner).pending.is_empty());
        assert_eq!(observer.window_state(WindowId(7)), None);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_on_unknown_or_destroyed_window_is_an_error() {
        let observer = ObserverService::new();
        let error = observer
            .wait_for_change(WaitSpec::new().window(WindowId(99)))
            .await
            .expect_err("unknown window");
        assert!(matches!(error, Error::UnknownWindow(WindowId(99))));

        observer.handle_event(&created(1, 0, 7));
        observer.handle_event(&destroyed(2, 10, 7));
        let error = observer
            .wait_for_change(WaitSpec::new().window(WindowId(7)))
            .await
            .expect_err("destroyed window");
        assert!(matches!(error, Error::UnknownWindow(WindowId(7))));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_quiet_and_observe_reject_unknown_actions() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let error = observer
            .wait_for_quiet(
                QuietSpec::new()
                    .window(WindowId(7))
                    .after_action(ActionId(9)),
            )
            .await
            .expect_err("unknown action");
        assert!(matches!(error, Error::UnknownAction(ActionId(9))));

        let error = observer
            .observe(
                ObserveSpec::new(Condition::Change)
                    .window(WindowId(7))
                    .after_action(ActionId(9)),
            )
            .await
            .expect_err("unknown action");
        assert!(matches!(error, Error::UnknownAction(ActionId(9))));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_change_times_out_with_an_observation() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let observation = observer
            .wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(300))
            .await
            .expect("known window");

        assert!(observation.timed_out, "a deadline is an observation");
        assert_eq!(observation.window_id, Some(WindowId(7)));
        assert_eq!(observation.commits, 0);
        assert_eq!(observation.changed_regions, Vec::new());
        assert_eq!(observation.elapsed_ms, 300);
        assert_eq!(observation.seq, 1, "watermark at resolution");
        assert!(observation.quiet, "quiet since the wait started");
    }

    #[tokio::test(start_paused = true)]
    async fn parked_waiter_resolves_when_paused_time_advances_past_the_deadline() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait =
            Box::pin(observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(300)));
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        // No event is ever fed: the waiter is parked on `sleep_until(deadline)` in
        // the event `ts_ms` domain, so advancing paused time past the deadline must
        // resolve it with a timeout observation.
        tokio::time::advance(Duration::from_millis(300)).await;

        let observation = wait.await.expect("known window");
        assert!(
            observation.timed_out,
            "the deadline expired: {observation:?}"
        );
        assert_eq!(
            observation.elapsed_ms, 300,
            "timeout_ms in the ts_ms domain"
        );
        assert_eq!(observation.commits, 0);
        assert_eq!(
            observer.now_ms(),
            300,
            "the event clock reached the deadline"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_quiet_resolves_at_exactly_anchor_plus_quiet_ms() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let observation = observer
            .wait_for_quiet(
                QuietSpec::new()
                    .window(WindowId(7))
                    .quiet_ms(100)
                    .timeout_ms(5_000),
            )
            .await
            .expect("known window");

        assert!(!observation.timed_out);
        assert!(observation.quiet);
        assert_eq!(observation.commits, 0);
        assert_eq!(observation.elapsed_ms, 100);
        assert_eq!(observer.now_ms(), 100, "resolved at the quiet deadline");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_change_resolves_on_an_event_fed_while_parked() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)),
        );

        // Poll the wait once: it validates, subscribes, seeds, finds no change and
        // parks on the generation watcher. `yield_now` is ready immediately, so the
        // biased `select!` returns here with the wait still pending.
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        // The event is fed *after* the wait parked: the generation bump must wake
        // it even though the seed already ran (this is the no-lost-wakeup case).
        observer.handle_event(&commit(2, 40, 7, 1, &[rect(0, 0, 4, 4)]));

        let observation = wait.await.expect("known window");
        assert!(!observation.timed_out);
        assert_eq!(observation.commits, 1);
        assert_eq!(observation.last_commit_seq, 1);
        assert_eq!(observation.changed_regions, vec![rect(0, 0, 4, 4)]);
        assert_eq!(observation.elapsed_ms, 40);
    }

    #[tokio::test(start_paused = true)]
    async fn non_quiet_wait_quiet_flag_honours_the_configured_default() {
        // Feed a non-commit change 40 ms after the wait starts: the `quiet`
        // evidence flag of a non-quiet wait must be measured against the
        // configured `ObserverConfig::default_quiet_ms`, not the crate constant.
        async fn quiet_flag_of_change_wait(default_quiet_ms: u64) -> bool {
            let observer = ObserverService::with_config(ObserverConfig {
                journal_capacity: DEFAULT_JOURNAL_CAPACITY,
                default_quiet_ms,
            });
            observer.handle_event(&created(1, 0, 7));

            let mut wait = Box::pin(
                observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)),
            );
            tokio::select! {
                biased;
                observation = &mut wait => panic!("resolved before any event: {observation:?}"),
                () = tokio::task::yield_now() => {}
            }
            observer.handle_event(&title_changed(2, 40, 7));
            wait.await.expect("known window").quiet
        }

        assert!(
            quiet_flag_of_change_wait(5).await,
            "40 ms >= the configured 5 ms default: quiet"
        );
        assert!(
            !quiet_flag_of_change_wait(DEFAULT_QUIET_MS).await,
            "40 ms < the 250 ms crate constant: not quiet"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_quiet_seeds_from_the_journal_after_an_action() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        let action = observer.record_action(ActionKind::Click, Some(WindowId(7)), None);
        // The commit happens *after* the action but *before* the wait starts: the
        // waiter must still count it and anchor the quiet timer on it.
        observer.handle_event(&commit(2, 30, 7, 1, &[rect(0, 0, 4, 4)]));

        let observation = observer
            .wait_for_quiet(
                QuietSpec::new()
                    .window(WindowId(7))
                    .quiet_ms(100)
                    .timeout_ms(5_000)
                    .after_action(action),
            )
            .await
            .expect("known window");

        assert!(!observation.timed_out);
        assert!(observation.quiet);
        assert_eq!(observation.after_action, Some(action));
        assert_eq!(observation.commits, 1, "seeded from the journal");
        assert_eq!(observer.now_ms(), 130, "anchor(30) + quiet_ms(100)");
        assert_eq!(observation.elapsed_ms, 100);
    }

    #[tokio::test(start_paused = true)]
    async fn observe_timeout_condition_never_reports_timed_out() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let observation = observer
            .observe(
                ObserveSpec::new(Condition::Timeout)
                    .window(WindowId(7))
                    .timeout_ms(100),
            )
            .await
            .expect("known window");

        assert!(!observation.timed_out, "the horizon *is* the condition");
        assert_eq!(observation.elapsed_ms, 100);
        assert_eq!(observer.now_ms(), 100);
        assert!(!observation.quiet, "100 ms < the 250 ms evidence threshold");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_change_resolves_on_a_lifecycle_event() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.observe(
                ObserveSpec::new(Condition::Change)
                    .window(WindowId(7))
                    .timeout_ms(5_000),
            ),
        );
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        observer.handle_event(&title_changed(2, 25, 7));

        let observation = wait.await.expect("known window");
        assert!(!observation.timed_out);
        assert_eq!(observation.commits, 0);
        assert!(observation.title_changed);
        assert_eq!(observation.elapsed_ms, 25);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_quiet_rearms_on_every_commit() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.wait_for_quiet(
                QuietSpec::new()
                    .window(WindowId(7))
                    .quiet_ms(100)
                    .timeout_ms(5_000),
            ),
        );
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        // A commit re-arms the quiet timer; only the last one plus 100 ms resolves.
        observer.handle_event(&commit(2, 60, 7, 1, &[rect(0, 0, 2, 2)]));
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved too early: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }
        observer.handle_event(&commit(3, 120, 7, 2, &[rect(0, 0, 2, 2)]));

        let observation = wait.await.expect("known window");
        assert!(!observation.timed_out);
        assert_eq!(observation.commits, 2);
        assert_eq!(observer.now_ms(), 220, "120 + quiet_ms(100)");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_change_ignores_the_seeded_lifecycle_event() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        // The `WindowCreated` event is already in the journal; a wait started after
        // it must not resolve on it.
        let observation = observer
            .wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(50))
            .await
            .expect("known window");

        assert!(observation.timed_out);
        assert_eq!(observation.commits, 0);
        assert_eq!(observation.new_windows, Vec::new());
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_waiters_all_resolve_on_the_same_event() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));
        let spec = WaitSpec::new().window(WindowId(7)).timeout_ms(5_000);

        let feeder = async {
            // Give the three waiters a chance to register, then feed one commit.
            tokio::task::yield_now().await;
            observer.handle_event(&commit(2, 40, 7, 1, &[rect(0, 0, 4, 4)]));
        };
        let (a, b, c, ()) = tokio::join!(
            observer.wait_for_change(spec.clone()),
            observer.wait_for_change(spec.clone()),
            observer.wait_for_change(spec),
            feeder,
        );

        for observation in [a, b, c] {
            let observation = observation.expect("known window");
            assert!(!observation.timed_out);
            assert_eq!(observation.commits, 1);
            assert_eq!(observation.elapsed_ms, 40);
        }
        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            None,
            "all guards released"
        );
        assert!(lock_state(&observer.inner).pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_waiter_clears_pending_observation() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.wait_for_quiet(
                QuietSpec::new()
                    .window(WindowId(7))
                    .quiet_ms(50)
                    .timeout_ms(5_000),
            ),
        );
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }
        assert!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation
                .is_some(),
            "visible while the wait is in flight"
        );

        drop(wait);

        assert_eq!(
            observer
                .window_state(WindowId(7))
                .unwrap()
                .pending_observation,
            None
        );
        assert!(lock_state(&observer.inner).pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn pending_waiter_resolves_when_its_window_is_destroyed() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)),
        );
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        observer.handle_event(&destroyed(2, 10, 7));

        let observation = wait.await.expect("known window");
        assert!(!observation.timed_out, "destruction resolves the wait");
        assert_eq!(observation.destroyed_windows, vec![WindowId(7)]);
        assert_eq!(observation.window_id, Some(WindowId(7)));
        assert_eq!(observation.elapsed_ms, 10);
        assert_eq!(observer.window_state(WindowId(7)), None);
    }

    #[tokio::test(start_paused = true)]
    async fn resync_wakes_pending_waiters_with_synthetic_events() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut filtered = Box::pin(
            observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)),
        );
        let mut global = Box::pin(observer.wait_for_change(WaitSpec::new().timeout_ms(60_000)));
        tokio::select! {
            biased;
            observation = &mut filtered => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }
        tokio::select! {
            biased;
            observation = &mut global => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        // Window 7 disappeared and window 8 appeared while the broadcast lagged.
        let report = observer.resync(state_snapshot(20, 200, vec![window_snapshot(8, 2)]));
        assert_eq!(report.windows_removed, vec![WindowId(7)]);
        assert_eq!(report.windows_added, vec![WindowId(8)]);

        let observation = filtered.await.expect("known window");
        assert!(!observation.timed_out);
        assert_eq!(observation.destroyed_windows, vec![WindowId(7)]);
        assert_eq!(observation.seq, 20);

        let observation = global.await.expect("unfiltered");
        assert!(!observation.timed_out);
        assert_eq!(observation.new_windows, vec![WindowId(8)]);
        assert_eq!(observation.seq, 20);
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_uses_the_event_clock_domain_for_elapsed_ms() {
        let observer = ObserverService::new();
        observer.handle_event(&created(1, 0, 7));

        let mut wait = Box::pin(
            observer.wait_for_change(WaitSpec::new().window(WindowId(7)).timeout_ms(60_000)),
        );
        tokio::select! {
            biased;
            observation = &mut wait => panic!("resolved before any event: {observation:?}"),
            () = tokio::task::yield_now() => {}
        }

        // Real tokio time advances without any event: the wait must still be bound
        // by the *event* clock, which only moves when events arrive.
        tokio::time::advance(Duration::from_millis(10)).await;
        observer.handle_event(&commit(2, 90, 7, 1, &[rect(0, 0, 1, 1)]));

        let observation = wait.await.expect("known window");
        assert_eq!(
            observation.elapsed_ms, 90,
            "elapsed is measured in the ts_ms domain"
        );
    }
}
