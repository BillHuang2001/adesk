//! Integration tests for launch ↔ window correlation.
//!
//! Public API only: evidence-tier ordering, tie-breaks, timeout expiry and the
//! `pending`/`expire` bookkeeping, all driven by the shared `support::FakeClock`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use adesk_app_registry::{
    CorrelationEvidence, CorrelationOutcome, Correlator, DEFAULT_CORRELATION_TIMEOUT,
};
use adesk_core::{LaunchId, WindowId};

use support::{app_info, as_clock, correlation, fake_clock, launch_record, window, FakeClock};

fn new_correlator(clock: &Arc<FakeClock>) -> Correlator {
    Correlator::new(as_clock(clock))
}

fn new_correlator_with(timeout: Duration, clock: &Arc<FakeClock>) -> Correlator {
    Correlator::with_timeout(timeout, as_clock(clock))
}

// --- evidence tiers --------------------------------------------------------

#[test]
fn pid_evidence_beats_wm_class_and_substring() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "app.alpha", Some(10), 0),
        &app_info("app.alpha", "Alpha", Some("alpha-wm")),
    );
    // Newer launch that matches the window by wm class *and* substring.
    correlator.record_launch(
        launch_record(2, "app.beta", None, 1),
        &app_info("app.beta", "Beta", Some("beta-wm")),
    );

    let outcome = correlator.correlate(&window(7, Some(10), Some("beta-wm"), Some("Beta window")));
    let matched = correlation(&outcome);
    assert_eq!(matched.window_id, WindowId(7));
    assert_eq!(matched.launch.launch_id, LaunchId(1));
    assert_eq!(matched.evidence, CorrelationEvidence::Pid);
    assert_eq!(
        correlator.pending(),
        2,
        "correlation never consumes a launch"
    );
}

#[test]
fn startup_wm_class_evidence_beats_substring() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "app.alpha", None, 0),
        &app_info("app.alpha", "Alpha", Some("beta-wm")),
    );
    // Newer launch whose name would also match the window's title.
    correlator.record_launch(
        launch_record(2, "app.beta", None, 5),
        &app_info("app.beta", "Beta", None),
    );

    let outcome = correlator.correlate(&window(3, None, Some("beta-wm"), Some("Beta window")));
    let matched = correlation(&outcome);
    assert_eq!(matched.launch.launch_id, LaunchId(1));
    assert_eq!(matched.evidence, CorrelationEvidence::StartupWmClass);
}

#[test]
fn wm_class_evidence_is_case_insensitive_and_skips_empty_values() {
    let clock = fake_clock(0);
    let mut case_insensitive = new_correlator(&clock);
    case_insensitive.record_launch(
        launch_record(1, "app.one", None, 0),
        &app_info("app.one", "One", Some("One-WM")),
    );
    let outcome = case_insensitive.correlate(&window(1, None, Some("one-wm"), None));
    assert_eq!(
        correlation(&outcome).evidence,
        CorrelationEvidence::StartupWmClass
    );

    // An empty `StartupWMClass` never matches, even against an empty app_id.
    let mut empty = new_correlator(&clock);
    empty.record_launch(
        launch_record(2, "app.two", None, 0),
        &app_info("app.two", "Two", Some("")),
    );
    assert_eq!(
        empty.correlate(&window(1, None, Some(""), None)),
        CorrelationOutcome::Uncorrelated
    );
}

#[test]
fn substring_evidence_matches_id_last_segment_and_name() {
    let clock = fake_clock(0);
    let cases: &[(Option<&str>, Option<&str>, bool)] = &[
        (Some("org.example.editor"), None, true),
        (Some("ORG.EXAMPLE.EDITOR"), None, true),
        (Some("editor"), None, true),
        (Some("my-editor-window"), None, true),
        (None, Some("Untitled — Editor"), true),
        (None, Some("unrelated"), false),
        (Some("com.other.thing"), Some("unrelated"), false),
    ];
    for (app_id, title, expected) in cases {
        let mut correlator = new_correlator(&clock);
        correlator.record_launch(
            launch_record(1, "org.example.editor", None, 0),
            &app_info("org.example.editor", "Editor", None),
        );
        let outcome = correlator.correlate(&window(1, None, *app_id, *title));
        assert_eq!(
            matches!(outcome, CorrelationOutcome::Correlated(_)),
            *expected,
            "app_id={app_id:?} title={title:?}"
        );
        if *expected {
            assert_eq!(
                correlation(&outcome).evidence,
                CorrelationEvidence::AppIdOrTitleSubstring
            );
        }
    }
}

#[test]
fn window_without_app_id_or_title_never_matches() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "app.one", None, 0),
        &app_info("app.one", "One", Some("one-wm")),
    );

    assert_eq!(
        correlator.correlate(&window(1, None, None, None)),
        CorrelationOutcome::Uncorrelated
    );
    assert_eq!(
        correlator.correlate(&window(2, None, Some(""), None)),
        CorrelationOutcome::Uncorrelated
    );
    assert_eq!(correlator.pending(), 1);
}

