//! Built-in scenario definitions and the runner.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
#![cfg(feature = "test-support")]

use std::collections::HashSet;

use adesk_agent::scenario::effective_max_steps;
use adesk_agent::testing::{ClientMethod, ScriptedClient, ScriptedResponse};
use adesk_agent::{
    ActionKind, AgentDecision, Expectation, ExpectationResult, LaunchOutcome, LoopConfig,
    ObserveOutcome, RuntimeInfo, Scenario, ScenarioId, ScenarioReport, ScenarioRunner, ScriptEntry,
    StepStatus, StopReason, TaskDescription, WindowList, PROTOCOL_VERSION,
};
use adesk_core::{
    ActionId, AppId, AppInfo, ErrorCode, LaunchId, Observation, Rect, Size, WindowId, WindowInfo,
    WindowState,
};
use adesk_proto::{ImageFormat, ImagePayload};

/// Runtime identity as a conforming runtime reports it.
fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        protocol_version: PROTOCOL_VERSION,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 42,
        renderer: "pixman".to_owned(),
        output: Size::new(1280, 800),
    }
}

/// One mapped window of the virtual output.
fn window(id: u64, title: &str) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: Some(AppId::from("org.example.files")),
        title: Some(title.to_owned()),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: Some(4242),
        created_seq: 1,
        last_commit_seq: 1,
        popup_count: 0,
    }
}

/// A `list_windows` result.
fn windows(windows: Vec<WindowInfo>, active: Option<WindowId>) -> WindowList {
    WindowList {
        windows,
        active_window_id: active,
    }
}

/// No known windows.
fn empty_windows() -> WindowList {
    windows(Vec::new(), None)
}

/// One discovered application.
fn app(id: &str, name: &str) -> AppInfo {
    AppInfo {
        id: AppId::from(id),
        name: name.to_owned(),
        icon: None,
        exec: Some(String::from("files")),
        try_exec: None,
        terminal: false,
        categories: vec![String::from("Utility")],
        startup_wm_class: None,
        dbus_activatable: false,
        hidden: false,
        no_display: false,
    }
}

/// A quiet observation causally after `after_action`.
fn observation(window_id: Option<WindowId>, after_action: Option<ActionId>) -> Observation {
    Observation {
        window_id,
        after_action,
        commits: 1,
        changed_regions: vec![Rect::new(0, 0, 10, 10)],
        focus_changed: None,
        title_changed: false,
        new_windows: Vec::new(),
        destroyed_windows: Vec::new(),
        popups_appeared: Vec::new(),
        popups_disappeared: Vec::new(),
        elapsed_ms: 12,
        quiet: true,
        timed_out: false,
        last_commit_seq: 3,
        seq: 9,
    }
}

/// An `observe` result, optionally carrying pixels.
fn observe_response(observation: Observation, image: Option<ImagePayload>) -> ScriptedResponse {
    ScriptedResponse::Observe(ObserveOutcome { observation, image })
}

/// A payload carrying only dimensions: the agent never inspects pixels.
fn image(width: u32, height: u32) -> ImagePayload {
    ImagePayload {
        width,
        height,
        format: ImageFormat::Png,
        stride: None,
        data: String::new(),
        scale: 1.0,
    }
}

/// The checked result of one expectation.
fn check<'a>(report: &'a ScenarioReport, expectation: &Expectation) -> &'a ExpectationResult {
    report
        .checks
        .iter()
        .find(|check| &check.expectation == expectation)
        .unwrap_or_else(|| panic!("no check for {expectation:?} in {:?}", report.checks))
}

/// A runner with the default loop configuration.
fn runner() -> ScenarioRunner {
    ScenarioRunner::new(LoopConfig::default())
}

