//! Provider-agnostic LLM interface and implementations.
//!
//! The agent depends only on [`LlmProvider`]: given a bounded [`AgentContext`],
//! return one [`AgentDecision`]. [`MockProvider`] replays a script (default in
//! tests and for `--provider mock` dry runs); [`DummyVlmProvider`] replays a canned
//! sequence or emits reproducible random decisions with no I/O; and
//! [`OpenAiCompatProvider`] speaks the OpenAI `/chat/completions` shape against any
//! compatible `base_url`.

pub mod dummy;
pub mod mock;
pub mod openai;

pub use dummy::{DummyConfig, DummyMode, DummyVlmProvider};
pub use mock::{MockProvider, ScriptEntry};
pub use openai::{OpenAiCompatProvider, OpenAiConfig};

use std::sync::Arc;

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

#[async_trait]
impl LlmProvider for Box<dyn LlmProvider> {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        (**self).complete(ctx).await
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn supports_images(&self) -> bool {
        (**self).supports_images()
    }
}

#[async_trait]
impl<T: LlmProvider + ?Sized> LlmProvider for Arc<T> {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        self.as_ref().complete(ctx).await
    }

    fn name(&self) -> &str {
        self.as_ref().name()
    }

    fn supports_images(&self) -> bool {
        self.as_ref().supports_images()
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
    /// Synthetic "dummy VLM" ([`DummyVlmProvider`]): canned or reproducible random
    /// decisions with no network or I/O.
    #[value(name = "dummy")]
    Dummy,
}

impl ProviderKind {
    /// Stable name, matching the `--provider` CLI values.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::OpenAi => "openai",
            Self::Dummy => "dummy",
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
    /// Mode for [`DummyVlmProvider`].
    pub dummy_mode: DummyMode,
    /// Canned sequence for [`DummyVlmProvider`] in [`DummyMode::Fixed`].
    pub dummy_script: Vec<AgentDecision>,
    /// PRNG seed for [`DummyVlmProvider`] in [`DummyMode::Random`].
    pub dummy_seed: u64,
    /// Per-step finish probability for [`DummyVlmProvider`] in [`DummyMode::Random`].
    pub dummy_finish_probability: f64,
    /// Hard step budget for [`DummyVlmProvider`] in [`DummyMode::Random`].
    pub dummy_step_budget: u32,
    /// Action pool for [`DummyVlmProvider`] in [`DummyMode::Random`] (empty =
    /// [`dummy::default_action_pool`]).
    pub dummy_pool: Vec<AgentDecision>,
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
            dummy_mode: DummyMode::default(),
            dummy_script: Vec::new(),
            dummy_seed: dummy::DEFAULT_SEED,
            dummy_finish_probability: dummy::DEFAULT_FINISH_PROBABILITY,
            dummy_step_budget: dummy::DEFAULT_STEP_BUDGET,
            dummy_pool: Vec::new(),
        }
    }
}

impl ProviderConfig {
    /// Build the configured provider.
    ///
    /// For [`ProviderKind::Mock`] the script is used as-is; an empty script means
    /// the built-in dry-run sequence (`ping`/`list_windows`/`finish`). For
    /// [`ProviderKind::Dummy`] the `dummy_*` fields configure a no-I/O synthetic
    /// provider that always builds. For [`ProviderKind::OpenAi`] the API key is
    /// resolved from `api_key` and then from the environment, and a missing key is
    /// [`ProviderError::MissingApiKey`].
    pub fn build(&self) -> Result<Box<dyn LlmProvider>, ProviderError> {
        match self.kind {
            ProviderKind::Mock => Ok(Box::new(MockProvider::scripted(self.mock_script.clone()))),
            ProviderKind::Dummy => Ok(Box::new(DummyVlmProvider::from_config(DummyConfig {
                mode: self.dummy_mode,
                script: self.dummy_script.clone(),
                seed: self.dummy_seed,
                finish_probability: self.dummy_finish_probability,
                step_budget: self.dummy_step_budget,
                pool: self.dummy_pool.clone(),
                name: String::from("dummy"),
            }))),
            ProviderKind::OpenAi => Ok(Box::new(OpenAiCompatProvider::new(self.openai_config()?)?)),
        }
    }

