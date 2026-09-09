//! Scripted, replayable provider — the default for tests and dry runs.
//!
//! A [`MockProvider`] plays a fixed list of [`ScriptEntry`] values. Each call
//! records the exact [`AgentContext`] it received, which is how the tests assert
//! that contexts stay bounded and that image rotation works.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::AgentContext;
use crate::decision::AgentDecision;
use crate::error::ProviderError;
use crate::provider::LlmProvider;

/// One scripted provider response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEntry {
    /// Decision to return.
    pub decision: AgentDecision,
    /// When set, the call fails with this error instead of returning `decision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderError>,
    /// Simulated decision latency, recorded in metrics.
    #[serde(default)]
    pub latency_ms: u64,
}

impl ScriptEntry {
    /// A scripted successful decision.
    pub fn decision(decision: AgentDecision) -> Self {
        Self {
            decision,
            error: None,
            latency_ms: 0,
        }
    }

    /// A scripted provider failure.
    pub fn error(error: ProviderError) -> Self {
        Self {
            decision: AgentDecision::Finish {
                success: false,
                summary: String::from("provider error"),
            },
            error: Some(error),
            latency_ms: 0,
        }
    }

    /// Attach a simulated latency.
    pub fn with_latency(mut self, latency_ms: u64) -> Self {
        self.latency_ms = latency_ms;
        self
    }
}

/// Mutable replay state behind a mutex so `complete(&self)` can record contexts.
#[derive(Debug)]
struct MockState {
    cursor: usize,
    contexts: Vec<AgentContext>,
}

/// Scripted/replayable [`LlmProvider`].
///
/// * An empty script is replaced by the built-in dry-run sequence
///   (`list_windows` → `finish`), so `--provider mock` smoke-runs a task without
///   any LLM or script file.
/// * Runs off the end of the script → [`ProviderError::InvalidResponse`]; the
///   provider never repeats a decision silently.
/// * `error` entries fail the call and are consumed like any other entry, so a
///   retry reaches the next entry instead of looping on the failure.
#[derive(Debug)]
pub struct MockProvider {
    script: Vec<ScriptEntry>,
    name: String,
    state: Mutex<MockState>,
}

/// Built-in dry-run sequence used when no script is configured.
///
/// The loop's own `ping` happens before the first decision, so this is the
/// `ping`/`list_windows`/`finish` sequence without a wire-level `ping` decision.
fn dry_run_script() -> Vec<ScriptEntry> {
    vec![
        ScriptEntry::decision(AgentDecision::ListWindows),
        ScriptEntry::decision(AgentDecision::Finish {
            success: true,
            summary: String::from("dry run: mock provider has no script"),
        }),
    ]
}

impl MockProvider {
    /// Build from a list of script entries.
    ///
    /// An empty `script` selects the built-in dry-run sequence.
    pub fn new(script: Vec<ScriptEntry>) -> Self {
        let script = if script.is_empty() {
            dry_run_script()
        } else {
            script
        };
        Self {
            script,
            name: String::from("mock"),
            state: Mutex::new(MockState {
                cursor: 0,
                contexts: Vec::new(),
            }),
        }
    }

    /// Build from plain decisions (the common test case).
    pub fn scripted(decisions: Vec<AgentDecision>) -> Self {
        Self::new(decisions.into_iter().map(ScriptEntry::decision).collect())
    }

    /// Parse a replay script from JSON (`[ScriptEntry]` or `[AgentDecision]`).
    pub fn from_json(json: &str) -> Result<Self, ProviderError> {
        if let Ok(entries) = serde_json::from_str::<Vec<ScriptEntry>>(json) {
            return Ok(Self::new(entries));
        }
        serde_json::from_str::<Vec<AgentDecision>>(json)
            .map(Self::scripted)
            .map_err(|e| ProviderError::InvalidResponse(format!("invalid mock script: {e}")))
    }

    /// Override the provider name (shows up in reports).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Contexts received so far, in call order (cloned snapshot).
    pub fn contexts(&self) -> Vec<AgentContext> {
        self.lock().contexts.clone()
    }

    /// Number of contexts received so far.
    pub fn context_count(&self) -> usize {
        self.lock().contexts.len()
    }

    /// Script entries not yet consumed.
    pub fn remaining(&self) -> usize {
        self.script.len().saturating_sub(self.lock().cursor)
    }

    /// Rewind to the start of the script and forget recorded contexts.
    pub fn reset(&mut self) {
        let mut state = self.lock();
        state.cursor = 0;
        state.contexts.clear();
    }

    /// Poison-tolerant lock helper.
    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        // Record the context and consume the entry under one lock, then release
        // it before any await so concurrent callers never block a sleep.
        let entry = {
            let mut state = self.lock();
            state.contexts.push(ctx.clone());
            match self.script.get(state.cursor) {
                Some(entry) => {
                    state.cursor += 1;
                    Some(entry.clone())
                }
                None => None,
            }
        };
        let Some(entry) = entry else {
            return Err(ProviderError::InvalidResponse(format!(
                "mock provider script exhausted after {} entries; add more ScriptEntry values or call reset()",
                self.script.len()
            )));
        };

