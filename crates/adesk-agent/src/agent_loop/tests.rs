//! Inline unit tests for the loop.
//!
//! Everything here runs against the socket-free [`ScriptedClient`] and a
//! no-I/O provider: no socket, compositor, GPU or network.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use adesk_core::{
    ActionId, Button, ErrorCode, Observation, Position, Rect, Size, WindowId, WindowInfo,
    WindowState,
};
use adesk_proto::{ImageFormat, ImagePayload};
use async_trait::async_trait;

use super::{AgentLoop, LoopConfig, StepStatus};
use crate::client::{
    AccessibilityOutcome, CaptureOutcome, ObserveOutcome, RuntimeInfo, PROTOCOL_VERSION,
};
use crate::context::{AgentContext, TaskDescription};
use crate::decision::{AgentDecision, ObserveCondition};
use crate::error::ProviderError;
use crate::provider::{LlmProvider, MockProvider};
use crate::testing::{ClientMethod, ScriptedClient, ScriptedResponse};

/// A text-only provider: replays `decisions` and remembers whether any context it
/// was handed carried pixels.
#[derive(Debug)]
struct TextOnlyProvider {
    decisions: Vec<AgentDecision>,
    served: AtomicUsize,
    saw_image: AtomicBool,
}

impl TextOnlyProvider {
    fn new(decisions: Vec<AgentDecision>) -> Self {
        Self {
            decisions,
            served: AtomicUsize::new(0),
            saw_image: AtomicBool::new(false),
        }
    }

    /// Whether a context ever reached this provider with an image attached.
    fn saw_image(&self) -> bool {
        self.saw_image.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for TextOnlyProvider {
    async fn complete(
        &self,
        ctx: &AgentContext,
    ) -> std::result::Result<AgentDecision, ProviderError> {
        if ctx.image.is_some() || ctx.keyframe.is_some() {
            self.saw_image.store(true, Ordering::SeqCst);
        }
        let index = self.served.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .decisions
            .get(index)
            .cloned()
            .unwrap_or(AgentDecision::Finish {
                success: false,
                summary: String::from("text-only provider script exhausted"),
            }))
    }

    fn name(&self) -> &str {
        "text-only"
    }

    fn supports_images(&self) -> bool {
        false
    }
}

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

/// A quiet observation causally after `after_action`.
fn quiet_after(after_action: Option<ActionId>) -> ScriptedResponse {
    ScriptedResponse::Observe(ObserveOutcome {
        observation: Observation {
            window_id: Some(WindowId(1)),
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
        },
        image: None,
    })
}

/// Canned responses in the loop's exact call order for [`decisions`].
fn script() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Action(ActionId(5)),
        quiet_after(Some(ActionId(5))),
        quiet_after(Some(ActionId(5))),
        quiet_after(Some(ActionId(5))),
    ]
}

/// One click (carrying the automatic observation) plus two explicit
/// observations; the second one *demands* pixels.
fn decisions() -> Vec<AgentDecision> {
    let observe = |include_image: Option<bool>| AgentDecision::Observe {
        window_id: Some(WindowId(1)),
        after_action: None,
        until: ObserveCondition::Quiet { quiet_ms: 250 },
        timeout_ms: None,
        include_image,
        max_dimension: None,
        region: None,
    };
    vec![
        AgentDecision::Click {
            window_id: WindowId(1),
            position: Position::pixels(10, 20),
            button: Button::Left,
            count: 1,
        },
        observe(None),
        observe(Some(true)),
        AgentDecision::Finish {
            success: true,
            summary: "done".to_owned(),
        },
    ]
}

/// A capture readback carrying a frame, for the explicit-`capture` case.
fn captured_frame() -> CaptureOutcome {
    CaptureOutcome {
        image: ImagePayload {
            width: 4,
            height: 2,
            format: ImageFormat::Png,
            stride: None,
            data: String::from("Q0FQVFVSRQ=="),
            scale: 1.0,
        },
        window: WindowInfo {
            id: WindowId(1),
            app_id: None,
            title: Some(String::from("editor")),
            geometry: Rect::new(0, 0, 1280, 800),
            state: WindowState::Active,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq: 3,
            popup_count: 0,
        },
        commit_seq: 3,
        changed_regions: Vec::new(),
    }
}

