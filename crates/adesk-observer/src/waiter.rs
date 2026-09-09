//! Waiter machinery: filters, the per-waiter accumulator, and condition evaluation.
//!
//! A waiter is a pure function of `(journal seed + live events + clock)`:
//!
//! 1. [`Filters`] decides which counted events belong to the wait.
//! 2. [`Accumulator`] folds them into the filter-relative numbers of
//!    [`adesk_core::Observation`].
//! 3. [`WaitCondition`] decides when to resolve; [`Accumulator::resolve`] then
//!    combines the accumulated counts with the per-window state.
//!
//! The service owns the waiting loop (`service.rs`); everything here is
//! synchronous and unit-testable without tokio.

// Phase 1 architecture skeleton: method bodies are `todo!()`, so the fields they
// will read look unused. Remove this allow together with the last `todo!()` in
// this file (see `CONTEXT.md` → Status).
#![allow(dead_code)]

use adesk_core::{ActionId, Observation, Region, WindowId};

use crate::journal::{CountedEvent, CountedKind};
use crate::state::WindowTemporalState;
use crate::DEFAULT_QUIET_MS;

/// Which counted events a waiter counts (`docs/protocol.md` §5.4).
///
/// `seq > min_seq` (from `after_action`) and `window_id` (when filtered) always
/// apply. `since_commit` additionally restricts *commits* to
/// `commit_seq > since_commit`; lifecycle, title, focus and popup events are not
/// commit-numbered and always count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filters {
    /// Window filter, `None` = global.
    pub(crate) window_id: Option<WindowId>,
    /// Strict lower bound on the event `seq` (the `after_action` point).
    pub(crate) min_seq: u64,
    /// Strict lower bound on `commit_seq`, commits only.
    pub(crate) since_commit: Option<u64>,
}

impl Filters {
    /// Builds the filter triple.
    pub(crate) fn new(
        window_id: Option<WindowId>,
        min_seq: u64,
        since_commit: Option<u64>,
    ) -> Self {
        Self {
            window_id,
            min_seq,
            since_commit,
        }
    }

    /// `true` when this event belongs to the wait.
    pub(crate) fn counts(&self, event: &CountedEvent) -> bool {
        if event.seq <= self.min_seq {
            return false;
        }
        if let Some(window_id) = self.window_id {
            if event.window_id != Some(window_id) {
                return false;
            }
        }
        if let (Some(min_commit), CountedKind::Commit { commit_seq, .. }) = (self.since_commit, &event.kind) {
            return *commit_seq > min_commit;
        }
        true
    }
}

/// What ends a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitCondition {
    /// First counted event (`Condition::Change` / `wait_for_change`).
    Change,
    /// No counted commit for `quiet_ms` (`Condition::Quiet` / `wait_for_quiet`).
    Quiet {
        /// Quiescence threshold.
        quiet_ms: u64,
    },
    /// Wait the full timeout and report what accumulated (`Condition::Timeout`).
    Timeout,
}

impl WaitCondition {
    /// The quiet threshold used for the `quiet` evidence flag of the observation.
    pub(crate) fn quiet_threshold_ms(self) -> u64 {
        match self {
            WaitCondition::Quiet { quiet_ms } => quiet_ms,
            WaitCondition::Change | WaitCondition::Timeout => DEFAULT_QUIET_MS,
        }
    }
}

/// A fully resolved wait request handed to the service loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitPlan {
    /// Window filter (echoed into `Observation::window_id`).
    pub(crate) window_id: Option<WindowId>,
    /// Action filter (echoed into `Observation::after_action`).
    pub(crate) after_action: Option<ActionId>,
    /// Event filter.
    pub(crate) filters: Filters,
    /// What ends the wait.
    pub(crate) condition: WaitCondition,
    /// Hard bound of the wait, milliseconds.
    pub(crate) timeout_ms: u64,
    /// Quiet-timer anchor: the action timestamp when `after_action` is set,
    /// otherwise the wait start.
    pub(crate) anchor_ts: u64,
    /// Wait start in the clock domain (`Observation::elapsed_ms` origin).
    pub(crate) started_at: u64,
}

impl WaitPlan {
    /// `ts_ms` at which the wait must give up.
    pub(crate) fn deadline_ms(&self) -> u64 {
        self.started_at.saturating_add(self.timeout_ms)
    }

    /// Quiet deadline implied by the condition, or `None` for non-quiet waits.
    ///
    /// `anchor` is `max(plan.anchor_ts, last counted commit ts)`.
    pub(crate) fn quiet_deadline_ms(&self, anchor: u64) -> Option<u64> {
        match self.condition {
            WaitCondition::Quiet { quiet_ms } => Some(anchor.saturating_add(quiet_ms)),
            WaitCondition::Change | WaitCondition::Timeout => None,
        }
    }
}

