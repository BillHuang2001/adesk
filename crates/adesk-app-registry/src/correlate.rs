//! Launch ↔ window correlation (`docs/architecture.md` §7).
//!
//! After a successful launch the server records the [`LaunchRecord`] with the
//! [`Correlator`]. When a toplevel maps, the server feeds a [`WindowCandidate`]
//! (window id, pid, xdg `app_id`, title) and either gets a
//! [`CorrelationOutcome::Correlated`] result — the window's `app_id` may be set and
//! announced — or [`CorrelationOutcome::Uncorrelated`], which is *reported*, never
//! guessed: the window keeps `app_id: null`.
//!
//! Evidence tiers, evaluated in order; the first tier with a match wins:
//!
//! 1. [`CorrelationEvidence::Pid`] — launch pid equals the window pid (both known).
//! 2. [`CorrelationEvidence::StartupWmClass`] — the entry's `StartupWMClass` equals
//!    the window's xdg `app_id` (case-insensitive).
//! 3. [`CorrelationEvidence::AppIdOrTitleSubstring`] — the window's `app_id` or
//!    title contains the desktop-file id, its last dot-segment, or the entry's
//!    localized `Name` (case-insensitive; empty needles are ignored).
//!
//! Within the winning tier the most recently started launch wins (ties: larger
//! `launch_id`). Pending launches older than the timeout are pruned before every
//! match, so a window never correlates with a stale launch. A launch stays pending
//! for its whole timeout, so several windows of one launch can correlate.
//!
//! The correlator and the registry MUST share one [`Clock`] instance — expiry
//! compares `now_ms` with [`LaunchRecord::started_at_ms`].

use std::sync::Arc;
use std::time::Duration;

use adesk_core::{AppInfo, WindowId};

use crate::clock::Clock;
use crate::launch::LaunchRecord;

/// Default correlation window (`docs/architecture.md` §7).
pub const DEFAULT_CORRELATION_TIMEOUT: Duration = Duration::from_secs(10);

/// What the compositor knows about a toplevel when it maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCandidate<'a> {
    /// The newly mapped window.
    pub window_id: WindowId,
    /// Client pid reported by the Wayland client, when known.
    pub pid: Option<i32>,
    /// xdg-toplevel `app_id`, when the client set one.
    pub app_id: Option<&'a str>,
    /// Toplevel title, when the client set one.
    pub title: Option<&'a str>,
}

/// Which rule matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorrelationEvidence {
    /// Exact pid match.
    Pid,
    /// `StartupWMClass` equals the window's xdg `app_id`.
    StartupWmClass,
    /// Window `app_id`/title contains the app id, its last segment, or the name.
    AppIdOrTitleSubstring,
}

/// A successful correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correlation {
    /// The window that mapped.
    pub window_id: WindowId,
    /// The launch it was attributed to.
    pub launch: LaunchRecord,
    /// The rule that matched.
    pub evidence: CorrelationEvidence,
}

/// Result of correlating one window.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorrelationOutcome {
    /// A pending launch matched; the window may be stamped with `app_id`.
    Correlated(Correlation),
    /// No pending launch matched within the timeout; report it, do not guess.
    Uncorrelated,
}

impl CorrelationOutcome {
    /// The launch record when correlated.
    pub fn launch(&self) -> Option<&LaunchRecord> {
        match self {
            CorrelationOutcome::Correlated(correlation) => Some(&correlation.launch),
            CorrelationOutcome::Uncorrelated => None,
        }
    }
}

// stub: pending launches awaiting a matching window.
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PendingLaunch {
    record: LaunchRecord,
    startup_wm_class: Option<String>,
    name: String,
}

/// Associates newly mapped windows with recent launches.
///
/// Owned by the server's event pump (single-threaded per runtime); no internal
/// locking. See the module docs for the matching rules.
#[derive(Debug)]
pub struct Correlator {
    // stub: populated by the implementation.
    #[allow(dead_code)]
    timeout: Duration,
    #[allow(dead_code)]
    clock: Arc<dyn Clock>,
    #[allow(dead_code)]
    pending: Vec<PendingLaunch>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl Correlator {
    /// Correlator with [`DEFAULT_CORRELATION_TIMEOUT`] using `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Correlator {
        todo!("stub: implementation phase")
    }

    /// Correlator with an explicit timeout, using `clock`.
    pub fn with_timeout(timeout: Duration, clock: Arc<dyn Clock>) -> Correlator {
        todo!("stub: implementation phase")
    }

    /// The configured correlation window.
    pub fn timeout(&self) -> Duration {
        todo!("stub: implementation phase")
    }

    /// Registers a successful launch; `app` supplies `StartupWMClass` and `Name`.
    pub fn record_launch(&mut self, record: LaunchRecord, app: &AppInfo) {
        todo!("stub: implementation phase")
    }

    /// Correlates a mapped window; prunes expired launches first.
    pub fn correlate(&mut self, window: &WindowCandidate<'_>) -> CorrelationOutcome {
        todo!("stub: implementation phase")
    }

    /// Drops launches older than the timeout, returning them for logging.
    pub fn expire(&mut self) -> Vec<LaunchRecord> {
        todo!("stub: implementation phase")
    }

    /// Number of launches still awaiting correlation.
    pub fn pending(&self) -> usize {
        todo!("stub: implementation phase")
    }
}
