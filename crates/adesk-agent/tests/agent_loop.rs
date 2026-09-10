//! Loop behavior against the socket-free `ScriptedClient` + `MockProvider`.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
//!
//! Every test here must run without a socket, compositor, GPU or network.
#![cfg(feature = "test-support")]

mod common;

use std::sync::Arc;

use adesk_agent::testing::{ClientMethod, ScriptedClient, ScriptedResponse};
use adesk_agent::{
    AgentContext, AgentDecision, AgentLoop, Error, LlmProvider, LoopConfig, MockProvider,
    ObserveOutcome, ProviderError, ScriptEntry, StepStatus, StopReason, TaskDescription,
    WindowList, PROTOCOL_VERSION,
};
use adesk_core::{
    ActionId, Button, ErrorCode, Observation, Position, Rect, WindowId, WindowInfo, WindowState,
};
use async_trait::async_trait;
use common::{empty_windows, runtime_info};

/// Build a loop over a scripted client and mock provider (used by the phase-2
/// tests below).
fn loop_with(
    client: ScriptedClient,
    decisions: Vec<AgentDecision>,
    config: LoopConfig,
) -> AgentLoop<ScriptedClient, MockProvider> {
    AgentLoop::new(client, MockProvider::scripted(decisions), config)
}

/// Wrapper keeping a handle on the mock provider's recorded contexts: the loop
/// takes its provider by value and `MockProvider` is not `Clone`.
#[derive(Debug)]
struct SharedProvider(Arc<MockProvider>);

#[async_trait]
impl LlmProvider for SharedProvider {
    async fn complete(
        &self,
        ctx: &AgentContext,
    ) -> std::result::Result<AgentDecision, ProviderError> {
        self.0.complete(ctx).await
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn supports_images(&self) -> bool {
        self.0.supports_images()
    }
}

/// The task every test runs (only the goal matters to the loop).
fn task() -> TaskDescription {
    TaskDescription::new("open the settings dialog and enable dark mode")
}

/// A `Finish` decision with the given outcome.
fn finish(success: bool, summary: &str) -> AgentDecision {
    AgentDecision::Finish {
        success,
        summary: summary.to_owned(),
    }
}

/// A click at a fixed window-relative position.
fn click_decision(window_id: u64) -> AgentDecision {
    AgentDecision::Click {
        window_id: WindowId(window_id),
        position: Position::pixels(10, 20),
        button: Button::Left,
        count: 1,
    }
}

/// One mapped, active window.
fn window(id: u64) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: None,
        title: Some(format!("window {id}")),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: None,
        created_seq: 1,
        last_commit_seq: 1,
        popup_count: 0,
    }
}

/// A quiet observation causally after `after_action`.
fn observation(after_action: Option<ActionId>) -> Observation {
    common::observation(Some(WindowId(1)), after_action)
}

/// An `observe` result without pixels.
fn observe(after_action: Option<ActionId>) -> ScriptedResponse {
    ScriptedResponse::Observe(ObserveOutcome {
        observation: observation(after_action),
        image: None,
    })
}

/// A `Finish { success: true }` decision ends the run with `StopReason::Finished`,
/// one step, and no actions counted.
#[tokio::test]
async fn finish_ends_the_run() {
    let mut client = ScriptedClient::new();
    client.push(ScriptedResponse::Ping(runtime_info()));
    let handle = client.clone();
    let mut agent = loop_with(client, vec![finish(true, "done")], LoopConfig::default());

    let outcome = agent.run(&task()).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(outcome.summary, "done");
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(outcome.metrics.steps, 1);
    assert_eq!(outcome.metrics.decisions, 1);
    assert_eq!(outcome.metrics.actions, 0, "Finish is not an action");

    assert_eq!(outcome.history.len(), 1);
    assert_eq!(outcome.history[0].status, StepStatus::Finished);
    assert_eq!(outcome.history[0].action_id, None);
    assert_eq!(outcome.history[0].observation, None);
    assert!(!outcome.history[0].readback);

    assert_eq!(handle.call_count(ClientMethod::Ping), 1);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 0);
    assert_eq!(handle.remaining(), 0);
}

