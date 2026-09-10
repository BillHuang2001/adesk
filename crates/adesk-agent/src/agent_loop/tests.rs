//! Inline unit tests for the loop.
//!
//! Everything here runs against the socket-free [`ScriptedClient`] and a
//! no-I/O provider: no socket, compositor, GPU or network.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use adesk_core::{ActionId, Button, Observation, Position, Rect, Size, WindowId};
use async_trait::async_trait;

use super::{AgentLoop, LoopConfig};
use crate::client::{ObserveOutcome, RuntimeInfo, PROTOCOL_VERSION};
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

/// Handle keeping the stub reachable after the loop takes its provider by value
/// (`async-trait` provides no blanket `impl LlmProvider for Box<dyn LlmProvider>`).
#[derive(Debug)]
struct Shared(Arc<TextOnlyProvider>);

#[async_trait]
impl LlmProvider for Shared {
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
/// explicitly requests an image; an image-capable provider running the identical
/// script is unaffected.
#[tokio::test]
async fn image_requests_are_gated_on_provider_capability() {
    let task = TaskDescription::new("open the settings dialog and enable dark mode");

    // Text-only provider: every observation is pixel-free, whatever was asked.
    let stub = Arc::new(TextOnlyProvider::new(decisions()));
    let mut client = ScriptedClient::new();
    client.push_all(script());
    let handle = client.clone();
    let mut agent = AgentLoop::new(client, Shared(Arc::clone(&stub)), LoopConfig::default());
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
}
