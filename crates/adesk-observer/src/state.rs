//! Per-window temporal state, the query snapshot, and the resync input.
//!
//! These types are the observer's *public memory*: `adesk-server` uses
//! [`ObserverSnapshot`] to answer temporal queries without waiting, and
//! [`StateSnapshot`] to resync after a broadcast `Lagged` error
//! (`docs/architecture.md` §1).

use adesk_core::{Rect, Region, WindowId};

/// A waiter currently registered on a window.
///
/// Mirrors `pending_observation` in `docs/architecture.md` §6: the earliest active
/// waiter's filter point, so the inspector can show which windows are being
/// observed and the journal can be kept long enough to seed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingObservation {
    /// Lowest filter `seq` among active waiters on this window.
    pub since_seq: u64,
    /// Clock timestamp when the earliest active waiter started.
    pub started_at: u64,
}

/// Temporal state of one window.
///
/// Updated by [`crate::ObserverService::handle_event`]; `last_*` fields are
/// absolute (not filter-relative) and describe the *current* state, while
/// filter-relative counts are accumulated per waiter from the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTemporalState {
    /// The window this state belongs to.
    pub window_id: WindowId,
    /// Highest `commit_seq` observed (also the absolute value reported by
    /// `Observation::last_commit_seq` for window-filtered waits).
    pub last_commit_seq: u64,
    /// `ts_ms` of the most recent counted commit; 0 when the window never committed.
    pub last_commit_at: u64,
    /// Damage of the most recent commit (replaced, not accumulated). Per-waiter
    /// unions are accumulated from the journal.
    pub last_damage: Region,
    /// `ts_ms` of the most recent state-changing event (commit, lifecycle, title,
    /// focus, popup).
    pub last_meaningful_change_at: u64,
    /// `ts_ms` of the most recent input action targeting this window; `None` when
    /// no input has been injected into it.
    pub last_input_at: Option<u64>,
    /// `ts_ms` from which the window has been continuously quiet — i.e. the
    /// timestamp of the most recent counted commit; `None` if it never committed.
    /// Per-waiter `quiet_ms` thresholds are applied against this value.
    pub quiet_since: Option<u64>,
    /// Number of commits observed since the window was created.
    pub commit_count: u64,
    /// Earliest active waiter on this window, if any.
    pub pending_observation: Option<PendingObservation>,
    /// Window geometry once known (from a resync snapshot); used to clip damage
    /// regions to the window (`docs/protocol.md` §4).
    pub geometry: Option<Rect>,
    /// Set when a broadcast lag made this window's state incomplete; cleared by
    /// the next event for the window. Waits still answer, with evidence the agent
    /// can interpret.
    pub state_uncertain: bool,
}

impl WindowTemporalState {
    /// Fresh state for a newly created window at `created_seq`/`ts_ms`.
    pub(crate) fn created(window_id: WindowId, ts_ms: u64) -> Self {
        Self {
            window_id,
            last_commit_seq: 0,
            last_commit_at: 0,
            last_damage: Region::empty(),
            last_meaningful_change_at: ts_ms,
            last_input_at: None,
            quiet_since: None,
            commit_count: 0,
            pending_observation: None,
            geometry: None,
            state_uncertain: false,
        }
    }

    /// `true` when the window has been quiet for at least `quiet_ms` at `now_ms`.
    pub fn is_quiet(&self, now_ms: u64, quiet_ms: u64) -> bool {
        self.quiet_since
            .is_some_and(|since| now_ms.saturating_sub(since) >= quiet_ms)
    }
}

/// Point-in-time view of the whole observer, for query methods that must not wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverSnapshot {
    /// Global event watermark at snapshot time.
    pub seq: u64,
    /// Clock timestamp at snapshot time.
    pub ts_ms: u64,
    /// Per-window state, ordered by window id.
    pub windows: Vec<WindowTemporalState>,
    /// Number of recorded actions (see `ActionRegistry::len`).
    pub actions_len: usize,
    /// Number of counted events currently retained in the journal.
    pub journal_len: usize,
    /// Counted events dropped because the journal overflowed its capacity.
    pub events_dropped: u64,
}

/// Resync input: the observer's view of the compositor's `QueryState` reply.
///
/// `adesk-compositor` answers `RuntimeCommand::QueryState` with its own snapshot
/// type; `adesk-server` translates that into this shape (the observer must not
/// depend on the compositor) and calls
/// [`crate::ObserverService::resync`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StateSnapshot {
    /// Global event sequence watermark the compositor has reached.
    pub seq: u64,
    /// Compositor clock at snapshot time (`ts_ms` domain).
    pub ts_ms: u64,
    /// Windows that currently exist, ordered by window id.
    pub windows: Vec<WindowSnapshot>,
}

/// One window inside a [`StateSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSnapshot {
    /// The window id.
    pub window_id: WindowId,
    /// Highest commit sequence the compositor has for this window.
    pub last_commit_seq: u64,
    /// Current window geometry (window-relative origin, usually `(0,0)`).
    pub geometry: Rect,
    /// Number of mapped popups.
    pub popup_count: u32,
}

/// What a [`crate::ObserverService::resync`] call changed.
///
/// Purely diagnostic (tracing + inspector); correctness of in-flight waiters is
/// guaranteed by the resync algorithm itself, not by this report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResyncReport {
    /// Watermark the observer adopted.
    pub snapshot_seq: u64,
    /// Windows the snapshot introduced (missed `WindowCreated`).
    pub windows_added: Vec<WindowId>,
    /// Windows the snapshot proved destroyed (missed `WindowDestroyed`).
    pub windows_removed: Vec<WindowId>,
    /// Windows whose state is now flagged uncertain (all affected windows).
    pub marked_uncertain: Vec<WindowId>,
    /// Journal entries dropped because they are older than the snapshot.
    pub events_dropped: usize,
}