// --- tie-breaks ------------------------------------------------------------

#[test]
fn most_recently_started_launch_wins_within_a_tier() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "app.old", None, 0),
        &app_info("app.old", "Old", Some("shared-wm")),
    );
    correlator.record_launch(
        launch_record(2, "app.new", None, 50),
        &app_info("app.new", "New", Some("shared-wm")),
    );

    let outcome = correlator.correlate(&window(1, None, Some("shared-wm"), None));
    let matched = correlation(&outcome);
    assert_eq!(matched.launch.launch_id, LaunchId(2));
    assert_eq!(matched.launch.started_at_ms, 50);
    assert_eq!(matched.evidence, CorrelationEvidence::StartupWmClass);
}

#[test]
fn larger_launch_id_breaks_started_at_ties() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    // Insertion order is deliberately the reverse of the launch-id order.
    correlator.record_launch(
        launch_record(2, "app.two", None, 10),
        &app_info("app.two", "Two", Some("shared-wm")),
    );
    correlator.record_launch(
        launch_record(1, "app.one", None, 10),
        &app_info("app.one", "One", Some("shared-wm")),
    );

    let outcome = correlator.correlate(&window(1, None, Some("shared-wm"), None));
    assert_eq!(correlation(&outcome).launch.launch_id, LaunchId(2));
}

// --- timeout / expiry ------------------------------------------------------

#[test]
fn correlate_prunes_expired_launches_before_matching() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator_with(Duration::from_millis(100), &clock);
    correlator.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", None),
    );

    clock.set(101);
    assert_eq!(
        correlator.correlate(&window(1, Some(10), None, None)),
        CorrelationOutcome::Uncorrelated
    );
    assert_eq!(correlator.pending(), 0, "the stale launch was pruned");
}

#[test]
fn a_window_at_the_exact_deadline_still_correlates() {
    let clock = fake_clock(0);
    let mut at_deadline = new_correlator_with(Duration::from_millis(100), &clock);
    at_deadline.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", None),
    );
    clock.set(100);
    let outcome = at_deadline.correlate(&window(1, Some(10), None, None));
    assert_eq!(correlation(&outcome).evidence, CorrelationEvidence::Pid);

    let later = fake_clock(0);
    let mut past_deadline = new_correlator_with(Duration::from_millis(100), &later);
    past_deadline.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", None),
    );
    later.set(101);
    assert_eq!(
        past_deadline.correlate(&window(1, Some(10), None, None)),
        CorrelationOutcome::Uncorrelated
    );
}

#[test]
fn expire_returns_records_in_insertion_order_and_prunes_them() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator_with(Duration::from_millis(100), &clock);
    correlator.record_launch(
        launch_record(1, "app.one", Some(1), 0),
        &app_info("app.one", "One", None),
    );
    clock.advance(50);
    correlator.record_launch(
        launch_record(2, "app.two", Some(2), 50),
        &app_info("app.two", "Two", None),
    );

    assert!(correlator.expire().is_empty(), "nothing is old enough yet");
    assert_eq!(correlator.pending(), 2);

    clock.advance(60);
    assert_eq!(
        correlator.expire(),
        vec![launch_record(1, "app.one", Some(1), 0)]
    );
    assert_eq!(correlator.pending(), 1);

    clock.advance(100);
    assert_eq!(
        correlator.expire(),
        vec![launch_record(2, "app.two", Some(2), 50)]
    );
    assert!(correlator.expire().is_empty());
    assert_eq!(correlator.pending(), 0);
}

#[test]
fn zero_timeout_expires_on_the_next_millisecond() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator_with(Duration::ZERO, &clock);
    correlator.record_launch(
        launch_record(1, "app.one", Some(1), 0),
        &app_info("app.one", "One", None),
    );

    assert!(
        correlator.expire().is_empty(),
        "age 0 is not past a zero timeout"
    );
    assert_eq!(correlator.pending(), 1);

    clock.advance(1);
    assert_eq!(
        correlator.expire(),
        vec![launch_record(1, "app.one", Some(1), 0)]
    );
    assert_eq!(correlator.pending(), 0);
}

#[test]
fn an_expired_launch_never_shadows_a_live_one() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator_with(Duration::from_millis(100), &clock);
    correlator.record_launch(
        launch_record(1, "app.old", None, 0),
        &app_info("app.old", "Old", Some("shared-wm")),
    );

    clock.set(150);
    correlator.record_launch(
        launch_record(2, "app.new", None, 150),
        &app_info("app.new", "New", Some("shared-wm")),
    );

    let outcome = correlator.correlate(&window(1, None, Some("shared-wm"), None));
    assert_eq!(correlation(&outcome).launch.launch_id, LaunchId(2));
    assert_eq!(correlator.pending(), 1);
}

