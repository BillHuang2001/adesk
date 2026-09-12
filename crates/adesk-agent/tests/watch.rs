//! Idle/watch mode against the socket-free `ScriptedClient` + `MockProvider`.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
//!
//! Every test here runs without a socket, compositor, GPU or network: the wake
//! source is a scripted `wait_for_events` response, never a real runtime. The
//! tests prove the wake→handle→idle cycle and both watch budgets.

mod common;

use std::sync::Arc;

use adesk_agent::testing::{ClientMethod, ScriptedClient, ScriptedResponse};
use adesk_agent::{
    AgentDecision, AgentLoop, LoopConfig, MockProvider, TaskDescription, WaitOutcome, WatchConfig,
    WatchStopReason,
};
use adesk_core::{EventKind, Notification, NotificationId, NotificationUrgency, RuntimeEvent};
use common::{empty_windows, runtime_info};

/// The standing job every test runs (only the goal matters to the loop).
fn task() -> TaskDescription {
    TaskDescription::new("handle the notification")
}

/// A `Finish` decision with the given outcome.
fn finish(success: bool, summary: &str) -> AgentDecision {
    AgentDecision::Finish {
        success,
        summary: summary.to_owned(),
    }
}

/// A `notification` event that wakes the agent.
fn notification_event(seq: u64) -> RuntimeEvent {
    RuntimeEvent::Notification {
        seq,
        ts_ms: seq * 10,
        notification: Notification {
            id: NotificationId(1),
            source: Some("test".to_owned()),
            title: "Build finished".to_owned(),
            body: String::new(),
            urgency: NotificationUrgency::Normal,
            category: None,
            actions: Vec::new(),
            hints: Default::default(),
            posted_seq: seq,
            posted_ts_ms: seq * 10,
            dismissed: false,
            closed_seq: None,
            close_reason: None,
            timeout_ms: None,
        },
    }
}

/// A `wait_for_events` result that woke the agent with `events`.
fn woke(events: Vec<RuntimeEvent>, seq: u64) -> ScriptedResponse {
    ScriptedResponse::WaitEvents(WaitOutcome {
        events,
        timed_out: false,
        elapsed_ms: 5,
        seq,
    })
}

/// A timed-out (idle) `wait_for_events` result.
fn idle(seq: u64) -> ScriptedResponse {
    ScriptedResponse::WaitEvents(WaitOutcome {
        events: Vec::new(),
        timed_out: true,
        elapsed_ms: 30_000,
        seq,
    })
}

/// A wakeup wakes the agent, the standing job runs, and the loop returns to
/// idle: one `Wakeup` (carrying the notification) whose job succeeded, one idle
/// wait, and `IdleBudget` as the stop reason. The wake event must also reach the
/// provider's context.
#[tokio::test]
async fn wake_handle_then_return_to_idle() {
    let provider = Arc::new(MockProvider::scripted(vec![
        AgentDecision::ListWindows,
        finish(true, "handled"),
    ]));
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        woke(vec![notification_event(1)], 1),
        ScriptedResponse::Windows(empty_windows()),
        idle(1),
    ]);
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());

    let watch = WatchConfig {
        max_idle_waits: Some(1),
        ..WatchConfig::default()
    };
    let outcome = agent
        .run_watch(&task(), &watch)
        .await
        .expect("watch succeeds");

    assert_eq!(outcome.stop_reason, WatchStopReason::IdleBudget);
    assert_eq!(
        outcome.idle_waits, 1,
        "the loop returned to idle after handling"
    );
    assert_eq!(outcome.wakeups.len(), 1);
    let wakeup = &outcome.wakeups[0];
    assert_eq!(wakeup.events.len(), 1);
    assert_eq!(wakeup.events[0].kind(), EventKind::Notification);
    assert_eq!(wakeup.seq, 1);
    assert!(wakeup.outcome.success);
    assert_eq!(wakeup.outcome.summary, "handled");
    assert_eq!(wakeup.outcome.history.len(), 2, "two job steps");

    // Two waits: the wake, then the idle wait that ended the watch.
    assert_eq!(handle.call_count(ClientMethod::WaitForEvents), 2);
    assert_eq!(handle.call_count(ClientMethod::Ping), 1, "validated once");
    assert_eq!(handle.remaining(), 0, "every canned response was consumed");
    // The wake event reached the provider's bounded context.
    let contexts = provider.contexts();
    assert!(!contexts.is_empty());
    assert!(
        contexts[0]
            .recent_events
            .iter()
            .any(|event| event.kind == EventKind::Notification && event.detail.contains("posted")),
        "recent_events: {:?}",
        contexts[0].recent_events
    );
}

/// `max_wakeups` stops the loop right after the specified number of handled
/// wakeups.
#[tokio::test]
async fn wakeup_budget_stops_the_watch() {
    let provider = Arc::new(MockProvider::scripted(vec![
        AgentDecision::ListWindows,
        finish(true, "handled"),
    ]));
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        woke(vec![notification_event(1)], 1),
        ScriptedResponse::Windows(empty_windows()),
    ]);
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());

    let watch = WatchConfig {
        max_wakeups: Some(1),
        ..WatchConfig::default()
    };
    let outcome = agent
        .run_watch(&task(), &watch)
        .await
        .expect("watch succeeds");

    assert_eq!(outcome.stop_reason, WatchStopReason::WakeupBudget);
    assert_eq!(outcome.wakeups.len(), 1);
    assert_eq!(outcome.idle_waits, 0);
    assert_eq!(handle.call_count(ClientMethod::WaitForEvents), 1);
    assert_eq!(handle.remaining(), 0, "every canned response was consumed");
}

/// With only timed-out waits, the loop stays idle and stops on the idle budget
/// without ever running the standing job.
#[tokio::test]
async fn idle_budget_stops_a_watch_with_no_events() {
    let provider = Arc::new(MockProvider::scripted(Vec::new()));
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        idle(0),
        idle(0),
        idle(0),
    ]);
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());

    let watch = WatchConfig {
        max_idle_waits: Some(3),
        ..WatchConfig::default()
    };
    let outcome = agent
        .run_watch(&task(), &watch)
        .await
        .expect("watch succeeds");

    assert_eq!(outcome.stop_reason, WatchStopReason::IdleBudget);
    assert_eq!(outcome.idle_waits, 3);
    assert!(outcome.wakeups.is_empty());
    assert_eq!(handle.call_count(ClientMethod::WaitForEvents), 3);
    assert_eq!(provider.context_count(), 0, "idle waits never run a job");
}

/// The wake wait is issued through the AGP `wait_for_events` seam with the
/// configured filter (kinds/timeout/max_events).
#[tokio::test]
async fn wait_for_events_is_issued_with_the_wake_filter() {
    let provider = Arc::new(MockProvider::scripted(vec![finish(true, "done")]));
    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        woke(vec![notification_event(3)], 3),
    ]);
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());

    let watch = WatchConfig {
        wait_timeout_ms: 4_000,
        max_events: 8,
        max_wakeups: Some(1),
        ..WatchConfig::default()
    };
    let outcome = agent
        .run_watch(&task(), &watch)
        .await
        .expect("watch succeeds");

    assert_eq!(outcome.stop_reason, WatchStopReason::WakeupBudget);
    assert_eq!(handle.call_count(ClientMethod::WaitForEvents), 1);
    assert_eq!(
        handle.calls()[1].summary,
        "kinds=Some([Notification, NotificationAction]) window_id=None \
         timeout_ms=4000 max_events=8 since_seq=None"
    );
}