/// After a seat input action the loop observes `quiet` with
/// `after_action = Some(last_action_id)`, so the observation carries causal
/// history. Assert one `Click` call, one `Observe` call, and that the observe
/// request's `after_action` equals the click's action id.
#[tokio::test]
async fn input_is_followed_by_quiet_observation_with_after_action() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Action(ActionId(7)),
        observe(Some(ActionId(7))),
    ]);
    let handle = client.clone();
    let mut agent = loop_with(
        client,
        vec![click_decision(3), finish(true, "clicked")],
        LoopConfig::default(),
    );

    let outcome = agent.run(&task()).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::Click), 1);
    assert_eq!(handle.call_count(ClientMethod::Observe), 1);
    assert_eq!(handle.remaining(), 0);

    let calls = handle.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].method, ClientMethod::Click);
    assert_eq!(
        calls[1].summary,
        "window_id=3 position=pixels(10,20) button=Left count=1"
    );
    assert_eq!(calls[2].method, ClientMethod::Observe);
    assert_eq!(
        calls[2].summary,
        "window_id=Some(3) after_action=Some(7) until=quiet(250) timeout_ms=5000 \
         include_image=true max_dimension=Some(1024) region=none"
    );

    assert_eq!(outcome.history[0].action_id, Some(ActionId(7)));
    let recorded = outcome.history[0]
        .observation
        .as_ref()
        .expect("the automatic observation is recorded");
    assert_eq!(recorded.after_action, Some(ActionId(7)));
    assert!(recorded.quiet);
    assert_eq!(outcome.history[0].status, StepStatus::Ok);
    assert_eq!(outcome.history[1].status, StepStatus::Finished);

    assert_eq!(outcome.metrics.input_actions, 1);
    assert_eq!(outcome.metrics.runtime_ops, 0);
    assert_eq!(outcome.metrics.gpu_readbacks, 0, "no image was requested");
}

/// The loop never exceeds `LoopConfig::max_steps` and stops with
/// `StopReason::StepBudgetExhausted` when the provider keeps issuing actions.
#[tokio::test]
async fn step_budget_is_enforced() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();
    let config = LoopConfig {
        max_steps: 2,
        ..LoopConfig::default()
    };
    let mut agent = loop_with(
        client,
        vec![
            AgentDecision::ListWindows,
            AgentDecision::ListWindows,
            AgentDecision::ListWindows,
        ],
        config,
    );

    let outcome = agent.run(&task()).await.expect("budget stops are Ok");

    assert!(!outcome.success);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.stop_reason, StopReason::StepBudgetExhausted);
    assert_eq!(outcome.metrics.steps, 2);
    assert_eq!(outcome.metrics.actions, 2);
    assert_eq!(outcome.history.len(), 2);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 2);
    assert_eq!(
        handle.remaining(),
        0,
        "the third decision is never executed"
    );
}

/// A retryable error (transport) retries the same step up to
/// `retries_per_step`, then the run continues; metrics record the failure.
#[tokio::test]
async fn retryable_errors_are_retried_then_recovered() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Error(adesk_core::Error::new(ErrorCode::Timeout, "runtime busy")),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();
    let config = LoopConfig {
        retries_per_step: 1,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = loop_with(
        client,
        vec![AgentDecision::ListWindows, finish(true, "done")],
        config,
    );

    let outcome = agent.run(&task()).await.expect("the retry recovers");

    assert!(outcome.success);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(
        handle.call_count(ClientMethod::ListWindows),
        2,
        "the step is retried once"
    );

    assert_eq!(outcome.history[0].status, StepStatus::Recovered);
    assert_eq!(outcome.history[0].error, None);
    assert_eq!(
        outcome.metrics.actions, 1,
        "one action, not one per attempt"
    );
    assert_eq!(outcome.metrics.failures, 1);
    assert_eq!(outcome.metrics.failures_by_kind.get("timeout"), Some(&1));
    assert_eq!(outcome.metrics.recoveries, 1);
    assert_eq!(outcome.metrics.recovery_rate, 1.0);
    assert_eq!(outcome.metrics.failure_rate, 0.5);
}

/// A recoverable error (`unknown_window`) does not end the run: the loop records
/// the failure, refreshes the window list, and the agent's next decision
/// succeeds — `recoveries == 1`.
#[tokio::test]
async fn unknown_window_is_recovered_by_refresh() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Error(adesk_core::Error::new(
            ErrorCode::UnknownWindow,
            "no such window",
        )),
        ScriptedResponse::Windows(WindowList {
            windows: vec![window(1)],
            active_window_id: Some(WindowId(1)),
        }),
        ScriptedResponse::Action(ActionId(11)),
        observe(Some(ActionId(11))),
    ]);
    let handle = client.clone();
    let mut agent = loop_with(
        client,
        vec![
            click_decision(1),
            click_decision(1),
            finish(true, "clicked"),
        ],
        LoopConfig::default(),
    );

    let outcome = agent
        .run(&task())
        .await
        .expect("recoverable errors continue");

    assert!(outcome.success);
    assert_eq!(outcome.steps, 3);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(handle.call_count(ClientMethod::Click), 2);
    assert_eq!(
        handle.call_count(ClientMethod::ListWindows),
        1,
        "one refresh per recoverable failure"
    );
    assert_eq!(handle.call_count(ClientMethod::Observe), 1);
    assert_eq!(handle.remaining(), 0);

    assert_eq!(outcome.history[0].status, StepStatus::Failed);
    let message = outcome.history[0]
        .error
        .as_deref()
        .expect("the failure is recorded");
    assert!(message.contains("no such window"), "message: {message}");
    assert_eq!(outcome.history[0].action_id, None);

    assert_eq!(outcome.history[1].status, StepStatus::Ok);
    assert_eq!(outcome.history[1].action_id, Some(ActionId(11)));
    assert_eq!(outcome.history[2].status, StepStatus::Finished);

    assert_eq!(outcome.metrics.failures, 1);
    assert_eq!(
        outcome.metrics.failures_by_kind.get("unknown_window"),
        Some(&1)
    );
    assert_eq!(outcome.metrics.recoveries, 1);
    assert_eq!(
        outcome.metrics.actions, 1,
        "the failed click is not counted as an action"
    );
}

