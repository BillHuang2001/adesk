//! Shared helpers for the `e2e` suite.
//!
//! This is **not** a test target of its own: it is included as a module by
//! `tests/e2e_runtime.rs` (which is gated on the `e2e` feature), so it is only
//! compiled — and only reachable — from that suite.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use adesk_a11y::{node, AccessibilitySource, FixtureSource};
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

/// Title of the fixture window the text-first e2e tests read as an accessibility
/// outline. The runtime hands it to the backend as the correlation title, so the
/// mapped toplevel must carry it.
pub const ACCESSIBILITY_WINDOW_TITLE: &str = "Preferences";

/// The exact §5.11 outline [`accessibility_fixture`] renders for
/// [`ACCESSIBILITY_WINDOW_TITLE`], with the protocol's default request options.
///
/// `docs/protocol.md` §5.11 fixes the outline grammar (pre-order ids, two spaces
/// of indent per level), so the e2e assertion can pin the real rendered text
/// rather than a substring.
pub const ACCESSIBILITY_OUTLINE: &str = concat!(
    r#"frame "Preferences" id=0"#,
    "\n",
    r#"  check_box "Dark Mode" value="on" actions=[toggle] id=1"#,
    "\n",
    r#"  push_button "Apply" actions=[click] id=2"#,
);

/// A deterministic accessibility backend serving [`ACCESSIBILITY_OUTLINE`].
///
/// [`adesk_a11y::FixtureSource`] needs no accessibility bus, toolkit or display,
/// so the whole §5.11 path is exercised headless; the returned handle lets a test
/// read back which window the runtime asked it to snapshot (`last_target`).
pub fn accessibility_fixture() -> Arc<FixtureSource> {
    Arc::new(FixtureSource::new(
        node("frame", ACCESSIBILITY_WINDOW_TITLE)
            .child(
                node("check_box", "Dark Mode")
                    .value("on")
                    .action("toggle")
                    .build(),
            )
            .child(node("push_button", "Apply").action("click").build())
            .build(),
    ))
}

/// Both `adesk_testkit::TestkitError` and `adesk_agent::Error` convert into this.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Name of this package's example that serves as the suite's fixture application.
const FIXTURE_APP: &str = "adesk-e2e-app";

/// Testkit's own fixture helper, used as a fallback when it happens to be built.
const TESTKIT_HELPER: &str = "adesk-test-app";

/// The fixture application binary that the launch tests put in a `.desktop` entry.
///
/// Resolution order:
///
/// 1. `target/<profile>/examples/adesk-e2e-app` — `cargo test` builds this package's
///    examples next to its test binaries (tests live in `target/<profile>/deps/`, examples
///    in `target/<profile>/examples/`), so the path is deterministic and needs no globbing;
/// 2. `adesk_testkit::helper_bin_path("adesk-test-app")` — an environment that already
///    built testkit's own helper keeps working.
///
/// Otherwise panics with the commands that build the example.
pub fn fixture_app_bin() -> PathBuf {
    if let Some(bundled) = bundled_fixture_app() {
        return bundled;
    }
    if let Ok(testkit_helper) = helper_bin_path(TESTKIT_HELPER) {
        return testkit_helper;
    }
    let searched = examples_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| String::from("<unknown>"));
    panic!(
        "no fixture application found: `{FIXTURE_APP}` is missing from {searched} and \
         testkit's `{TESTKIT_HELPER}` was not found next to the test binary or on $PATH; \
         build the example first:\n  \
         ./scripts/dev.sh cargo test -p adesk-agent --features e2e --no-run\n\
         or\n  \
         ./scripts/dev.sh cargo build -p adesk-agent --example {FIXTURE_APP} --features e2e"
    )
}

/// `target/<profile>/examples/<FIXTURE_APP><EXE_SUFFIX>` when that file exists.
fn bundled_fixture_app() -> Option<PathBuf> {
    let candidate = examples_dir()?.join(format!("{FIXTURE_APP}{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// The `examples` directory belonging to the running test binary.
///
/// A test binary lives in `target/<profile>/deps/`, so its grandparent is the profile
/// directory that also holds `examples/`.
fn examples_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.parent()?.join("examples"))
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

/// A pixman runtime whose §5.11 backend is the injected deterministic `source`.
///
/// Forwarded to `ServerConfig::accessibility_source`, so every accessibility
/// method of the running server answers from `source` — no accessibility bus,
/// toolkit or display involved. Like [`runtime`], it does not scope the process
/// environment, so it serializes with the other tests only through the runtime's
/// own ports.
pub async fn runtime_with_accessibility(
    source: Arc<dyn AccessibilitySource>,
) -> Result<TestRuntime, adesk_testkit::TestkitError> {
    TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_accessibility_source(source)
            .with_apply_env(false),
    )
    .await
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
