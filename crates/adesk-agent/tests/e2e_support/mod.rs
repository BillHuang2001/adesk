//! Shared helpers for the `e2e` suite.
//!
//! This is **not** a test target of its own: it is included as a module by
//! `tests/e2e_runtime.rs` (which is gated on the `e2e` feature), so it is only
//! compiled — and only reachable — from that suite.

use std::path::PathBuf;
use std::time::Duration;

use adesk_agent::{
    AgentDecision, AgentLoop, AgpClient, LoopConfig, MetricsReport, MockProvider, Scenario,
    ScenarioId, ScenarioReport, ScenarioRunner, StepRecord, TaskDescription,
};
use adesk_core::{Button, Observation, Position, WindowId};
use adesk_testkit::{
    helper_bin_path, EventAssert, Expected, FillPattern, Size, TestRuntime, TestRuntimeConfig,
    TestWindow, ToplevelSpec, WaylandTestClient,
};

/// Deadline for every bounded harness wait.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// App id (and `.desktop` id) of the fixture the launch tests start.
pub const LAUNCHED_APP_ID: &str = "org.example.files";

/// Self-exit deadline for the fixture helper: bounded, but far longer than any test body.
pub const HELPER_LIFETIME: Duration = Duration::from_secs(5);

/// Delay before the dialog test destroys its popup, while the scenario is observing.
pub const POPUP_DESTROY_DELAY: Duration = Duration::from_millis(400);

/// Delay before the navigation test renames its window, while the scenario is observing.
pub const TITLE_CHANGE_DELAY: Duration = Duration::from_millis(800);

/// Both `adesk_testkit::TestkitError` and `adesk_agent::Error` convert into this.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The fixture helper binary, with a build instruction when it is missing.
pub fn require_helper_bin() -> PathBuf {
    helper_bin_path("adesk-test-app").unwrap_or_else(|error| {
        panic!(
            "the `adesk-test-app` fixture helper is missing ({error}); build it first:\n  \
             ./scripts/dev.sh cargo build -p adesk-testkit --bin adesk-test-app"
        )
    })
}

/// Loop config for real-runtime runs: no retry backoff (tests must never sleep).
pub fn loop_config() -> LoopConfig {
    LoopConfig {
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    }
}

/// A pixman runtime that does not scope the process environment.
pub async fn runtime() -> Result<TestRuntime, adesk_testkit::TestkitError> {
    TestRuntime::start_with(TestRuntimeConfig::new().with_apply_env(false)).await
}

/// A fresh AGP adapter (`AgpClient` is not `Clone`).
pub async fn connect(runtime: &TestRuntime) -> Result<AgpClient, adesk_agent::Error> {
    AgpClient::connect(runtime.socket_path()).await
}

/// Maps one toplevel and asserts the id the runtime assigned to it.
///
/// The event tap is created *before* the first commit: the broadcast does not replay,
/// so a tap created afterwards would miss `WindowCreated`.
pub async fn map_window(
    runtime: &TestRuntime,
    app_id: &str,
    title: &str,
    expected_id: u64,
) -> Result<(WaylandTestClient, TestWindow, WindowId), adesk_testkit::TestkitError> {
    let mut events = EventAssert::tap(runtime);
    let wayland = runtime.wayland_client()?;
    let window = wayland.create_toplevel(ToplevelSpec::new(app_id, title, Size::new(320, 200)))?;
    window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;
    let created = events
        .wait_for_expected(&Expected::WindowCreated, DEADLINE)
        .await?;
    let id = created
        .window_id()
        .expect("window_created carries a window id");
    assert_eq!(
        id,
        WindowId(expected_id),
        "a fresh runtime allocates window ids in map order"
    );
    Ok((wayland, window, id))
}

/// Runs a built-in scenario against `runtime` and asserts every expectation passed.
pub async fn run_scenario(
    runtime: &TestRuntime,
    id: ScenarioId,
) -> Result<ScenarioReport, adesk_agent::Error> {
    run_scenario_with(runtime, &Scenario::builtin(id)).await
}

/// Runs `scenario` against `runtime` and asserts every expectation passed.
pub async fn run_scenario_with(
    runtime: &TestRuntime,
    scenario: &Scenario,
) -> Result<ScenarioReport, adesk_agent::Error> {
    let client = connect(runtime).await?;
    let report = ScenarioRunner::new(loop_config())
        .run(scenario, client)
        .await?;
    assert_report(&report);
    Ok(report)
}

/// Panics with per-expectation detail when a scenario report did not pass.
pub fn assert_report(report: &ScenarioReport) {
    let failures: Vec<String> = report
        .checks
        .iter()
        .filter(|check| !check.passed)
        .map(|check| format!("{:?}: {}", check.expectation, check.detail))
        .collect();
    assert!(
        report.passed,
        "scenario `{}` failed: {}",
        report.name,
        failures.join("; ")
    );
}

/// The first recorded step whose decision has `kind`.
pub fn step(history: &[StepRecord], kind: adesk_agent::ActionKind) -> &StepRecord {
    history
        .iter()
        .find(|record| record.decision.kind() == kind)
        .unwrap_or_else(|| {
            let kinds: Vec<adesk_agent::ActionKind> =
                history.iter().map(|r| r.decision.kind()).collect();
            panic!("no {kind:?} step in {kinds:?}")
        })
}

/// The first observation-producing step after `after_step`.
pub fn observation_after(history: &[StepRecord], after_step: u32) -> &StepRecord {
    history
        .iter()
        .find(|record| {
            record.step > after_step
                && matches!(
                    record.decision.kind(),
                    adesk_agent::ActionKind::Observe | adesk_agent::ActionKind::Wait
                )
        })
        .unwrap_or_else(|| {
            let kinds: Vec<adesk_agent::ActionKind> =
                history.iter().map(|r| r.decision.kind()).collect();
            panic!("no observation step after step {after_step} in {kinds:?}")
        })
}

/// The observation a step retained (every observed step must have one).
pub fn observation(record: &StepRecord) -> &Observation {
    record.observation.as_ref().unwrap_or_else(|| {
        panic!(
            "step {} ({:?}) recorded no observation",
            record.step, record.decision
        )
    })
}

/// A `Finish { success: true }` decision.
pub fn finish() -> AgentDecision {
    AgentDecision::Finish {
        success: true,
        summary: String::from("done"),
    }
}

/// A click at a normalized window-relative position.
pub fn click_at(window_id: WindowId, x: f64, y: f64) -> AgentDecision {
    AgentDecision::Click {
        window_id,
        position: Position::normalized(x, y),
        button: Button::Left,
        count: 1,
    }
}

/// The task handed to the loop when a test scripts decisions directly.
pub fn task(goal: &str) -> TaskDescription {
    TaskDescription::new(goal)
}

/// Runs `decisions` through a fresh loop and returns its outcome.
pub async fn run_script(
    runtime: &TestRuntime,
    decisions: Vec<AgentDecision>,
) -> Result<adesk_agent::LoopOutcome, adesk_agent::Error> {
    let client = connect(runtime).await?;
    let mut agent = AgentLoop::new(client, MockProvider::scripted(decisions), loop_config());
    agent.run(&task("scripted e2e run")).await
}

/// A metrics report with the timing fields removed, for exact comparison.
pub fn stable_metrics(report: &MetricsReport) -> serde_json::Value {
    let mut value = serde_json::to_value(report).expect("MetricsReport serializes");
    let object = value
        .as_object_mut()
        .expect("a serialized MetricsReport is a JSON object");
    object.remove("elapsed_ms");
    object.remove("decision_latency");
    value
}