/// Every `ScenarioId` has a definition with a task, a non-empty script, and at
/// least one expectation; `as_str()` values are unique and match the CLI values.
#[test]
fn every_scenario_is_defined() {
    let ids = ScenarioId::all();
    assert_eq!(ids.len(), 8, "all eight built-ins are listed");

    let mut names = Vec::new();
    for &id in ids {
        let scenario = Scenario::builtin(id);
        assert_eq!(scenario.id, id, "builtin({id:?}) reports its own id");
        assert!(!scenario.name.is_empty(), "{id:?} has no name");
        assert!(
            !scenario.description.is_empty(),
            "{} has no description",
            scenario.name
        );
        assert!(
            !scenario.task.goal.is_empty(),
            "{} has no task goal",
            scenario.name
        );
        assert!(
            !scenario.script.is_empty(),
            "{} has an empty replay script",
            scenario.name
        );
        assert!(
            !scenario.expectations.is_empty(),
            "{} has no expectations",
            scenario.name
        );
        assert!(
            (6..=10).contains(&scenario.max_steps),
            "{} max_steps={} is outside 6..=10",
            scenario.name,
            scenario.max_steps
        );
        assert!(
            matches!(
                scenario.script.last().map(|entry| &entry.decision),
                Some(AgentDecision::Finish { success: true, .. })
            ),
            "{} must end its script with a successful Finish",
            scenario.name
        );
        assert!(
            scenario
                .expectations
                .iter()
                .any(|expectation| matches!(expectation, Expectation::Finished { success: true })),
            "{} must expect a successful finish",
            scenario.name
        );
        names.push(id.as_str());
    }

    let unique: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "duplicate names: {names:?}");
    assert_eq!(
        names,
        vec![
            "launch",
            "activate",
            "click",
            "type",
            "scroll",
            "dialog",
            "navigation",
            "error-recovery",
        ],
        "as_str() is the CLI vocabulary, in ScenarioId::all() order"
    );
}

/// `launch`: list apps → launch → wait for the window; expects `Finished{true}`
/// and `ActionSeen(LaunchApp)`.
#[tokio::test]
async fn launch_scenario_passes_with_script() {
    let scenario = Scenario::builtin(ScenarioId::Launch);
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Apps(vec![app("org.example.files", "Files")]),
        ScriptedResponse::Launch(LaunchOutcome {
            launch_id: LaunchId(1),
            app_id: AppId::from("org.example.files"),
            pid: Some(4242),
        }),
        observe_response(observation(Some(WindowId(5)), None), None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert_eq!(report.id, ScenarioId::Launch);
    assert_eq!(report.name, scenario.name);
    assert_eq!(report.outcome.stop_reason, StopReason::Finished);
    assert!(report.outcome.success);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::LaunchApp)).passed);
    assert_eq!(handle.call_count(ClientMethod::Ping), 1);
    assert_eq!(handle.call_count(ClientMethod::ListApps), 1);
    assert_eq!(handle.call_count(ClientMethod::LaunchApp), 1);
    assert_eq!(handle.call_count(ClientMethod::Observe), 1);
    assert_eq!(handle.remaining(), 0, "every canned response was consumed");

    // Waiting for the window is an observation without pixels.
    let calls = handle.calls();
    assert_eq!(calls[3].method, ClientMethod::Observe);
    assert!(
        calls[3].summary.contains("until=quiet(250)"),
        "summary: {}",
        calls[3].summary
    );
    assert!(
        calls[3].summary.contains("include_image=false"),
        "summary: {}",
        calls[3].summary
    );
    assert_eq!(report.outcome.metrics.gpu_readbacks, 0);
}

