//! Scripted, replayable provider — the default for tests and dry runs.
//!
//! A [`MockProvider`] plays a fixed list of [`ScriptEntry`] values. Each call
//! records the exact [`AgentContext`] it received, which is how the tests assert
//! that contexts stay bounded and that image rotation works.

use std::sync::Mutex;

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
/// * Runs off the end of the script → [`ProviderError::InvalidResponse`].
/// * `error` entries fail the call without advancing the recorded contexts.
#[derive(Debug)]
pub struct MockProvider {
    script: Vec<ScriptEntry>,
    name: String,
    state: Mutex<MockState>,
}

impl MockProvider {
    /// Build from a list of script entries.
    pub fn new(script: Vec<ScriptEntry>) -> Self {
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
    async fn complete(&self, _ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        todo!("phase 2: record context, pop next ScriptEntry, honour error/latency")
    }

    fn name(&self) -> &str {
        &self.name
    }
}
