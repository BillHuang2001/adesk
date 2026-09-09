//! OpenAI-compatible `/chat/completions` provider.
//!
//! Works against any endpoint that speaks the OpenAI chat-completions shape:
//! the request carries a system prompt, one user message with the serialized
//! [`AgentContext`], and images as base64 data URLs; the response content must
//! parse into an [`AgentDecision`]. `base_url`, `model` and the API key come from
//! the CLI or the environment (`ADESK_AGENT_BASE_URL`, `ADESK_AGENT_MODEL`,
//! `ADESK_AGENT_API_KEY` / `OPENAI_API_KEY`).

use std::time::Duration;

use async_trait::async_trait;

use crate::context::AgentContext;
use crate::decision::AgentDecision;
use crate::error::ProviderError;
use crate::provider::LlmProvider;

/// Default endpoint when `--base-url`/`ADESK_AGENT_BASE_URL` is absent.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default model when `--model`/`ADESK_AGENT_MODEL` is absent.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
/// Default per-request timeout in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
/// Environment variable holding the API key (checked before `OPENAI_API_KEY`).
pub const API_KEY_ENV: &str = "ADESK_AGENT_API_KEY";
/// Fallback environment variable holding the API key.
pub const API_KEY_ENV_FALLBACK: &str = "OPENAI_API_KEY";

/// Default system prompt: the decision schema the model must emit.
///
/// This is the prompt-level contract for [`AgentDecision`]; keep it in sync when
/// the decision vocabulary changes.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You control a headless Wayland desktop through ADesk. Reply with exactly one JSON object \
(no prose, no code fences) describing the next single action.\n\
Runtime-native ops (never synthesized input): \
{\"op\":\"list_apps\",\"query\":null} | {\"op\":\"list_windows\"} | {\"op\":\"get_window\",\"window_id\":N} | \
{\"op\":\"launch_app\",\"app_id\":\"...\",\"args\":[]} | {\"op\":\"activate_window\",\"window_id\":N} | \
{\"op\":\"close_window\",\"window_id\":N} | {\"op\":\"capture\",\"window_id\":N,\"region\":null,\"max_dimension\":null} | \
{\"op\":\"observe\",\"window_id\":N|null,\"until\":{\"type\":\"quiet\",\"quiet_ms\":250},\"include_image\":true} | \
{\"op\":\"wait\",\"window_id\":N|null,\"until\":{\"type\":\"change\"}}.\n\
Application input via the seat: \
{\"op\":\"click\",\"window_id\":N,\"position\":{\"type\":\"normalized\",\"x\":0.5,\"y\":0.5},\"button\":\"left\",\"count\":1} | \
{\"op\":\"type\",\"text\":\"...\"} | {\"op\":\"keypress\",\"keys\":[\"CTRL\",\"L\"]} | \
{\"op\":\"scroll\",\"window_id\":N,\"position\":{\"type\":\"normalized\",\"x\":0.5,\"y\":0.5},\"dx\":0.0,\"dy\":-3.0}.\n\
Finish with {\"op\":\"finish\",\"success\":true|false,\"summary\":\"...\"}. \
Coordinates are window-relative; prefer normalized positions. After an input action, observe \
quiet before assuming the UI settled — quiet is evidence, not proof. Do not repeat an action that \
failed; re-list windows/apps to refresh stale ids.";

/// How much image detail to request from the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageDetail {
    /// Let the provider choose.
    #[default]
    Auto,
    /// Cheap 512px-budget processing.
    Low,
    /// Tiled high-detail processing (more visual tokens).
    High,
}

/// Fully resolved configuration for [`OpenAiCompatProvider`].
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Endpoint base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Model name, e.g. `gpt-4o-mini`.
    pub model: String,
    /// API key (required; resolved from CLI/env before construction).
    pub api_key: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Response token cap.
    pub max_tokens: u32,
    /// Per-request HTTP timeout.
    pub timeout_ms: u64,
    /// Image detail hint.
    pub image_detail: ImageDetail,
    /// System prompt override; defaults to [`DEFAULT_SYSTEM_PROMPT`].
    pub system_prompt: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: String::from(DEFAULT_BASE_URL),
            model: String::from(DEFAULT_MODEL),
            api_key: String::new(),
            temperature: 0.0,
            max_tokens: 1024,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            image_detail: ImageDetail::Auto,
            system_prompt: String::from(DEFAULT_SYSTEM_PROMPT),
        }
    }
}

/// Provider backed by an OpenAI-compatible chat-completions endpoint.
#[derive(Debug)]
pub struct OpenAiCompatProvider {
    /// HTTP client (shared, connection-pooled).
    #[allow(dead_code)] // read by `complete` in phase 2
    http: reqwest::Client,
    config: OpenAiConfig,
}

impl OpenAiCompatProvider {
    /// Validate the configuration and build the HTTP client.
    ///
    /// Returns [`ProviderError::MissingApiKey`] when `api_key` is empty.
    pub fn new(config: OpenAiConfig) -> Result<Self, ProviderError> {
        if config.api_key.trim().is_empty() {
            return Err(ProviderError::MissingApiKey);
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(Self { http, config })
    }

    /// The resolved configuration.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// Build the JSON request body for one decision.
    ///
    /// Pure and unit-testable: embeds `ctx.image`/`ctx.keyframe` as
    /// `data:image/png;base64,...` (or `image/rgba8` is never sent — the runtime
    /// always returns PNG to providers), and serializes the context as the user
    /// message. No network access.
    pub fn build_chat_request(
        ctx: &AgentContext,
        config: &OpenAiConfig,
    ) -> serde_json::Value {
        let _ = (ctx, config);
        todo!("phase 2: build messages[] with text + image data URLs")
    }

    /// Parse a model response into an [`AgentDecision`].
    ///
    /// Accepts raw JSON content or content wrapped in a ```json fence; anything
    /// else is [`ProviderError::InvalidResponse`]. Pure and unit-testable.
    pub fn parse_decision(content: &str) -> Result<AgentDecision, ProviderError> {
        let _ = content;
        todo!("phase 2: strip fences, serde_json::from_str::<AgentDecision>")
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn complete(&self, _ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        todo!("phase 2: POST {base_url}/chat/completions, parse content -> decision")
    }

    fn name(&self) -> &str {
        "openai"
    }
}