/// `max_consecutive_failures` consecutive failures stop the run with
/// `StopReason::FailureBudgetExhausted` and `success == false`.
#[tokio::test]
async fn failure_budget_stops_the_run() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Error(adesk_core::Error::new(ErrorCode::UnknownWindow, "gone")),
        ScriptedResponse::Windows(empty_windows()),
        ScriptedResponse::Error(adesk_core::Error::new(ErrorCode::UnknownWindow, "gone")),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();
    let config = LoopConfig {
        max_consecutive_failures: 2,
        ..LoopConfig::default()
    };
    let mut agent = loop_with(
        client,
        vec![
            click_decision(1),
            click_decision(1),
            finish(true, "never reached"),
        ],
        config,
    );

    let outcome = agent.run(&task()).await.expect("budget stops are Ok");

    assert!(!outcome.success);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.stop_reason, StopReason::FailureBudgetExhausted);
    assert!(
        outcome.summary.contains('2'),
        "summary names the budget: {}",
        outcome.summary
    );
    assert_eq!(outcome.history.len(), 2);
    assert!(
        outcome
            .history
            .iter()
            .all(|record| record.status == StepStatus::Failed),
        "history: {:?}",
        outcome.history
    );
    assert_eq!(outcome.metrics.failures, 2);
    assert_eq!(outcome.metrics.recoveries, 0);
    assert_eq!(handle.call_count(ClientMethod::Click), 2);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 2);
    assert_eq!(
        handle.remaining(),
        0,
        "the run stops before the Finish decision"
    );
}

/// A provider error is surfaced as `Error::Provider`; a provider that runs out of
/// script entries fails loudly instead of silently repeating.
#[tokio::test]
async fn provider_failures_are_classified() {
    // A non-retryable provider failure ends the run as `Error::Provider`.
    let mut client = ScriptedClient::new();
    client.push(ScriptedResponse::Ping(runtime_info()));
    let provider = MockProvider::new(vec![ScriptEntry::error(ProviderError::InvalidResponse(
        "bad json".to_owned(),
    ))]);
    let mut agent = AgentLoop::new(client, provider, LoopConfig::default());
    let error = agent
        .run(&task())
        .await
        .expect_err("invalid responses are fatal");
    assert!(
        matches!(error, Error::Provider(ProviderError::InvalidResponse(ref message)) if message == "bad json"),
        "unexpected error: {error}"
    );
    assert!(
        agent.history().is_empty(),
        "a step without a decision is not recorded"
    );

    // A retryable provider failure is retried and reaches the next entry.
    let mut client = ScriptedClient::new();
    client.push(ScriptedResponse::Ping(runtime_info()));
    let provider = MockProvider::new(vec![
        ScriptEntry::error(ProviderError::Transport("boom".to_owned())),
        ScriptEntry::decision(finish(true, "recovered")),
    ]);
    let config = LoopConfig {
        retries_per_step: 1,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = AgentLoop::new(client, provider, config);
    let outcome = agent.run(&task()).await.expect("the retry succeeds");
    assert!(outcome.success);
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.summary, "recovered");
    assert_eq!(
        outcome.metrics.failures, 0,
        "provider retries are not step failures"
    );

    // An exhausted script fails loudly instead of repeating the last decision.
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let provider = MockProvider::scripted(vec![AgentDecision::ListWindows]);
    let mut agent = AgentLoop::new(client, provider, LoopConfig::default());
    let error = agent
        .run(&task())
        .await
        .expect_err("an exhausted script is fatal");
    assert!(
        matches!(error, Error::Provider(ProviderError::InvalidResponse(ref message)) if message.contains("exhausted")),
        "unexpected error: {error}"
    );
    assert_eq!(
        agent.history().len(),
        1,
        "the first step ran before the provider ran dry"
    );
}