#[test]
fn a_non_monotonic_clock_never_expires_or_panics() {
    let clock = fake_clock(1_000);
    let mut correlator = new_correlator_with(Duration::from_millis(100), &clock);
    correlator.record_launch(
        launch_record(1, "app.one", Some(10), 1_000),
        &app_info("app.one", "One", None),
    );

    // The clock jumps backwards: the age saturates at zero, so nothing expires.
    clock.set(500);
    assert!(correlator.expire().is_empty());
    assert_eq!(correlator.pending(), 1);
    let outcome = correlator.correlate(&window(1, Some(10), None, None));
    assert_eq!(correlation(&outcome).evidence, CorrelationEvidence::Pid);
}

// --- bookkeeping and accessors ---------------------------------------------

#[test]
fn one_launch_correlates_several_windows() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "org.mozilla.firefox", Some(42), 0),
        &app_info("org.mozilla.firefox", "Firefox", Some("firefox")),
    );

    let first = correlator.correlate(&window(1, Some(42), Some("firefox"), None));
    let second = correlator.correlate(&window(2, None, Some("firefox"), Some("New Tab")));

    assert_eq!(correlation(&first).launch.launch_id, LaunchId(1));
    assert_eq!(correlation(&second).launch.launch_id, LaunchId(1));
    assert_eq!(correlation(&first).window_id, WindowId(1));
    assert_eq!(correlation(&second).window_id, WindowId(2));
    assert_eq!(correlator.pending(), 1, "the launch stays pending");
}

#[test]
fn uncorrelated_when_nothing_is_pending() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    let outcome = correlator.correlate(&window(1, Some(42), Some("firefox"), Some("Firefox")));
    assert_eq!(outcome, CorrelationOutcome::Uncorrelated);
    assert!(outcome.launch().is_none());
    assert_eq!(correlator.pending(), 0);
}

#[test]
fn uncorrelated_reports_instead_of_guessing_and_keeps_the_launch() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    correlator.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", Some("one-wm")),
    );

    assert_eq!(
        correlator.correlate(&window(1, None, Some("other-wm"), Some("Other"))),
        CorrelationOutcome::Uncorrelated
    );
    assert_eq!(correlator.pending(), 1);

    let outcome = correlator.correlate(&window(2, None, Some("one-wm"), None));
    assert_eq!(correlation(&outcome).window_id, WindowId(2));
}

#[test]
fn pid_tier_needs_both_sides_to_report_a_pid() {
    let clock = fake_clock(0);

    let mut launch_with_pid = new_correlator(&clock);
    launch_with_pid.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", None),
    );
    assert_eq!(
        launch_with_pid.correlate(&window(1, None, None, None)),
        CorrelationOutcome::Uncorrelated
    );

    let mut window_with_pid = new_correlator(&clock);
    window_with_pid.record_launch(
        launch_record(1, "app.one", None, 0),
        &app_info("app.one", "One", None),
    );
    assert_eq!(
        window_with_pid.correlate(&window(1, Some(10), None, None)),
        CorrelationOutcome::Uncorrelated
    );

    let mut mismatched = new_correlator(&clock);
    mismatched.record_launch(
        launch_record(1, "app.one", Some(10), 0),
        &app_info("app.one", "One", None),
    );
    assert_eq!(
        mismatched.correlate(&window(1, Some(11), None, None)),
        CorrelationOutcome::Uncorrelated
    );
}

#[test]
fn outcome_launch_accessor_reports_the_record() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    let launched = launch_record(3, "app.one", Some(1), 0);
    correlator.record_launch(launched.clone(), &app_info("app.one", "One", None));

    let outcome = correlator.correlate(&window(1, Some(1), None, None));
    assert_eq!(outcome.launch(), Some(&launched));
    assert_eq!(CorrelationOutcome::Uncorrelated.launch(), None);
}

#[test]
fn default_timeout_is_ten_seconds() {
    let clock = fake_clock(0);
    assert_eq!(DEFAULT_CORRELATION_TIMEOUT, Duration::from_secs(10));
    assert_eq!(
        new_correlator(&clock).timeout(),
        DEFAULT_CORRELATION_TIMEOUT
    );
    assert_eq!(
        new_correlator_with(Duration::from_millis(250), &clock).timeout(),
        Duration::from_millis(250)
    );
}

#[test]
fn pending_counts_every_recorded_launch() {
    let clock = fake_clock(0);
    let mut correlator = new_correlator(&clock);
    assert_eq!(correlator.pending(), 0);
    for id in 1..=3 {
        correlator.record_launch(
            launch_record(id, "app.one", Some(id as i32), 0),
            &app_info("app.one", "One", None),
        );
        assert_eq!(correlator.pending(), id as usize);
    }
}
