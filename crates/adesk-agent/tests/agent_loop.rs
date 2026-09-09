//! Loop behavior against the socket-free `ScriptedClient` + `MockProvider`.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
//!
//! Every test here must run without a socket, compositor, GPU or network.
#![cfg(feature = "test-support")]

use adesk_agent::testing::ScriptedClient;
use adesk_agent::{AgentDecision, AgentLoop, LoopConfig, MockProvider};

/// Build a loop over a scripted client and mock provider (used by the phase-2
/// tests below).
#[allow(dead_code)]
fn loop_with(
    client: ScriptedClient,
    decisions: Vec<AgentDecision>,
    config: LoopConfig,
) -> AgentLoop<ScriptedClient, MockProvider> {
    let _ = (client, decisions, config);
    todo!("phase 2: test helper")
}

/// A `Finish { success: true }` decision ends the run with `StopReason::Finished`,
/// one step, and no actions counted.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn finish_ends_the_run() {
    todo!("phase 2: run loop, assert outcome.success && steps == 1")
}

/// After a seat input action the loop observes `quiet` with
/// `after_action = Some(last_action_id)`, so the observation carries causal
/// history. Assert one `Click` call, one `Observe` call, and that the observe
/// request's `after_action` equals the click's action id.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn input_is_followed_by_quiet_observation_with_after_action() {
    todo!("phase 2: assert after_action wiring")
}

/// The loop never exceeds `LoopConfig::max_steps` and stops with
/// `StopReason::StepBudgetExhausted` when the provider keeps issuing actions.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn step_budget_is_enforced() {
    todo!("phase 2: script > max_steps decisions, assert steps == max_steps")
}

/// A retryable error (transport) retries the same step up to
/// `retries_per_step`, then the run continues; metrics record the failure.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn retryable_errors_are_retried_then_recovered() {
    todo!("phase 2: push Error(transport), then success; assert retry + recovery")
}

/// A recoverable error (`unknown_window`) does not end the run: the loop records
/// the failure, refreshes the window list, and the agent's next decision
/// succeeds — `recoveries == 1`.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn unknown_window_is_recovered_by_refresh() {
    todo!("phase 2: script click -> unknown_window -> list_windows -> click -> finish")
}

/// `max_consecutive_failures` consecutive failures stop the run with
/// `StopReason::FailureBudgetExhausted` and `success == false`.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn failure_budget_stops_the_run() {
    todo!("phase 2: script repeated errors, assert stop reason")
}

/// A provider error is surfaced as `Error::Provider`; a provider that runs out of
/// script entries fails loudly instead of silently repeating.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn provider_failures_are_classified() {
    todo!("phase 2: MockProvider::new(vec![ScriptEntry::error(..)])")
}

/// The runtime's `ping` version is validated before the first decision when
/// `validate_protocol_version` is set; a mismatch is `Error::ProtocolVersion`.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn protocol_version_mismatch_is_fatal() {
    todo!("phase 2: ping with protocol_version = 999")
}

/// Every executed decision appears in `history` with its status, action id and
/// observation, in order.
#[tokio::test]
#[ignore = "phase 2: loop not implemented"]
async fn history_records_every_step() {
    todo!("phase 2: assert StepRecord sequence")
}

/// `ScriptedClient` pops canned responses in order and records calls;
/// `MockProvider` records the contexts it receives.
#[tokio::test]
#[ignore = "phase 2: fake not implemented"]
async fn scripted_client_and_mock_provider_record_calls() {
    todo!("phase 2: assert call_count + context_count")
}