        if entry.latency_ms > 0 {
            // `sleep` panics without a reactor, so only sleep inside a tokio
            // runtime; a scripted latency outside one is simply not simulated.
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::time::sleep(Duration::from_millis(entry.latency_ms)).await;
            } else {
                tracing::warn!(
                    latency_ms = entry.latency_ms,
                    "no tokio runtime; scripted provider latency is not simulated"
                );
            }
        }

        match entry.error {
            Some(error) => Err(error),
            None => Ok(entry.decision),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Minimal bounded context (no socket, no runtime).
    fn ctx(task: &str) -> AgentContext {
        AgentContext {
            task: task.to_owned(),
            success_criteria: None,
            step: 0,
            max_steps: 20,
            runtime: None,
            windows: Vec::new(),
            active_window: None,
            apps: Vec::new(),
            recent_actions: Vec::new(),
            recent_events: Vec::new(),
            observation: None,
            last_error: None,
            image: None,
            keyframe: None,
        }
    }

    fn finish() -> AgentDecision {
        AgentDecision::Finish {
            success: true,
            summary: String::from("done"),
        }
    }

    #[tokio::test]
    async fn empty_script_runs_the_builtin_dry_run_sequence() {
        let provider = MockProvider::new(Vec::new());

        assert_eq!(provider.remaining(), 2, "dry run has two decisions");
        assert_eq!(
            provider.complete(&ctx("dry run")).await.unwrap(),
            AgentDecision::ListWindows
        );
        assert_eq!(provider.remaining(), 1);
        match provider.complete(&ctx("dry run")).await.unwrap() {
            AgentDecision::Finish { success, summary } => {
                assert!(success);
                assert!(summary.contains("dry run"), "summary: {summary}");
            }
            other => panic!("expected Finish, got {other:?}"),
        }
        assert_eq!(provider.remaining(), 0);
    }

    #[tokio::test]
    async fn scripted_provider_replays_in_order_and_records_contexts() {
        let provider = MockProvider::scripted(vec![AgentDecision::ListWindows, finish()]);

        assert_eq!(provider.name(), "mock");
        assert!(provider.supports_images());
        assert_eq!(provider.context_count(), 0);

        assert_eq!(
            provider.complete(&ctx("first")).await.unwrap(),
            AgentDecision::ListWindows
        );
        assert_eq!(provider.complete(&ctx("second")).await.unwrap(), finish());

        assert_eq!(provider.context_count(), 2);
        let contexts = provider.contexts();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].task, "first");
        assert_eq!(contexts[1].task, "second");
        assert_eq!(provider.remaining(), 0);
    }

    #[tokio::test]
    async fn exhausted_script_fails_loudly_instead_of_repeating() {
        let provider = MockProvider::scripted(vec![AgentDecision::ListWindows]);
        provider.complete(&ctx("one")).await.unwrap();

        let err = provider.complete(&ctx("two")).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("exhausted"), "message: {message}");
        // The failed call is still recorded, but the cursor never moves past the
        // end of the script.
        assert_eq!(provider.context_count(), 2);
        assert_eq!(provider.remaining(), 0);
    }

    #[tokio::test]
    async fn error_entries_fail_once_and_are_consumed() {
        let provider = MockProvider::new(vec![
            ScriptEntry::error(ProviderError::Transport(String::from("boom"))),
            ScriptEntry::decision(finish()),
        ]);

        let err = provider.complete(&ctx("retry")).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Transport(ref message) if message == "boom"),
            "unexpected error: {err}"
        );
        assert_eq!(provider.remaining(), 1, "the failure must be consumed");

        // A retry reaches the next entry rather than the same failure again.
        assert_eq!(provider.complete(&ctx("retry")).await.unwrap(), finish());
        assert_eq!(provider.context_count(), 2);
    }

    #[tokio::test]
    async fn latency_entries_delay_the_decision() {
        let provider = MockProvider::new(vec![ScriptEntry::decision(finish()).with_latency(5)]);

        let started = Instant::now();
        provider.complete(&ctx("slow")).await.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(5),
            "simulated latency must be observable, got {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn reset_rewinds_the_script_and_forgets_contexts() {
        let mut provider = MockProvider::scripted(vec![AgentDecision::ListWindows, finish()]);
        provider.complete(&ctx("first")).await.unwrap();

        provider.reset();
        assert_eq!(provider.remaining(), 2);
        assert_eq!(provider.context_count(), 0);
        assert_eq!(
            provider.complete(&ctx("second")).await.unwrap(),
            AgentDecision::ListWindows
        );
    }

    #[test]
    fn from_json_accepts_entries_and_bare_decisions() {
        let entries =
            MockProvider::from_json(r#"[{"decision":{"op":"list_windows"},"latency_ms":3}]"#)
                .expect("entry shape");
        assert_eq!(entries.remaining(), 1);

        let bare = MockProvider::from_json(r#"[{"op":"finish","success":false,"summary":"nope"}]"#)
            .expect("decision shape");
        assert_eq!(bare.remaining(), 1);

        let err = MockProvider::from_json("not json").unwrap_err();
        assert!(matches!(err, ProviderError::InvalidResponse(_)));
    }

    #[test]
    fn with_name_overrides_the_provider_name() {
        let provider = MockProvider::scripted(vec![finish()]).with_name("replay");
        assert_eq!(provider.name(), "replay");
    }
}