/// `activate`: two windows, activate the inactive one via `activate_window`
/// (asserting no synthetic input), expect `ActionSeen(ActivateWindow)`.
#[tokio::test]
async fn activate_scenario_uses_runtime_native_focus() {
    let scenario = Scenario::builtin(ScenarioId::Activate);
    let mut client = ScriptedClient::new();
    let mut focused = observation(Some(WindowId(2)), Some(ActionId(1)));
    focused.focus_changed = Some(true);
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(windows(
            vec![window(1, "Home"), window(2, "Editor")],
            Some(WindowId(1)),
        )),
        ScriptedResponse::Action(ActionId(1)),
        observe_response(focused, None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(
        check(
            &report,
            &Expectation::ActionSeen(ActionKind::ActivateWindow)
        )
        .passed
    );
    assert_eq!(handle.call_count(ClientMethod::ActivateWindow), 1);
    assert_eq!(handle.calls()[2].summary, "window_id=2");

    // Focus is compositor state, never synthesized input.
    assert_eq!(report.outcome.metrics.input_actions, 0);
    for method in [
        ClientMethod::Click,
        ClientMethod::Keypress,
        ClientMethod::TypeText,
        ClientMethod::Scroll,
    ] {
        assert_eq!(
            handle.call_count(method),
            0,
            "{method:?} must not be used to change focus"
        );
    }

    let observed = report.outcome.history[2]
        .observation
        .as_ref()
        .expect("the observation is recorded");
    assert_eq!(observed.focus_changed, Some(true));
}

/// `click`: normalized position click + quiet observation, capped readbacks.
#[tokio::test]
async fn click_scenario_observes_quiet() {
    let scenario = Scenario::builtin(ScenarioId::Click);
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(windows(vec![window(1, "Editor")], Some(WindowId(1)))),
        ScriptedResponse::Action(ActionId(1)),
        observe_response(
            observation(Some(WindowId(1)), Some(ActionId(1))),
            Some(image(1280, 800)),
        ),
        observe_response(observation(Some(WindowId(1)), Some(ActionId(1))), None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Click)).passed);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Observe)).passed);
    assert!(check(&report, &Expectation::MaxReadbacks(1)).passed);
    assert_eq!(report.outcome.metrics.gpu_readbacks, 1);

    let calls = handle.calls();
    assert_eq!(calls[2].method, ClientMethod::Click);
    assert!(
        calls[2].summary.contains("position=normalized(0.5,0.05)"),
        "summary: {}",
        calls[2].summary
    );
    // The automatic observation carries the click's action id; the explicit one
    // is a second, pixel-free quiet wait.
    assert_eq!(calls[3].method, ClientMethod::Observe);
    assert!(
        calls[3]
            .summary
            .contains("after_action=Some(1) until=quiet(250)"),
        "summary: {}",
        calls[3].summary
    );
    assert_eq!(calls[4].method, ClientMethod::Observe);
    assert!(
        calls[4].summary.contains("include_image=false"),
        "summary: {}",
        calls[4].summary
    );
    assert_eq!(report.outcome.metrics.visual_tokens, 1_105);
}

/// `type`: `type_text` into the focused window + quiet observation.
#[tokio::test]
async fn type_scenario_types_into_focus() {
    let scenario = Scenario::builtin(ScenarioId::Type);
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(windows(vec![window(1, "Save")], Some(WindowId(1)))),
        ScriptedResponse::Type(adesk_agent::TypeOutcome {
            action_id: ActionId(2),
            skipped: Vec::new(),
        }),
        observe_response(
            observation(Some(WindowId(1)), Some(ActionId(2))),
            Some(image(1280, 800)),
        ),
        observe_response(observation(Some(WindowId(1)), Some(ActionId(2))), None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::TypeText)).passed);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Observe)).passed);
    assert_eq!(handle.call_count(ClientMethod::TypeText), 1);
    assert_eq!(
        handle.calls()[2].summary,
        r#"text="report.txt" window_id=Some(1)"#
    );
    assert_eq!(
        handle.call_count(ClientMethod::Keypress),
        0,
        "text goes through type_text, not synthesized chords"
    );
    assert_eq!(report.outcome.metrics.input_actions, 1);
    assert_eq!(
        report.outcome.metrics.runtime_ops, 2,
        "list_windows + observe"
    );
}

