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

    /// The configured correlation window.
    pub fn timeout(&self) -> Duration {
        self.timeout
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

    /// Drops launches older than the timeout, returning them for logging.
    pub fn expire(&mut self) -> Vec<LaunchRecord> {
        let now_ms = self.clock.now_ms();
        let timeout = self.timeout;
        let mut expired = Vec::new();
        self.pending.retain(|pending| {
            if pending.is_expired(now_ms, timeout) {
                expired.push(pending.record.clone());
                false
            } else {
                true
            }
        });
        expired
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use adesk_core::{AppId, LaunchId};

    use super::*;

    /// Manually advanced clock so correlation deadlines are deterministic.
    #[derive(Debug, Default)]
    struct FakeClock {
        now_ms: AtomicU64,
    }

    impl FakeClock {
        fn new(now_ms: u64) -> Arc<FakeClock> {
            Arc::new(FakeClock {
                now_ms: AtomicU64::new(now_ms),
            })
        }

        fn advance(&self, ms: u64) {
            self.now_ms.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.now_ms.load(Ordering::SeqCst)
        }
    }

    fn new_correlator(clock: &Arc<FakeClock>) -> Correlator {
        let clock: Arc<dyn Clock> = clock.clone();
        Correlator::new(clock)
    }

    fn correlator_with(timeout: Duration, clock: &Arc<FakeClock>) -> Correlator {
        let clock: Arc<dyn Clock> = clock.clone();
        Correlator::with_timeout(timeout, clock)
    }

    fn app(id: &str, name: &str, startup_wm_class: Option<&str>) -> AppInfo {
        AppInfo {
            id: AppId::from(id),
            name: name.to_owned(),
            icon: None,
            exec: Some("/bin/true".to_owned()),
            terminal: false,
            categories: Vec::new(),
            startup_wm_class: startup_wm_class.map(str::to_owned),
            dbus_activatable: false,
            hidden: false,
            no_display: false,
            try_exec: None,
        }
    }

    fn record(launch_id: u64, app_id: &str, pid: Option<i32>, started_at_ms: u64) -> LaunchRecord {
        LaunchRecord {
            launch_id: LaunchId(launch_id),
            app_id: AppId::from(app_id),
            pid,
            started_at_ms,
        }
    }

    fn window<'a>(
        window_id: u64,
        pid: Option<i32>,
        app_id: Option<&'a str>,
        title: Option<&'a str>,
    ) -> WindowCandidate<'a> {
        WindowCandidate {
            window_id: WindowId(window_id),
            pid,
            app_id,
            title,
        }
    }

    fn correlated(outcome: &CorrelationOutcome) -> &Correlation {
        match outcome {
            CorrelationOutcome::Correlated(correlation) => correlation,
            other => panic!("expected a correlation, got {other:?}"),
        }
    }

    #[test]
    fn new_uses_default_timeout_and_starts_empty() {
        let clock = FakeClock::new(0);
        let correlator = new_correlator(&clock);
        assert_eq!(correlator.timeout(), DEFAULT_CORRELATION_TIMEOUT);
        assert_eq!(correlator.pending(), 0);
    }

    #[test]
    fn with_timeout_stores_the_explicit_timeout() {
        let clock = FakeClock::new(0);
        let correlator = correlator_with(Duration::from_millis(250), &clock);
        assert_eq!(correlator.timeout(), Duration::from_millis(250));
    }

    #[test]
    fn record_launch_counts_pending_launches() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "app.one", Some(1), 0),
            &app("app.one", "One", None),
        );
        correlator.record_launch(
            record(2, "app.two", Some(2), 0),
            &app("app.two", "Two", None),
        );
        assert_eq!(correlator.pending(), 2);
    }

    #[test]
    fn pid_match_correlates() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", Some(42), 0),
            &app("org.mozilla.firefox", "Firefox", Some("firefox")),
        );

        let outcome = correlator.correlate(&window(7, Some(42), Some("firefox"), Some("New Tab")));
        let correlation = correlated(&outcome);
        assert_eq!(correlation.window_id, WindowId(7));
        assert_eq!(correlation.evidence, CorrelationEvidence::Pid);
        assert_eq!(correlation.launch.launch_id, LaunchId(1));
        assert_eq!(
            outcome.launch().unwrap().app_id,
            AppId::from("org.mozilla.firefox")
        );
    }

    #[test]
    fn pid_tier_beats_startup_wm_class() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        // Substring/`StartupWMClass` candidate, recorded later.
        correlator.record_launch(
            record(2, "com.example.thing", Some(99), 5),
            &app("com.example.thing", "Thing", None),
        );
        // Pid candidate, recorded earlier and also matching `StartupWMClass`.
        correlator.record_launch(
            record(1, "org.other.thing", Some(7), 0),
            &app("org.other.thing", "Unrelated", Some("thing")),
        );

        let outcome =
            correlator.correlate(&window(5, Some(7), Some("thing"), Some("Thing window")));
        let correlation = correlated(&outcome);
        assert_eq!(correlation.evidence, CorrelationEvidence::Pid);
        assert_eq!(correlation.launch.launch_id, LaunchId(1));
    }

    #[test]
    fn pid_match_requires_both_sides_known() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        // Launch pid unknown, window pid known: tier 1 must not fire.
        correlator.record_launch(
            record(1, "org.mozilla.firefox", None, 0),
            &app("org.mozilla.firefox", "Firefox", Some("firefox")),
        );
        let outcome = correlator.correlate(&window(7, Some(42), Some("firefox"), None));
        assert_eq!(
            correlated(&outcome).evidence,
            CorrelationEvidence::StartupWmClass
        );

        // Window pid unknown, launch pid known: tier 1 must not fire either.
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(2, "org.mozilla.firefox", Some(42), 0),
            &app("org.mozilla.firefox", "Firefox", None),
        );
        let outcome = correlator.correlate(&window(8, None, Some("org.mozilla.firefox"), None));
        assert_eq!(
            correlated(&outcome).evidence,
            CorrelationEvidence::AppIdOrTitleSubstring
        );
    }

    #[test]
    fn startup_wm_class_matches_case_insensitively() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.gnome.Nautilus", Some(1), 0),
            &app("org.gnome.Nautilus", "Files", Some("Org.Gnome.Nautilus")),
        );

        let outcome = correlator.correlate(&window(3, None, Some("org.gnome.nautilus"), None));
        let correlation = correlated(&outcome);
        assert_eq!(correlation.evidence, CorrelationEvidence::StartupWmClass);
        assert_eq!(correlation.launch.launch_id, LaunchId(1));
    }

    #[test]
    fn startup_wm_class_tier_beats_substring() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        // Substring candidate (name needle), recorded later.
        correlator.record_launch(
            record(2, "com.example.thing", None, 5),
            &app("com.example.thing", "thing", None),
        );
        // `StartupWMClass` candidate, recorded earlier.
        correlator.record_launch(
            record(1, "org.other.thing", None, 0),
            &app("org.other.thing", "Unrelated", Some("thing")),
        );

        let outcome = correlator.correlate(&window(4, None, Some("thing"), None));
        let correlation = correlated(&outcome);
        assert_eq!(correlation.evidence, CorrelationEvidence::StartupWmClass);
        assert_eq!(correlation.launch.launch_id, LaunchId(1));
    }

    #[test]
    fn startup_wm_class_skipped_when_absent_or_empty() {
        let clock = FakeClock::new(0);

        // Absent on the entry; the app id/name needles do not match either.
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "com.example.unrelated", None, 0),
            &app("com.example.unrelated", "Unrelated", None),
        );
        assert_eq!(
            correlator.correlate(&window(1, None, Some("thing"), None)),
            CorrelationOutcome::Uncorrelated
        );

        // Empty on the entry, empty on the window: must not match each other.
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(2, "com.example.unrelated", None, 0),
            &app("com.example.unrelated", "Unrelated", Some("")),
        );
        assert_eq!(
            correlator.correlate(&window(2, None, Some(""), None)),
            CorrelationOutcome::Uncorrelated
        );
    }

    #[test]
    fn substring_matches_full_desktop_file_id() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", None, 0),
            &app("org.mozilla.firefox", "Totally Unrelated", None),
        );

        let outcome = correlator.correlate(&window(9, None, Some("org.mozilla.firefox"), None));
        assert_eq!(
            correlated(&outcome).evidence,
            CorrelationEvidence::AppIdOrTitleSubstring
        );
    }

    #[test]
    fn substring_matches_last_dot_segment() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", None, 0),
            &app("org.mozilla.firefox", "Totally Unrelated", None),
        );

        let outcome = correlator.correlate(&window(9, None, Some("firefox"), Some("New Tab")));
        let correlation = correlated(&outcome);
        assert_eq!(
            correlation.evidence,
            CorrelationEvidence::AppIdOrTitleSubstring
        );
        assert_eq!(correlation.launch.launch_id, LaunchId(1));
    }

    #[test]
    fn substring_matches_entry_name_case_insensitively_in_title() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "com.example.zzz", None, 0),
            &app("com.example.zzz", "Firefox", None),
        );

        let outcome = correlator.correlate(&window(11, None, None, Some("FIREFOX — New Tab")));
        assert_eq!(
            correlated(&outcome).evidence,
            CorrelationEvidence::AppIdOrTitleSubstring
        );
    }

    #[test]
    fn substring_ignores_empty_needles() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(record(1, "", None, 0), &app("", "", None));

        assert_eq!(
            correlator.correlate(&window(1, None, Some("anything"), Some("anything"))),
            CorrelationOutcome::Uncorrelated
        );
        assert_eq!(correlator.pending(), 1);
    }

    #[test]
    fn substring_ignores_windows_without_app_id_and_title() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", None, 0),
            &app("org.mozilla.firefox", "Firefox", None),
        );

        assert_eq!(
            correlator.correlate(&window(1, None, None, None)),
            CorrelationOutcome::Uncorrelated
        );
    }

    #[test]
    fn most_recently_started_launch_wins_within_a_tier() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        // Recorded first but started later: started_at_ms dominates both
        // insertion order and the smaller launch id.
        correlator.record_launch(
            record(1, "app.one", Some(5), 500),
            &app("app.one", "One", None),
        );
        correlator.record_launch(
            record(2, "app.two", Some(5), 100),
            &app("app.two", "Two", None),
        );

        let outcome = correlator.correlate(&window(1, Some(5), None, None));
        assert_eq!(correlated(&outcome).launch.launch_id, LaunchId(1));
    }

    #[test]
    fn larger_launch_id_breaks_started_at_ties() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(4, "app.one", Some(5), 100),
            &app("app.one", "One", None),
        );
        correlator.record_launch(
            record(7, "app.two", Some(5), 100),
            &app("app.two", "Two", None),
        );

        let outcome = correlator.correlate(&window(1, Some(5), None, None));
        assert_eq!(correlated(&outcome).launch.launch_id, LaunchId(7));
    }

    #[test]
    fn stale_launch_does_not_match_after_the_timeout() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", Some(42), 0),
            &app("org.mozilla.firefox", "Firefox", Some("firefox")),
        );

        clock.advance(DEFAULT_CORRELATION_TIMEOUT.as_millis() as u64 + 1);
        assert_eq!(
            correlator.correlate(&window(1, Some(42), Some("firefox"), None)),
            CorrelationOutcome::Uncorrelated
        );
        assert_eq!(correlator.pending(), 0);
    }

    #[test]
    fn expiry_boundary_is_exclusive_at_the_timeout() {
        let clock = FakeClock::new(0);
        let mut correlator = correlator_with(Duration::from_millis(100), &clock);
        correlator.record_launch(record(1, "app", Some(1), 0), &app("app", "App", None));

        // Age == timeout: still pending, still matches.
        clock.advance(100);
        let outcome = correlator.correlate(&window(1, Some(1), None, None));
        assert_eq!(correlated(&outcome).launch.launch_id, LaunchId(1));
        assert_eq!(correlator.pending(), 1);

        // Age == timeout + 1ms: expired.
        clock.advance(1);
        assert_eq!(
            correlator.correlate(&window(2, Some(1), None, None)),
            CorrelationOutcome::Uncorrelated
        );
        assert_eq!(correlator.pending(), 0);
    }

    #[test]
    fn correlate_prunes_expired_launches_before_matching() {
        let clock = FakeClock::new(0);
        let mut correlator = correlator_with(Duration::from_millis(100), &clock);
        // Would match by pid, but is stale by the time the window maps.
        correlator.record_launch(record(1, "app", Some(1), 0), &app("app", "App", None));

        clock.advance(200);
        // Fresh launch that does not match this window.
        correlator.record_launch(
            record(2, "other", Some(2), 200),
            &app("other", "Other", None),
        );

        assert_eq!(
            correlator.correlate(&window(1, Some(1), None, None)),
            CorrelationOutcome::Uncorrelated
        );
        assert_eq!(correlator.pending(), 1);
    }

    #[test]
    fn one_launch_correlates_several_windows() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            record(1, "org.mozilla.firefox", Some(42), 0),
            &app("org.mozilla.firefox", "Firefox", Some("firefox")),
        );

        let first = correlator.correlate(&window(1, Some(42), Some("firefox"), None));
        let second = correlator.correlate(&window(2, None, Some("firefox"), Some("New Tab")));

        assert_eq!(correlated(&first).launch.launch_id, LaunchId(1));
        assert_eq!(correlated(&second).launch.launch_id, LaunchId(1));
        assert_eq!(correlated(&first).window_id, WindowId(1));
        assert_eq!(correlated(&second).window_id, WindowId(2));
        assert_eq!(correlator.pending(), 1);
    }

    #[test]
    fn uncorrelated_when_no_launch_is_pending() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        let outcome = correlator.correlate(&window(1, Some(42), Some("firefox"), Some("Firefox")));
        assert_eq!(outcome, CorrelationOutcome::Uncorrelated);
        assert!(outcome.launch().is_none());
    }

    #[test]
    fn expire_returns_expired_records_in_insertion_order() {
        let clock = FakeClock::new(0);
        let mut correlator = correlator_with(Duration::from_millis(100), &clock);
        correlator.record_launch(
            record(1, "app.one", Some(1), 0),
            &app("app.one", "One", None),
        );
        clock.advance(50);
        correlator.record_launch(
            record(2, "app.two", Some(2), 50),
            &app("app.two", "Two", None),
        );

        // Nothing expired yet.
        assert!(correlator.expire().is_empty());
        assert_eq!(correlator.pending(), 2);

        // Only the first launch is older than the timeout.
        clock.advance(60);
        assert_eq!(correlator.expire(), vec![record(1, "app.one", Some(1), 0)]);
        assert_eq!(correlator.pending(), 1);

        // Now the second one too, and the list is empty afterwards.
        clock.advance(100);
        assert_eq!(correlator.expire(), vec![record(2, "app.two", Some(2), 50)]);
        assert!(correlator.expire().is_empty());
        assert_eq!(correlator.pending(), 0);
    }

    #[test]
    fn zero_timeout_expires_on_the_next_millisecond() {
        let clock = FakeClock::new(0);
        let mut correlator = correlator_with(Duration::ZERO, &clock);
        correlator.record_launch(record(1, "app", Some(1), 0), &app("app", "App", None));

        // Age 0 is not strictly greater than a zero timeout.
        assert!(correlator.expire().is_empty());
        assert_eq!(correlator.pending(), 1);

        clock.advance(1);
        assert_eq!(correlator.expire(), vec![record(1, "app", Some(1), 0)]);
        assert_eq!(correlator.pending(), 0);
    }

    #[test]
    fn correlation_outcome_launch_accessor_reports_the_record() {
        let clock = FakeClock::new(0);
        let mut correlator = new_correlator(&clock);
        let launched = record(3, "app.one", Some(1), 0);
        correlator.record_launch(launched.clone(), &app("app.one", "One", None));

        let outcome = correlator.correlate(&window(1, Some(1), None, None));
        assert_eq!(outcome.launch(), Some(&launched));
        assert_eq!(CorrelationOutcome::Uncorrelated.launch(), None);
    }
}