    /// Resolve the OpenAI-compatible configuration.
    ///
    /// Blank CLI/env values count as absent, so the defaults apply. The API key is
    /// taken from `api_key` first, then from [`openai::API_KEY_ENV`] and finally
    /// [`openai::API_KEY_ENV_FALLBACK`]; no other value ever holds a secret.
    fn openai_config(&self) -> Result<OpenAiConfig, ProviderError> {
        let api_key = non_empty(self.api_key.as_deref())
            .map(str::to_owned)
            .or_else(|| non_empty_env(openai::API_KEY_ENV))
            .or_else(|| non_empty_env(openai::API_KEY_ENV_FALLBACK))
            .ok_or(ProviderError::MissingApiKey)?;

        Ok(OpenAiConfig {
            base_url: non_empty(self.base_url.as_deref())
                .unwrap_or(openai::DEFAULT_BASE_URL)
                .to_owned(),
            model: non_empty(self.model.as_deref())
                .unwrap_or(openai::DEFAULT_MODEL)
                .to_owned(),
            api_key,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            timeout_ms: self.timeout_ms,
            system_prompt: non_empty(self.system_prompt.as_deref())
                .unwrap_or(openai::DEFAULT_SYSTEM_PROMPT)
                .to_owned(),
        })
    }
}