/// `scroll`: pointer axis scroll + change observation.
#[tokio::test]
async fn scroll_scenario_scrolls_and_observes() {
    let scenario = Scenario::builtin(ScenarioId::Scroll);
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(windows(vec![window(1, "Document")], Some(WindowId(1)))),
        ScriptedResponse::Action(ActionId(3)),
        observe_response(
            observation(Some(WindowId(1)), Some(ActionId(3))),
            Some(image(1280, 800)),
        ),
        observe_response(observation(Some(WindowId(1)), Some(ActionId(3))), None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Scroll)).passed);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Observe)).passed);
    assert_eq!(handle.call_count(ClientMethod::Scroll), 1);
    assert!(
        handle.calls()[2]
            .summary
            .contains("position=normalized(0.5,0.5) dx=0 dy=-3"),
        "summary: {}",
        handle.calls()[2].summary
    );
    // The explicit observation waits for the change, not for quiet.
    assert!(
        handle.calls()[4].summary.contains("until=change"),
        "summary: {}",
        handle.calls()[4].summary
    );
    assert_eq!(report.outcome.metrics.gpu_readbacks, 1);
}

/// `dialog`: popup appears → dismiss → verify disappearance.
#[tokio::test]
async fn dialog_scenario_handles_popups() {
    let scenario = Scenario::builtin(ScenarioId::Dialog);
    let mut appeared = observation(Some(WindowId(1)), None);
    appeared.commits = 2;
    appeared.popups_appeared = vec![7];
    let mut dismissed = observation(Some(WindowId(1)), Some(ActionId(4)));
    dismissed.popups_disappeared = vec![7];

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        observe_response(appeared, None),
        ScriptedResponse::Action(ActionId(4)),
        observe_response(dismissed.clone(), Some(image(1280, 800))),
        observe_response(dismissed, None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Click)).passed);
    assert!(
        check(
            &report,
            &Expectation::MinActions {
                kind: ActionKind::Observe,
                count: 2,
            }
        )
        .passed
    );

    // detect → dismiss → verify.
    let detected = report.outcome.history[0]
        .observation
        .as_ref()
        .expect("the popup observation is recorded");
    assert_eq!(detected.popups_appeared, vec![7]);
    assert!(
        report.outcome.history.iter().any(|record| record
            .observation
            .as_ref()
            .is_some_and(|observation| observation.popups_disappeared == vec![7])),
        "the dismissal is observed: {:?}",
        report.outcome.history
    );
    assert_eq!(handle.call_count(ClientMethod::Observe), 3);
    assert_eq!(handle.call_count(ClientMethod::Click), 1);
}

/// `navigation`: click a link, observe the change, verify the title changed.
#[tokio::test]
async fn navigation_scenario_crosses_pages() {
    let scenario = Scenario::builtin(ScenarioId::Navigation);
    let mut navigated = observation(Some(WindowId(1)), Some(ActionId(5)));
    navigated.title_changed = true;

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(windows(vec![window(1, "Home")], Some(WindowId(1)))),
        ScriptedResponse::Action(ActionId(5)),
        observe_response(navigated.clone(), Some(image(1280, 800))),
        observe_response(navigated, None),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Click)).passed);
    assert!(check(&report, &Expectation::ActionSeen(ActionKind::Observe)).passed);
    assert!(
        report.outcome.history.iter().any(|record| record
            .observation
            .as_ref()
            .is_some_and(|observation| observation.title_changed)),
        "a title_changed observation is recorded: {:?}",
        report.outcome.history
    );
    assert!(
        handle.calls()[2]
            .summary
            .contains("position=normalized(0.25,0.4)"),
        "summary: {}",
        handle.calls()[2].summary
    );
    assert_eq!(handle.call_count(ClientMethod::Click), 1);
}