/// Per-waiter fold of counted events into filter-relative observation numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Accumulator {
    /// Filter applied by the service before absorbing.
    pub(crate) filters: Filters,
    /// `ts_ms` of the most recent counted commit (`None` = none in filter).
    pub(crate) last_commit_ts: Option<u64>,
    /// Highest `commit_seq` in the filter.
    pub(crate) last_commit_seq: u64,
    /// Number of counted commits.
    pub(crate) commits: u64,
    /// Union of commit damage (clipped to geometry when known).
    pub(crate) damage: Region,
    /// Counted `WindowCreated` ids, in order.
    pub(crate) new_windows: Vec<WindowId>,
    /// Counted `WindowDestroyed` ids, in order.
    pub(crate) destroyed_windows: Vec<WindowId>,
    /// Counted popup ids that appeared, in order.
    pub(crate) popups_appeared: Vec<u64>,
    /// Counted popup ids that disappeared, in order.
    pub(crate) popups_disappeared: Vec<u64>,
    /// `Some(true)` = focus moved to the filtered window, `Some(false)` = focus
    /// moved away from it (latest transition wins); `None` = no focus info.
    pub(crate) focus_changed: Option<bool>,
    /// Any counted `TitleChanged` in the filter.
    pub(crate) title_changed: bool,
    /// Number of counted events (any kind).
    pub(crate) events: u64,
}

impl Accumulator {
    /// Empty accumulator for `filters`.
    pub(crate) fn new(filters: Filters) -> Self {
        Self {
            filters,
            last_commit_ts: None,
            last_commit_seq: 0,
            commits: 0,
            damage: Region::empty(),
            new_windows: Vec::new(),
            destroyed_windows: Vec::new(),
            popups_appeared: Vec::new(),
            popups_disappeared: Vec::new(),
            focus_changed: None,
            title_changed: false,
            events: 0,
        }
    }

    /// Folds one already-counted event into the observation numbers.
    ///
    /// `clip` is the window geometry when known: damage is clipped to it
    /// (`docs/protocol.md` §4); unknown geometry leaves damage untouched.
    pub(crate) fn absorb(&mut self, event: &CountedEvent, clip: Option<adesk_core::Rect>) {
        let _ = (self, event, clip);
        todo!("Phase 2: update counters/lists/regions for each CountedKind")
    }

    /// `true` when at least one event counted (`Condition::Change`).
    pub(crate) fn counted(&self) -> bool {
        self.events > 0
    }

    /// Quiet anchor: the most recent counted commit, falling back to `anchor_ts`.
    pub(crate) fn quiet_anchor(&self, anchor_ts: u64) -> u64 {
        self.last_commit_ts.unwrap_or(anchor_ts)
    }

    /// Produces the protocol `Observation` at resolution time.
    pub(crate) fn resolve(&self, ctx: &ResolveContext<'_>) -> Observation {
        let _ = (self, ctx);
        todo!("Phase 2: build Observation from accumulated counts + per-window state")
    }
}

/// Everything `resolve` needs beyond the accumulator.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolveContext<'a> {
    /// Window filter, echoed into the observation.
    pub(crate) window_id: Option<WindowId>,
    /// Action filter, echoed into the observation.
    pub(crate) after_action: Option<ActionId>,
    /// Wait start (`elapsed_ms` origin).
    pub(crate) started_at: u64,
    /// Clock at resolution.
    pub(crate) now_ms: u64,
    /// Global event watermark at resolution.
    pub(crate) watermark: u64,
    /// Quiet threshold for the `quiet` evidence flag.
    pub(crate) quiet_threshold_ms: u64,
    /// Per-window state at resolution (absent for a destroyed/unfiltered window).
    pub(crate) window_state: Option<&'a WindowTemporalState>,
    /// Global last commit sequence (max over all windows), used when unfiltered.
    pub(crate) global_last_commit_seq: u64,
    /// `true` when the deadline expired before the condition was met.
    pub(crate) timed_out: bool,
}

/// `true` when the condition is satisfied at `now_ms`.
///
/// - `Change`: any counted event.
/// - `Quiet { quiet_ms }`: `now_ms - max(anchor_ts, last counted commit) >= quiet_ms`.
/// - `Timeout`: never (the deadline is the condition).
pub(crate) fn condition_met(
    condition: WaitCondition,
    accumulator: &Accumulator,
    anchor_ts: u64,
    now_ms: u64,
) -> bool {
    let _ = (condition, accumulator, anchor_ts, now_ms);
    todo!("Phase 2: implement the three condition predicates")
}
