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

/// Evidence tiers in evaluation order; the first tier with a match wins.
const TIERS: [CorrelationEvidence; 3] = [
    CorrelationEvidence::Pid,
    CorrelationEvidence::StartupWmClass,
    CorrelationEvidence::AppIdOrTitleSubstring,
];

/// The matcher implementing one evidence tier.
fn matcher_for(evidence: CorrelationEvidence) -> fn(&PendingLaunch, &WindowCandidate<'_>) -> bool {
    match evidence {
        CorrelationEvidence::Pid => matches_pid,
        CorrelationEvidence::StartupWmClass => matches_startup_wm_class,
        CorrelationEvidence::AppIdOrTitleSubstring => matches_app_id_or_title,
    }
}

/// Tier 1: exact pid match, only when both sides report a pid.
fn matches_pid(pending: &PendingLaunch, window: &WindowCandidate<'_>) -> bool {
    matches!(
        (pending.record.pid, window.pid),
        (Some(launch_pid), Some(window_pid)) if launch_pid == window_pid
    )
}

/// Tier 2: `StartupWMClass` equals the window's xdg `app_id`, case-insensitively.
///
/// Skipped when either side is absent or empty.
fn matches_startup_wm_class(pending: &PendingLaunch, window: &WindowCandidate<'_>) -> bool {
    let wm_class = match pending.startup_wm_class.as_deref() {
        Some(class) if !class.is_empty() => class,
        _ => return false,
    };
    let app_id = match window.app_id {
        Some(app_id) if !app_id.is_empty() => app_id,
        _ => return false,
    };
    wm_class.to_lowercase() == app_id.to_lowercase()
}

/// Tier 3: the window's `app_id` or title contains one of the needles.
///
/// Needles are the desktop-file id, its last dot-segment and the entry `Name`;
/// empty needles are ignored, as are windows that expose neither `app_id` nor title.
fn matches_app_id_or_title(pending: &PendingLaunch, window: &WindowCandidate<'_>) -> bool {
    let haystacks: Vec<String> = [window.app_id, window.title]
        .into_iter()
        .flatten()
        .map(str::to_lowercase)
        .collect();
    if haystacks.is_empty() {
        return false;
    }

    let app_id = pending.record.app_id.as_str();
    let last_segment = app_id.rsplit('.').next().unwrap_or(app_id);
    [app_id, last_segment, pending.name.as_str()]
        .into_iter()
        .filter(|needle| !needle.is_empty())
        .any(|needle| {
            let needle = needle.to_lowercase();
            haystacks.iter().any(|haystack| haystack.contains(&needle))
        })
}

/// A launch awaiting a matching window, plus the entry fields used by the
/// `StartupWMClass`/substring tiers.
#[derive(Debug, Clone)]
struct PendingLaunch {
    record: LaunchRecord,
    startup_wm_class: Option<String>,
    name: String,
}

impl PendingLaunch {
    /// Whether the launch outlived the correlation window.
    ///
    /// A launch is expired only when its age is *strictly greater* than the
    /// timeout, so a window mapping exactly at the deadline still correlates.
    /// `saturating_sub` keeps a non-monotonic clock from panicking.
    fn is_expired(&self, now_ms: u64, timeout: Duration) -> bool {
        Duration::from_millis(now_ms.saturating_sub(self.record.started_at_ms)) > timeout
    }
}

/// Associates newly mapped windows with recent launches.
///
/// Owned by the server's event pump (single-threaded per runtime); no internal
/// locking. See the module docs for the matching rules.
#[derive(Debug)]
pub struct Correlator {
    timeout: Duration,
    clock: Arc<dyn Clock>,
    pending: Vec<PendingLaunch>,
}

impl Correlator {
    /// Correlator with [`DEFAULT_CORRELATION_TIMEOUT`] using `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Correlator {
        Correlator::with_timeout(DEFAULT_CORRELATION_TIMEOUT, clock)
    }

    /// Correlator with an explicit timeout, using `clock`.
    pub fn with_timeout(timeout: Duration, clock: Arc<dyn Clock>) -> Correlator {
        Correlator {
            timeout,
            clock,
            pending: Vec::new(),
        }
    }

    /// Registers a successful launch; `app` supplies `StartupWMClass` and `Name`.
    pub fn record_launch(&mut self, record: LaunchRecord, app: &AppInfo) {
        self.pending.push(PendingLaunch {
            record,
            startup_wm_class: app
                .startup_wm_class
                .clone()
                .filter(|class| !class.is_empty()),
            name: app.name.clone(),
        });
    }

    /// Correlates a mapped window; prunes expired launches first.
    pub fn correlate(&mut self, window: &WindowCandidate<'_>) -> CorrelationOutcome {
        self.prune_expired();
        for evidence in TIERS {
            if let Some(matched) = self.best_match(window, matcher_for(evidence)) {
                return CorrelationOutcome::Correlated(Correlation {
                    window_id: window.window_id,
                    launch: matched.record.clone(),
                    evidence,
                });
            }
        }
        CorrelationOutcome::Uncorrelated
    }

    /// Number of launches still awaiting correlation.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Drops launches older than the timeout (pruned before every match).
    fn prune_expired(&mut self) {
        let now_ms = self.clock.now_ms();
        let timeout = self.timeout;
        self.pending
            .retain(|pending| !pending.is_expired(now_ms, timeout));
    }

    /// The pending launch that best matches within one tier: most recently
    /// started wins, ties break to the larger `launch_id`.
    ///
    /// The launch is deliberately left pending — several windows of one launch
    /// may correlate.
    fn best_match(
        &self,
        window: &WindowCandidate<'_>,
        matches: fn(&PendingLaunch, &WindowCandidate<'_>) -> bool,
    ) -> Option<&PendingLaunch> {
        self.pending
            .iter()
            .filter(|pending| matches(pending, window))
            .max_by_key(|pending| (pending.record.started_at_ms, pending.record.launch_id))
    }
}