/// The runtime's `ping` version is validated before the first decision when
/// `validate_protocol_version` is set; a mismatch is `Error::ProtocolVersion`.
#[tokio::test]
async fn protocol_version_mismatch_is_fatal() {
    let mut client = ScriptedClient::new();
    client.push(ScriptedResponse::Ping(adesk_agent::RuntimeInfo {
        protocol_version: 999,
        ..runtime_info()
    }));
    let handle = client.clone();
    let provider = MockProvider::scripted(vec![finish(true, "never asked")]);
    let mut agent = AgentLoop::new(client, provider, LoopConfig::default());

    let error = agent
        .run(&task())
        .await
        .expect_err("a version mismatch is fatal");

    match error {
        Error::ProtocolVersion { expected, got } => {
            assert_eq!(expected, PROTOCOL_VERSION);
            assert_eq!(got, 999);
        }
        other => panic!("expected Error::ProtocolVersion, got {other:?}"),
    }
    assert_eq!(handle.call_count(ClientMethod::Ping), 1);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 0);
    assert!(agent.history().is_empty());
}

/// Every executed decision appears in `history` with its status, action id and
/// observation, in order.
#[tokio::test]
async fn history_records_every_step() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(WindowList {
            windows: vec![window(1)],
            active_window_id: Some(WindowId(1)),
        }),
        ScriptedResponse::Action(ActionId(2)),
        observe(Some(ActionId(2))),
    ]);
    let handle = client.clone();
    let mut agent = loop_with(
        client,
        vec![
            AgentDecision::ListWindows,
            click_decision(1),
            finish(true, "done"),
        ],
        LoopConfig::default(),
    );

    let outcome = agent.run(&task()).await.expect("run succeeds");

    assert_eq!(outcome.history.len(), 3);
    let indices: Vec<u32> = outcome.history.iter().map(|record| record.step).collect();
    assert_eq!(indices, vec![0, 1, 2], "history is in step order");

    let listed = &outcome.history[0];
    assert_eq!(listed.decision, AgentDecision::ListWindows);
    assert_eq!(listed.status, StepStatus::Ok);
    assert_eq!(listed.action_id, None);
    assert_eq!(listed.observation, None);
    assert!(!listed.readback);

    let clicked = &outcome.history[1];
    assert_eq!(clicked.decision, click_decision(1));
    assert_eq!(clicked.status, StepStatus::Ok);
    assert_eq!(clicked.action_id, Some(ActionId(2)));
    let recorded = clicked
        .observation
        .as_ref()
        .expect("the automatic observation is recorded");
    assert_eq!(recorded.after_action, Some(ActionId(2)));
    assert_eq!(recorded.window_id, Some(WindowId(1)));
    assert!(!clicked.readback);

    let finished = &outcome.history[2];
    assert_eq!(finished.decision, finish(true, "done"));
    assert_eq!(finished.status, StepStatus::Finished);
    assert_eq!(finished.action_id, None);
    assert_eq!(finished.observation, None);

    assert_eq!(agent.history().len(), 3, "the loop keeps its own history");
    assert_eq!(handle.call_count(ClientMethod::Observe), 1);
    assert_eq!(handle.remaining(), 0);
}

/// `ScriptedClient` pops canned responses in order and records calls;
/// `MockProvider` records the contexts it receives.
#[tokio::test]
async fn scripted_client_and_mock_provider_record_calls() {
    let provider = Arc::new(MockProvider::scripted(vec![
        AgentDecision::ListWindows,
        finish(true, "done"),
    ]));
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();
    let mut agent = AgentLoop::new(
        client,
        SharedProvider(Arc::clone(&provider)),
        LoopConfig::default(),
    );

    let outcome = agent.run(&task()).await.expect("run succeeds");
    assert!(outcome.success);

    assert_eq!(handle.call_count(ClientMethod::Ping), 1);
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 1);
    assert_eq!(
        handle.remaining(),
        0,
        "every canned response was consumed in order"
    );
    let methods: Vec<ClientMethod> = handle.calls().into_iter().map(|call| call.method).collect();
    assert_eq!(methods, vec![ClientMethod::Ping, ClientMethod::ListWindows]);

    assert_eq!(provider.context_count(), 2, "one context per decision");
    assert_eq!(provider.remaining(), 0);
    let contexts = provider.contexts();
    assert_eq!(contexts[0].step, 0);
    assert_eq!(contexts[1].step, 1);
    assert_eq!(contexts[0].task, task().goal);
    assert!(
        contexts[0].runtime.is_some(),
        "ping facts are cached for the context"
    );
    assert_eq!(
        contexts[1].recent_actions.len(),
        1,
        "the executed step is in the next context"
    );
}
