//! The synthetic `DummyVlmProvider` drives the real `AgentLoop` to clean
//! termination.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
//!
//! Every test here runs without a socket, compositor, GPU or network: the loop is
//! wired against the socket-free `ScriptedClient`, and the dummy provider itself
//! performs no I/O. The loop's client calls are positional against the script, so
//! one `ScriptedResponse` is pushed per expected call.
#![cfg(feature = "test-support")]

use adesk_agent::provider::{DummyConfig, DummyMode, DummyVlmProvider};
use adesk_agent::testing::{ClientMethod, ScriptedClient, ScriptedResponse};
use adesk_agent::{
    AgentDecision, AgentLoop, LoopConfig, RuntimeInfo, StopReason, TaskDescription, WindowList,
    PROTOCOL_VERSION,
};
use adesk_core::Size;

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

/// An empty window list (no windows known yet).
fn empty_windows() -> WindowList {
    WindowList {
        windows: Vec::new(),
        active_window_id: None,
    }
}

/// The task every test runs (only the goal matters to the loop).
fn task() -> TaskDescription {
    TaskDescription::new("exercise the dummy VLM provider end to end")
}

/// A no-backoff loop config: the tests must never sleep on wall-clock time.
fn config() -> LoopConfig {
    LoopConfig {
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    }
}

/// Fixed mode replays its one decision, then the provider's fallback `Finish`
/// ends the run cleanly: two steps, one `list_windows` call and no client call
/// for the finish.
#[tokio::test]
async fn fixed_dummy_provider_terminates_the_loop() {
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();

    let provider = DummyVlmProvider::fixed([AgentDecision::ListWindows]);
    let mut agent = AgentLoop::new(client, provider, config());

    let outcome = agent
        .run(&task())
        .await
        .expect("the run terminates cleanly");

    assert!(outcome.success);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert_eq!(
        outcome.steps, 2,
        "one ListWindows step, then the fallback Finish"
    );
    assert_eq!(outcome.history.len(), 2);
    assert_eq!(outcome.history[0].decision, AgentDecision::ListWindows);
    assert!(
        matches!(
            outcome.history[1].decision,
            AgentDecision::Finish { success: true, .. }
        ),
        "the exhausted fixed script falls back to a successful Finish: {:?}",
        outcome.history[1].decision
    );
    assert_eq!(handle.call_count(ClientMethod::ListWindows), 1);
    assert_eq!(
        handle.remaining(),
        0,
        "the script was consumed exactly, with no call for the finish"
    );
}

/// Random mode with `finish_probability = 0.0` can only terminate through the
/// step budget: it draws exactly `step_budget` pooled decisions and then the
/// unconditional `Finish`, so the loop ends within `step_budget + 1` steps.
#[tokio::test]
async fn random_dummy_provider_terminates_via_the_step_budget() {
    const N: u32 = 3;

    let mut client = ScriptedClient::new();
    client.push(ScriptedResponse::Ping(runtime_info()));
    for _ in 0..N {
        client.push(ScriptedResponse::Windows(empty_windows()));
    }
    let handle = client.clone();

    let provider = DummyVlmProvider::from_config(DummyConfig {
        mode: DummyMode::Random,
        seed: 42,
        pool: vec![AgentDecision::ListWindows],
        finish_probability: 0.0,
        step_budget: N,
        ..DummyConfig::default()
    });
    let mut agent = AgentLoop::new(client, provider, config());

    let outcome = agent
        .run(&task())
        .await
        .expect("the run terminates cleanly");

    assert!(outcome.success);
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    assert!(
        outcome.steps <= N + 1,
        "the loop must terminate within step_budget + 1 = {} steps, got {}",
        N + 1,
        outcome.steps
    );
    assert_eq!(
        outcome.steps,
        N + 1,
        "with finish_probability = 0.0 the budget forces the finish"
    );
    assert_eq!(handle.call_count(ClientMethod::ListWindows), N as usize);
    assert_eq!(
        handle.remaining(),
        0,
        "one drawn decision per step, no call for the finish"
    );
}
