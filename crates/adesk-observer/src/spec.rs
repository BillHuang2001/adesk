//! Wait specifications mirroring `docs/protocol.md` §5.4.
//!
//! One spec type per AGP method. The image-related parameters (`include_image`,
//! `region`, `max_dimension`) are deliberately **not** here: rendering happens in
//! `adesk-server` after the wait resolves (`docs/architecture.md` §6), so the
//! observer has no opinion about pixels.

use adesk_core::{ActionId, WindowId};

use crate::{DEFAULT_QUIET_MS, DEFAULT_TIMEOUT_MS};

/// Parameters of `wait_for_change` (AGP §5.4).
///
/// Resolves on the first counted event: a `SurfaceCommit` with
/// `commit_seq > since_commit` (when a filter is given) or any window lifecycle
/// event (create/destroy/activate/title/focus/popup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitSpec {
    /// Restrict counting to one window. `None` = global observation.
    pub window_id: Option<WindowId>,
    /// Only commits with `commit_seq` strictly greater than this count.
    /// Lifecycle events are not commit-numbered and always count.
    pub since_commit: Option<u64>,
    /// Hard upper bound of the wait; on expiry the observation has
    /// `timed_out: true`.
    pub timeout_ms: u64,
}

impl Default for WaitSpec {
    fn default() -> Self {
        Self {
            window_id: None,
            since_commit: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl WaitSpec {
    /// Spec with protocol defaults (`timeout_ms = 5000`, no filters).
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the wait to `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Counts only commits with `commit_seq > since_commit`.
    pub fn since_commit(mut self, since_commit: u64) -> Self {
        self.since_commit = Some(since_commit);
        self
    }

    /// Overrides the timeout.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

/// Parameters of `wait_for_quiet` (AGP §5.4).
///
/// Resolves once `quiet_ms` have elapsed with no counted surface commit in the
/// filtered window, measured from `max(after_action.ts_ms, last counted commit)`.
/// Any counted commit re-arms the timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietSpec {
    /// Restrict counting to one window. `None` = global observation.
    pub window_id: Option<WindowId>,
    /// Quiescence threshold in milliseconds.
    pub quiet_ms: u64,
    /// Hard upper bound of the wait.
    pub timeout_ms: u64,
    /// Only events with `seq > action_seq` count; also anchors the quiet timer
    /// when no counted commit follows the action.
    pub after_action: Option<ActionId>,
}

impl Default for QuietSpec {
    fn default() -> Self {
        Self {
            window_id: None,
            quiet_ms: DEFAULT_QUIET_MS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            after_action: None,
        }
    }
}

impl QuietSpec {
    /// Spec with protocol defaults (`quiet_ms = 250`, `timeout_ms = 5000`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the wait to `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Overrides the quiescence threshold.
    pub fn quiet_ms(mut self, quiet_ms: u64) -> Self {
        self.quiet_ms = quiet_ms;
        self
    }

    /// Overrides the timeout.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Anchors counting (and the quiet timer) at a recorded action.
    pub fn after_action(mut self, action_id: ActionId) -> Self {
        self.after_action = Some(action_id);
        self
    }
}

/// Parameters of `observe` (AGP §5.4).
///
/// Same machinery as the other two waits, driven by an explicit [`Condition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserveSpec {
    /// Restrict counting to one window. `None` = global observation.
    pub window_id: Option<WindowId>,
    /// Only events with `seq > action_seq` count.
    pub after_action: Option<ActionId>,
    /// Resolution condition.
    pub until: Condition,
    /// Hard upper bound of the wait.
    pub timeout_ms: u64,
}

impl Default for ObserveSpec {
    fn default() -> Self {
        Self {
            window_id: None,
            after_action: None,
            until: Condition::Quiet {
                quiet_ms: DEFAULT_QUIET_MS,
            },
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl ObserveSpec {
    /// Spec that waits for `condition` with protocol defaults.
    pub fn new(until: Condition) -> Self {
        Self {
            until,
            ..Self::default()
        }
    }

    /// Restricts the observation to `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Anchors counting at a recorded action.
    pub fn after_action(mut self, action_id: ActionId) -> Self {
        self.after_action = Some(action_id);
        self
    }

    /// Overrides the timeout.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

/// The `Condition` union of AGP §5.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    /// Return on the first counted surface commit or window lifecycle event.
    Change,
    /// Return once `quiet_ms` have elapsed without a counted commit.
    Quiet {
        /// Quiescence threshold in milliseconds.
        quiet_ms: u64,
    },
    /// Wait the full timeout and report what accumulated (animation sampling).
    ///
    /// Never reports `timed_out: true`: reaching the sampling horizon *is* the
    /// condition.
    Timeout,
}

impl Condition {
    /// Quiet threshold carried by the condition.
    ///
    /// `Condition::Quiet` returns its own threshold; `Change`/`Timeout` return
    /// [`crate::DEFAULT_QUIET_MS`]. The service resolves the `quiet` evidence
    /// flag of a non-quiet condition against `ObserverConfig::default_quiet_ms`
    /// (which defaults to this constant).
    pub fn quiet_threshold_ms(&self) -> u64 {
        match self {
            Condition::Quiet { quiet_ms } => *quiet_ms,
            Condition::Change | Condition::Timeout => DEFAULT_QUIET_MS,
        }
    }
}
