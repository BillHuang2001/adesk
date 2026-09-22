//! Text-first observation: the agent reads the desktop as accessibility text.
//!
//! These tests prove the runtime can be "seen" through the window's accessibility
//! outline carried in the *model context* — with no pixel ever captured — and that
//! a missing accessibility backend degrades benignly instead of ending the run.
//! They drive the loop with the socket-free `ScriptedClient` + `MockProvider`, so
//! no runtime, GPU or network is involved.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`

mod common;

use std::sync::Arc;

use adesk_agent::client::AccessibilityOutcome;
use adesk_agent::testing::{ClientMethod, ScriptedClient, ScriptedResponse};
use adesk_agent::{
    ActionKind, AgentDecision, AgentLoop, Expectation, LoopConfig, MockProvider, ObserveCondition,
    ObserveOutcome, Scenario, ScenarioId, ScenarioRunner, ScriptEntry, StepStatus, StopReason,
    TaskDescription,
};
use adesk_core::{Rect, WindowId, WindowInfo, WindowState};
use common::{observation, runtime_info};

/// The task every test runs (only the goal matters to the loop).
fn task() -> TaskDescription {
    TaskDescription::new("read the preferences dialog and enable dark mode")
}

/// A mapped, active window with `id`, for the `get_window` refresh the
/// accessibility upsert issues for an untracked window.
fn window(id: u64) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: None,
        title: Some(String::from("Preferences")),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: None,
        created_seq: 1,
        last_commit_seq: 3,
        popup_count: 0,
    }
}

/// An `observe` decision on `window_id`, explicitly without an image request.
fn observe_decision(window_id: u64) -> AgentDecision {
    AgentDecision::Observe {
        window_id: Some(WindowId(window_id)),
        after_action: None,
        until: ObserveCondition::Quiet { quiet_ms: 250 },
        timeout_ms: None,
        include_image: Some(false),
        max_dimension: None,
        region: None,
    }
}

/// A successful `finish`.
fn finish() -> AgentDecision {
    AgentDecision::Finish {
        success: true,
        summary: String::from("done"),
    }
}

/// A quiet `observe` result for `window_id`, without pixels.
fn quiet_observe(window_id: u64) -> ScriptedResponse {
    ScriptedResponse::Observe(ObserveOutcome {
        observation: observation(Some(WindowId(window_id)), None),
        image: None,
    })
}

/// An accessibility outline for `window_id`.
fn outline(window_id: u64, text: &str) -> AccessibilityOutcome {
    AccessibilityOutcome {
        window_id: WindowId(window_id),
        node_count: 3,
        truncated: false,
        text: text.to_owned(),
    }
}

/// With the text-first capability on, an `observe` pulls the observed window's
/// accessibility outline through one extra `accessibility_tree` call and the
/// outline reaches the model context — the agent "sees" the UI as text with no
/// pixel readback and no visual tokens.
#[tokio::test]
async fn accessibility_text_reaches_the_model_context_without_pixels() {
    let outline_text =
        "- window \"Preferences\"\n  - check_box \"Dark Mode\" (checked)\n  - button \"Apply\"\n";

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        quiet_observe(1),
        ScriptedResponse::AccessibilityTree(outline(1, outline_text)),
        // The outcome carries only the window id, so the untracked window is refreshed.
        ScriptedResponse::Window(window(1)),
    ]);
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![observe_decision(1), finish()]));
    let config = LoopConfig {
        include_accessibility: true,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), config);
    let outcome = agent.run(&task()).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(
        handle.call_count(ClientMethod::AccessibilityTree),
        1,
        "exactly one text read, in place of pixels"
    );

    // The whole point: the desktop was rendered as text, not pixels.
    assert_eq!(
        outcome.metrics.gpu_readbacks, 0,
        "no pixel readback happened"
    );
    assert_eq!(outcome.metrics.images_sent, 0, "no image was embedded");
    assert_eq!(
        outcome.metrics.visual_tokens, 0,
        "no visual tokens were spent"
    );
    assert_eq!(
        handle.call_count(ClientMethod::CaptureWindow),
        0,
        "a capture must never be issued for a text-first observation"
    );
    let observe = handle
        .calls()
        .into_iter()
        .find(|call| call.method == ClientMethod::Observe)
        .expect("the explicit observe was issued");
    assert!(
        observe.summary.contains("include_image=false"),
        "the observation reads no pixels: {}",
        observe.summary
    );

    // The outline is the model's only view of the window.
    let contexts = provider.contexts();
    assert!(
        contexts
            .iter()
            .any(|context| context.accessibility.as_deref() == Some(outline_text)),
        "the accessibility outline must reach the provider context"
    );
    assert!(
        contexts
            .iter()
            .all(|context| context.image.is_none() && context.keyframe.is_none()),
        "no frame is ever attached to the context"
    );
}