/// `Some(trimmed)` for a non-blank value, `None` otherwise.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Non-blank environment variable, trimmed.
fn non_empty_env(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Minimal bounded context shared by the provider tests (no socket, no runtime).
#[cfg(test)]
pub(crate) fn test_context(task: &str) -> AgentContext {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::AgentDecision;

    #[test]
    fn provider_kind_names_match_the_cli() {
        assert_eq!(ProviderKind::Mock.as_str(), "mock");
        assert_eq!(ProviderKind::OpenAi.as_str(), "openai");
        assert_eq!(ProviderKind::Dummy.as_str(), "dummy");
    }

    #[tokio::test]
    async fn default_config_builds_the_mock_provider() {
        let provider = ProviderConfig::default().build().expect("mock builds");
        assert_eq!(provider.name(), "mock");
        assert!(provider.supports_images());

        // The empty script is the built-in dry-run sequence.
        let decision = provider.complete(&test_context("smoke")).await.unwrap();
        assert_eq!(decision, AgentDecision::ListWindows);
        let decision = provider.complete(&test_context("smoke")).await.unwrap();
        assert!(matches!(
            decision,
            AgentDecision::Finish { success: true, .. }
        ));
    }

    #[tokio::test]
    async fn mock_script_is_handed_to_the_mock_provider() {
        let config = ProviderConfig {
            mock_script: vec![AgentDecision::ListApps {
                query: Some(String::from("term")),
            }],
            ..ProviderConfig::default()
        };
        let provider = config.build().expect("mock builds");
        assert_eq!(
            provider.complete(&test_context("smoke")).await.unwrap(),
            AgentDecision::ListApps {
                query: Some(String::from("term")),
            }
        );
    }

    #[test]
    fn openai_config_resolves_cli_values_over_defaults() {
        let config = ProviderConfig {
            kind: ProviderKind::OpenAi,
            api_key: Some(String::from("sk-test")),
            ..ProviderConfig::default()
        }
        .openai_config()
        .expect("explicit key");

        assert_eq!(config.base_url, openai::DEFAULT_BASE_URL);
        assert_eq!(config.model, openai::DEFAULT_MODEL);
        assert_eq!(config.system_prompt, openai::DEFAULT_SYSTEM_PROMPT);
        assert_eq!(config.timeout_ms, openai::DEFAULT_TIMEOUT_MS);

        let explicit = ProviderConfig {
            kind: ProviderKind::OpenAi,
            model: Some(String::from("  local-model  ")),
            base_url: Some(String::from("http://127.0.0.1:8080/v1")),
            api_key: Some(String::from(" sk-test ")),
            temperature: 0.5,
            max_tokens: 128,
            timeout_ms: 1234,
            system_prompt: Some(String::from("SYS")),
            mock_script: Vec::new(),
            dummy_mode: DummyMode::Fixed,
            dummy_script: Vec::new(),
            dummy_seed: 7,
            dummy_finish_probability: 0.5,
            dummy_step_budget: 3,
            dummy_pool: Vec::new(),
        }
        .openai_config()
        .expect("explicit key");

        assert_eq!(explicit.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(explicit.model, "local-model");
        assert_eq!(explicit.api_key, "sk-test");
        assert_eq!(explicit.temperature, 0.5);
        assert_eq!(explicit.max_tokens, 128);
        assert_eq!(explicit.timeout_ms, 1234);
        assert_eq!(explicit.system_prompt, "SYS");
    }

    #[test]
    fn openai_config_treats_blank_values_as_absent() {
        let config = ProviderConfig {
            kind: ProviderKind::OpenAi,
            model: Some(String::from("   ")),
            base_url: Some(String::from("")),
            api_key: Some(String::from("sk-test")),
            system_prompt: Some(String::from("\t")),
            ..ProviderConfig::default()
        }
        .openai_config()
        .expect("explicit key");

        assert_eq!(config.base_url, openai::DEFAULT_BASE_URL);
        assert_eq!(config.model, openai::DEFAULT_MODEL);
        assert_eq!(config.system_prompt, openai::DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn build_returns_an_openai_provider_with_an_explicit_key() {
        let config = ProviderConfig {
            kind: ProviderKind::OpenAi,
            api_key: Some(String::from("sk-test")),
            ..ProviderConfig::default()
        };
        let provider = config.build().expect("explicit key");
        assert_eq!(provider.name(), "openai");
        assert!(provider.supports_images());
    }

    #[test]
    fn default_config_carries_the_documented_dummy_defaults() {
        let config = ProviderConfig::default();
        assert_eq!(config.dummy_mode, DummyMode::Random);
        assert!(config.dummy_script.is_empty());
        assert_eq!(config.dummy_seed, dummy::DEFAULT_SEED);
        assert_eq!(
            config.dummy_finish_probability,
            dummy::DEFAULT_FINISH_PROBABILITY
        );
        assert_eq!(config.dummy_step_budget, dummy::DEFAULT_STEP_BUDGET);
        assert!(config.dummy_pool.is_empty());
    }

    #[tokio::test]
    async fn dummy_config_builds_a_fixed_dummy_provider() {
        let config = ProviderConfig {
            kind: ProviderKind::Dummy,
            dummy_mode: DummyMode::Fixed,
            dummy_script: vec![AgentDecision::ListWindows],
            dummy_pool: Vec::new(),
            ..ProviderConfig::default()
        };
        let provider = config.build().expect("dummy never fails");
        assert_eq!(provider.name(), "dummy");
        assert!(provider.supports_images());
        assert_eq!(
            provider.complete(&test_context("smoke")).await.unwrap(),
            AgentDecision::ListWindows
        );
        assert!(matches!(
            provider.complete(&test_context("smoke")).await.unwrap(),
            AgentDecision::Finish { success: true, .. }
        ));
    }

    #[tokio::test]
    async fn dummy_config_builds_a_reproducible_random_provider() {
        let config = ProviderConfig {
            kind: ProviderKind::Dummy,
            dummy_mode: DummyMode::Random,
            dummy_seed: 1234,
            ..ProviderConfig::default()
        };
        let first = config.build().expect("dummy never fails");
        let second = config.build().expect("dummy never fails");

        let mut first_seq = Vec::new();
        let mut second_seq = Vec::new();
        for _ in 0..10 {
            first_seq.push(first.complete(&test_context("smoke")).await.unwrap());
            second_seq.push(second.complete(&test_context("smoke")).await.unwrap());
        }
        assert_eq!(first_seq, second_seq);
    }

    #[tokio::test]
    async fn boxed_and_arc_providers_delegate_to_the_inner_provider() {
        let boxed: Box<dyn LlmProvider> =
            Box::new(MockProvider::scripted(vec![AgentDecision::ListWindows]));
        assert_eq!(boxed.name(), "mock");
        assert!(boxed.supports_images());
        assert_eq!(
            boxed.complete(&test_context("wrapped")).await.unwrap(),
            AgentDecision::ListWindows
        );

        // `MockProvider` is not `Clone`, so the second wrapper gets its own script.
        let arc: Arc<MockProvider> =
            Arc::new(MockProvider::scripted(vec![AgentDecision::ListApps {
                query: None,
            }]));
        assert_eq!(arc.name(), "mock");
        assert!(arc.supports_images());
        assert_eq!(
            arc.complete(&test_context("wrapped")).await.unwrap(),
            AgentDecision::ListApps { query: None }
        );
    }
}