/// `error_recovery`: injected `unknown_window` → refresh → continue → finish.
#[tokio::test]
async fn error_recovery_scenario_recovers() {
    let scenario = Scenario::builtin(ScenarioId::ErrorRecovery);
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Error(adesk_core::Error::new(
            ErrorCode::UnknownWindow,
            "no such window",
        )),
        ScriptedResponse::Windows(windows(vec![window(1, "Wizard")], Some(WindowId(1)))),
        ScriptedResponse::Action(ActionId(6)),
        observe_response(
            observation(Some(WindowId(1)), Some(ActionId(6))),
            Some(image(1280, 800)),
        ),
    ]);
    let handle = client.clone();

    let report = runner()
        .run(&scenario, client)
        .await
        .expect("recoverable errors continue");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert!(report.outcome.success);
    assert_eq!(report.outcome.stop_reason, StopReason::Finished);
    assert!(
        report.outcome.metrics.recoveries >= 1,
        "recoveries: {}",
        report.outcome.metrics.recoveries
    );
    assert_eq!(report.outcome.metrics.failures, 1);
    assert_eq!(report.outcome.history[0].status, StepStatus::Failed);
    assert_eq!(report.outcome.history[1].status, StepStatus::Ok);
    assert_eq!(
        handle.call_count(ClientMethod::ListWindows),
        1,
        "one refresh after the stale window id"
    );
    assert_eq!(handle.call_count(ClientMethod::Click), 2);
    assert_eq!(
        report
            .outcome
            .metrics
            .actions_by_kind
            .get(&ActionKind::Click),
        Some(&1),
        "only the click that landed is an action"
    );
    assert_eq!(handle.remaining(), 0);
}

/// The runner honours `min(runner.max_steps, scenario.max_steps)` and reports
/// failed expectations instead of erroring.
#[tokio::test]
async fn runner_enforces_step_budget_and_reports_failures() {
    let scenario = Scenario {
        id: ScenarioId::Launch,
        name: "budget-probe",
        description: "test fixture: never finishes",
        task: TaskDescription::new("list windows until the budget runs out"),
        script: vec![
            ScriptEntry::decision(AgentDecision::ListWindows),
            ScriptEntry::decision(AgentDecision::ListWindows),
            ScriptEntry::decision(AgentDecision::ListWindows),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::MaxSteps(1),
            Expectation::MaxSteps(2),
            Expectation::ActionSeen(ActionKind::LaunchApp),
            Expectation::NoFailedSteps,
        ],
        max_steps: 10,
    };
    let runner = ScenarioRunner::new(LoopConfig {
        max_steps: 2,
        ..LoopConfig::default()
    });
    assert_eq!(
        effective_max_steps(&runner, &scenario),
        2,
        "the runner's budget is the smaller one"
    );

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();

    let report = runner
        .run(&scenario, client)
        .await
        .expect("failed expectations are reported, not returned as errors");

    assert!(!report.passed);
    assert_eq!(report.outcome.steps, 2);
    assert_eq!(report.outcome.stop_reason, StopReason::StepBudgetExhausted);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 2);
    assert_eq!(
        handle.remaining(),
        0,
        "the third decision is never executed"
    );

    let failed: Vec<&ExpectationResult> =
        report.checks.iter().filter(|check| !check.passed).collect();
    assert_eq!(
        failed.len(),
        3,
        "checks report themselves: {:?}",
        report.checks
    );
    let finished = check(&report, &Expectation::Finished { success: true });
    assert!(!finished.passed);
    assert!(
        finished.detail.contains("StepBudgetExhausted"),
        "detail states the observed stop reason: {}",
        finished.detail
    );
    let steps = check(&report, &Expectation::MaxSteps(1));
    assert!(!steps.passed);
    assert!(
        steps.detail.contains("steps=2"),
        "detail states the observed value: {}",
        steps.detail
    );
    assert!(check(&report, &Expectation::MaxSteps(2)).passed);
    assert!(!check(&report, &Expectation::ActionSeen(ActionKind::LaunchApp)).passed);
    assert!(check(&report, &Expectation::NoFailedSteps).passed);

    // The scenario's own budget caps the run as well.
    let mut tight = scenario.clone();
    tight.max_steps = 1;
    let runner = ScenarioRunner::new(LoopConfig::default());
    assert_eq!(effective_max_steps(&runner, &tight), 1);

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let report = runner
        .run(&tight, client)
        .await
        .expect("budget stops are Ok");

    assert_eq!(report.outcome.steps, 1);
    assert_eq!(report.outcome.stop_reason, StopReason::StepBudgetExhausted);
    assert!(!report.passed);
}
