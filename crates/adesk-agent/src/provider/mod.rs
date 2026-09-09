//! Provider-agnostic LLM interface and implementations.
//!
//! The agent depends only on [`LlmProvider`]: given a bounded [`AgentContext`],
//! return one [`AgentDecision`]. [`MockProvider`] replays a script (default in
//! tests and for `--provider mock` dry runs); [`OpenAiCompatProvider`] speaks the
//! OpenAI `/chat/completions` shape against any compatible `base_url`.

pub mod mock;
pub mod openai;

pub use mock::{MockProvider, ScriptEntry};
pub use openai::{ImageDetail, OpenAiCompatProvider, OpenAiConfig};

use async_trait::async_trait;

use crate::context::AgentContext;
use crate::decision::AgentDecision;
use crate::error::ProviderError;

/// The single LLM seam: turn a bounded context into one decision.
///
/// Object-safe: the binary holds a `Box<dyn LlmProvider>`. Implementations must
/// be `Send + Sync` because the loop may run inside a tokio task.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Produce the next decision for `ctx`.
    ///
    /// Errors are provider problems (network, HTTP, parsing); the loop classifies
    /// them and retries transient ones.
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError>;

    /// Stable provider name, used in reports and logs.
    fn name(&self) -> &str;

    /// Whether the provider accepts images in the context.
    fn supports_images(&self) -> bool {
        true
    }
}

/// Which provider backend to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderKind {
    /// Scripted/replayable provider ([`MockProvider`]); default for tests and dry runs.
    Mock,
    /// OpenAI-compatible `/chat/completions` endpoint ([`OpenAiCompatProvider`]).
    #[value(name = "openai")]
    OpenAi,
}

impl ProviderKind {
    /// Stable name, matching the `--provider` CLI values.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::OpenAi => "openai",
        }
    }
}

/// Provider selection and configuration, resolved from CLI flags and env vars.
///
/// Env fallbacks: `ADESK_AGENT_PROVIDER`, `ADESK_AGENT_MODEL`,
/// `ADESK_AGENT_BASE_URL`, `ADESK_AGENT_API_KEY` (plus `OPENAI_API_KEY` for the
/// OpenAI-compatible provider).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Backend to build.
    pub kind: ProviderKind,
    /// Model name (provider default when `None`).
    pub model: Option<String>,
    /// Endpoint base URL (provider default when `None`).
    pub base_url: Option<String>,
    /// API key (env fallback when `None`).
    pub api_key: Option<String>,
    /// Sampling temperature.
    pub temperature: f32,
    /// Response token cap.
    pub max_tokens: u32,
    /// Per-request HTTP timeout.
    pub timeout_ms: u64,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Script handed to [`MockProvider`] (empty = built-in dry-run script).
    pub mock_script: Vec<AgentDecision>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::Mock,
            model: None,
            base_url: None,
            api_key: None,
            temperature: 0.0,
            max_tokens: 1024,
            timeout_ms: openai::DEFAULT_TIMEOUT_MS,
            system_prompt: None,
            mock_script: Vec::new(),
        }
    }
}

impl ProviderConfig {
    /// Build the configured provider.
    ///
    /// For [`ProviderKind::Mock`] the script is used as-is; an empty script means
    /// the built-in dry-run sequence (`ping`/`list_windows`/`finish`). For
    /// [`ProviderKind::OpenAi`] the API key is resolved from `api_key` and then
    /// from the environment, and a missing key is [`ProviderError::MissingApiKey`].
    pub fn build(&self) -> Result<Box<dyn LlmProvider>, ProviderError> {
        todo!("phase 2: construct MockProvider or OpenAiCompatProvider")
    }
}