/// `ping` then one explicit capture, in the loop's exact call order.
fn capture_script() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::Ping(runtime_info()),
        ScriptedResponse::Capture(captured_frame()),
    ]
}

/// One explicit capture, then `Finish`; the capture is requested without any
/// image flag, because `Capture` never has one.
fn capture_decisions() -> Vec<AgentDecision> {
    vec![
        AgentDecision::Capture {
            window_id: WindowId(1),
            region: None,
            max_dimension: None,
        },
        AgentDecision::Finish {
            success: true,
            summary: "captured".to_owned(),
        },
    ]
}

/// The `observe` call summaries a run recorded, in order.
fn observe_summaries(client: &ScriptedClient) -> Vec<String> {
    client
        .calls()
        .into_iter()
        .filter(|call| call.method == ClientMethod::Observe)
        .map(|call| call.summary)
        .collect()
}

/// `LlmProvider::supports_images` is load-bearing: a text-only provider never
/// asks the runtime for pixels and never sees any, even when the decision
/// explicitly requests an image or is an explicit `capture`; an image-capable
/// provider running the identical script is unaffected.
#[tokio::test]
async fn image_requests_are_gated_on_provider_capability() {
    let task = TaskDescription::new("open the settings dialog and enable dark mode");

    // Text-only provider: every observation is pixel-free, whatever was asked.
    let stub = Arc::new(TextOnlyProvider::new(decisions()));
    let mut client = ScriptedClient::new();
    client.push_all(script());
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&stub), LoopConfig::default());
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    let calls = observe_summaries(&handle);
    assert_eq!(calls.len(), 3, "click + two explicit observations");
    for summary in &calls {
        assert!(summary.contains("include_image=false"), "{summary}");
    }
    assert_eq!(handle.call_count(ClientMethod::CaptureWindow), 0);
    assert!(
        !stub.saw_image(),
        "a text-only provider must never receive pixels"
    );
    assert_eq!(
        outcome.metrics.gpu_readbacks, 0,
        "no frame was ever rendered"
    );

    // Same script, image-capable provider: the gate opens.
    let mut client = ScriptedClient::new();
    client.push_all(script());
    let handle = client.clone();
    let mut agent = AgentLoop::new(
        client,
        MockProvider::scripted(decisions()),
        LoopConfig::default(),
    );
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    let calls = observe_summaries(&handle);
    assert_eq!(calls.len(), 3);
    for summary in &calls {
        assert!(summary.contains("include_image=true"), "{summary}");
    }

    // An explicit `capture` carries no image flag, so its readback is the one
    // path into the context that an observation request cannot gate: the frame
    // is real (it is a GPU readback, and the step is recorded as one) but a
    // text-only provider must still never see it.
    let stub = Arc::new(TextOnlyProvider::new(capture_decisions()));
    let mut client = ScriptedClient::new();
    client.push_all(capture_script());
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Arc::clone(&stub), LoopConfig::default());
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::CaptureWindow), 1);
    assert_eq!(
        outcome.metrics.gpu_readbacks, 1,
        "the readback itself did happen"
    );
    assert!(
        !stub.saw_image(),
        "an explicit capture must not hand pixels to a text-only provider"
    );
    assert_eq!(
        agent.context().image_count(),
        0,
        "no frame was attached to the text-only provider's context"
    );
}

/// A window with `id`, for the `get_window` refresh the accessibility upsert
/// issues for an untracked window.
fn window(id: WindowId) -> WindowInfo {
    WindowInfo {
        id,
        app_id: None,
        title: Some(String::from("editor")),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: None,
        created_seq: 1,
        last_commit_seq: 3,
        popup_count: 0,
    }
}

/// An accessibility outline for `window_id`.
fn accessibility_outcome(window_id: WindowId, text: &str) -> AccessibilityOutcome {
    AccessibilityOutcome {
        window_id,
        node_count: 3,
        truncated: false,
        text: text.to_owned(),
    }
}