/// The same story at the scenario-runner level: a text-first plan finishes with
/// `MaxReadbacks(0)` satisfied — the run consumed the accessibility text alone.
#[tokio::test]
async fn scenario_runner_finishes_from_accessibility_text_without_readbacks() {
    let outline_text = "- window \"Preferences\"\n  - button \"Apply\"\n";
    let scenario = Scenario {
        id: ScenarioId::Launch,
        name: "text-first",
        description: "test fixture: read the desktop as accessibility text",
        task: task(),
        script: vec![
            ScriptEntry::decision(observe_decision(1)),
            ScriptEntry::decision(finish()),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::Observe),
            Expectation::MaxReadbacks(0),
            Expectation::NoFailedSteps,
        ],
        max_steps: 4,
    };

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        quiet_observe(1),
        ScriptedResponse::AccessibilityTree(outline(1, outline_text)),
        ScriptedResponse::Window(window(1)),
    ]);
    let handle = client.clone();

    let runner = ScenarioRunner::new(LoopConfig {
        include_accessibility: true,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    });
    let report = runner
        .run(&scenario, client)
        .await
        .expect("the scenario runs");

    assert!(report.passed, "failed checks: {:?}", report.checks);
    assert_eq!(report.outcome.stop_reason, StopReason::Finished);
    assert_eq!(report.outcome.metrics.gpu_readbacks, 0);
    assert_eq!(report.outcome.metrics.images_sent, 0);
    assert_eq!(report.outcome.metrics.visual_tokens, 0);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 1);
    assert_eq!(handle.call_count(ClientMethod::CaptureWindow), 0);
    assert_eq!(handle.remaining(), 0, "every canned response was consumed");
}

/// A runtime without an accessibility backend degrades benignly: the
/// `not_supported` error is swallowed as a non-failure, the run finishes, and the
/// context simply carries no outline (and still no pixels).
#[tokio::test]
async fn unavailable_accessibility_backend_is_non_fatal() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        quiet_observe(1),
        // The runtime has no accessibility stack to serve the request.
        ScriptedResponse::Error(adesk_core::Error::not_supported("no accessibility backend")),
    ]);
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![observe_decision(1), finish()]));
    let config = LoopConfig {
        include_accessibility: true,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), config);
    let outcome = agent
        .run(&task())
        .await
        .expect("an unavailable backend must not end the run");

    assert!(outcome.success);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 1);
    assert_eq!(
        outcome.metrics.failures, 0,
        "a benign degradation is not a step failure"
    );
    assert_eq!(
        outcome.history[0].status,
        StepStatus::Ok,
        "the step carrying the degraded enrichment still succeeds"
    );

    // No outline and no pixels: the run simply proceeded without text vision.
    assert!(
        provider
            .contexts()
            .iter()
            .all(|context| context.accessibility.is_none()),
        "an unavailable backend leaves no outline"
    );
    assert_eq!(outcome.metrics.gpu_readbacks, 0);
    assert_eq!(outcome.metrics.visual_tokens, 0);
    assert_eq!(handle.remaining(), 0, "every canned response was consumed");
}