/// An `observe` on `window_id`, explicitly without an image request.
fn observe_decision(window_id: WindowId) -> AgentDecision {
    AgentDecision::Observe {
        window_id: Some(window_id),
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

/// With [`LoopConfig::include_accessibility`] on, an `observe` decision pulls the
/// observed window's accessibility outline through one extra `accessibility_tree`
/// call, and the outline reaches the next provider context — with no readback.
#[tokio::test]
async fn accessibility_enrichment_reaches_the_context_without_pixels() {
    let task = TaskDescription::new("read the preferences dialog");
    let outline = "- window \"Preferences\"\n  - button \"Dark Mode\"\n";

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        quiet_after(None),
        ScriptedResponse::AccessibilityTree(accessibility_outcome(WindowId(1), outline)),
        ScriptedResponse::Window(window(WindowId(1))),
    ]);
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![
        observe_decision(WindowId(1)),
        finish(),
    ]));
    let config = LoopConfig {
        include_accessibility: true,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), config);
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 1);
    assert_eq!(
        outcome.metrics.gpu_readbacks, 0,
        "text-first enrichment never reads pixels back"
    );
    assert!(
        provider
            .contexts()
            .iter()
            .any(|context| context.accessibility.as_deref() == Some(outline)),
        "the outline must reach the provider context"
    );
    assert!(
        provider
            .contexts()
            .iter()
            .all(|context| context.image.is_none() && context.keyframe.is_none()),
        "no frame is ever attached to the context"
    );
}

/// With the gate off (the default) the same run makes no accessibility call: the
/// script carries no `AccessibilityTree` response, so an extra call would exhaust
/// it and panic — yet the run still finishes and no outline reaches the context.
#[tokio::test]
async fn accessibility_is_not_fetched_when_the_gate_is_off() {
    let task = TaskDescription::new("read the preferences dialog");
    assert!(
        !LoopConfig::default().include_accessibility,
        "the capability is opt-in and off by default"
    );

    let mut client = ScriptedClient::new();
    client.push_all([ScriptedResponse::Ping(runtime_info()), quiet_after(None)]);
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![
        observe_decision(WindowId(1)),
        finish(),
    ]));
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 0);
    assert!(
        provider
            .contexts()
            .iter()
            .all(|context| context.accessibility.is_none()),
        "the disabled capability adds nothing to the context"
    );
}

/// An unavailable accessibility backend degrades benignly: the run survives and
/// the context simply carries no outline.
#[tokio::test]
async fn unavailable_accessibility_is_non_fatal() {
    let task = TaskDescription::new("read the preferences dialog");

    let mut client = ScriptedClient::new();
    client.push_all([
        ScriptedResponse::Ping(runtime_info()),
        quiet_after(None),
        ScriptedResponse::Error(adesk_core::Error::new(
            ErrorCode::NotSupported,
            "no accessibility backend",
        )),
    ]);
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![
        observe_decision(WindowId(1)),
        finish(),
    ]));
    let config = LoopConfig {
        include_accessibility: true,
        retry_backoff_ms: 0,
        ..LoopConfig::default()
    };
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), config);
    let outcome = agent
        .run(&task)
        .await
        .expect("an unavailable backend must not end the run");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 1);
    assert!(
        provider
            .contexts()
            .iter()
            .all(|context| context.accessibility.is_none()),
        "an unavailable backend leaves no outline"
    );
}

/// The explicit `accessibility_tree` decision is gated the same way: with the
/// capability off it makes no runtime call and does not fail the step.
#[tokio::test]
async fn explicit_accessibility_decision_is_gated_and_non_fatal() {
    let task = TaskDescription::new("read the preferences dialog");

    let mut client = ScriptedClient::new();
    // No `AccessibilityTree` response: a gated decision must not call out.
    client.push(ScriptedResponse::Ping(runtime_info()));
    let handle = client.clone();

    let provider = Arc::new(MockProvider::scripted(vec![
        AgentDecision::AccessibilityTree {
            window_id: Some(WindowId(1)),
            max_nodes: None,
        },
        finish(),
    ]));
    let mut agent = AgentLoop::new(client, Arc::clone(&provider), LoopConfig::default());
    let outcome = agent.run(&task).await.expect("run succeeds");

    assert!(outcome.success);
    assert_eq!(handle.call_count(ClientMethod::AccessibilityTree), 0);
    assert_eq!(
        outcome.history[0].status,
        StepStatus::Ok,
        "a gated decision is a benign no-op, not a failed step"
    );
}
