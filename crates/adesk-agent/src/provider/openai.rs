//! OpenAI-compatible `/chat/completions` provider.
//!
//! Works against any endpoint that speaks the OpenAI chat-completions shape:
//! the request carries a system prompt, one user message with the serialized
//! [`AgentContext`], and images as base64 data URLs; the response content must
//! parse into an [`AgentDecision`]. `base_url`, `model` and the API key come from
//! the CLI or the environment (`ADESK_AGENT_BASE_URL`, `ADESK_AGENT_MODEL`,
//! `ADESK_AGENT_API_KEY` / `OPENAI_API_KEY`).

use std::time::Duration;

use adesk_proto::{ImageFormat, ImagePayload};
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
    http: reqwest::Client,
    config: OpenAiConfig,
}

/// Wire name of an [`ImageDetail`] hint.
fn detail_name(detail: ImageDetail) -> &'static str {
    match detail {
        ImageDetail::Auto => "auto",
        ImageDetail::Low => "low",
        ImageDetail::High => "high",
    }
}

/// One `image_url` content part for a PNG payload.
///
/// Returns `None` for non-PNG payloads: `rgba8` is never sent, because the
/// runtime always encodes PNG for providers and endpoints cannot decode raw
/// pixels.
fn image_url_part(image: &ImagePayload, detail: ImageDetail) -> Option<serde_json::Value> {
    if image.format != ImageFormat::Png {
        tracing::warn!(
            format = ?image.format,
            width = image.width,
            height = image.height,
            "skipping non-PNG image in the provider request"
        );
        return None;
    }
    Some(serde_json::json!({
        "type": "image_url",
        "image_url": {
            "url": format!("data:image/png;base64,{}", image.data),
            "detail": detail_name(detail),
        },
    }))
}

/// Serialize the context as compact JSON with both image slots removed.
///
/// The images travel as separate `image_url` parts; keeping them in the text
/// would duplicate their base64 payloads and waste tokens.
fn context_text(ctx: &AgentContext) -> String {
    let mut value = match serde_json::to_value(ctx) {
        Ok(value) => value,
        Err(err) => {
            tracing::error!(error = %err, "failed to serialize the agent context");
            return serde_json::json!({ "context_error": err.to_string() }).to_string();
        }
    };
    if let Some(object) = value.as_object_mut() {
        object.remove("image");
        object.remove("keyframe");
    }
    value.to_string()
}

/// Truncate `text` to `max_chars` characters on a `char` boundary.
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

/// Map a reqwest failure onto a provider error.
fn request_error(err: reqwest::Error, timeout_ms: u64) -> ProviderError {
    if err.is_timeout() {
        ProviderError::Timeout(timeout_ms)
    } else {
        ProviderError::Transport(err.to_string())
    }
}

/// Strip a leading/trailing markdown code fence from model output.
fn strip_code_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // Drop the language tag on the fence line.
    let body = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => rest,
    };
    match body.rfind("```") {
        Some(index) => body[..index].trim(),
        None => body.trim(),
    }
}

/// Outermost `{...}` span of `text`, for models that wrap JSON in prose.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// Text of `choices[0].message.content`, accepting the string and the
/// content-parts array shapes.
fn extract_content(response: &serde_json::Value) -> Option<String> {
    let content = response
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?;
    match content {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if let Some(part) = part.get("text").and_then(serde_json::Value::as_str) {
                    text.push_str(part);
                }
            }
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
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
    ///
    /// The user message carries the previous keyframe first and the current image
    /// last, so the most recent frame is the most recent image part. With no
    /// images the content is a plain string (maximum endpoint compatibility);
    /// with images it is the content-parts array.
    pub fn build_chat_request(ctx: &AgentContext, config: &OpenAiConfig) -> serde_json::Value {
        let text = context_text(ctx);
        let images: Vec<serde_json::Value> = [ctx.keyframe.as_ref(), ctx.image.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|image| image_url_part(image, config.image_detail))
            .collect();

        let content = if images.is_empty() {
            serde_json::Value::String(text)
        } else {
            let mut parts = Vec::with_capacity(images.len() + 1);
            parts.push(serde_json::json!({ "type": "text", "text": text }));
            parts.extend(images);
            serde_json::Value::Array(parts)
        };

        serde_json::json!({
            "model": config.model,
            "temperature": config.temperature,
            "max_tokens": config.max_tokens,
            "messages": [
                { "role": "system", "content": config.system_prompt },
                { "role": "user", "content": content },
            ],
        })
    }

    /// Parse a model response into an [`AgentDecision`].
    ///
    /// Accepts raw JSON content or content wrapped in a ```json fence; anything
    /// else is [`ProviderError::InvalidResponse`]. Pure and unit-testable.
    pub fn parse_decision(content: &str) -> Result<AgentDecision, ProviderError> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(ProviderError::InvalidResponse(String::from(
                "empty response content",
            )));
        }

        let unfenced = strip_code_fence(trimmed);
        match serde_json::from_str::<AgentDecision>(unfenced) {
            Ok(decision) => Ok(decision),
            Err(unfenced_error) => match extract_json_object(unfenced) {
                Some(object) => serde_json::from_str::<AgentDecision>(object).map_err(|err| {
                    ProviderError::InvalidResponse(format!(
                        "{err} (content: {})",
                        truncate(trimmed, 200)
                    ))
                }),
                None => Err(ProviderError::InvalidResponse(format!(
                    "{unfenced_error} (content: {})",
                    truncate(trimmed, 200)
                ))),
            },
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = Self::build_chat_request(ctx, &self.config);

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| request_error(err, self.config.timeout_ms))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| request_error(err, self.config.timeout_ms))?;
        tracing::debug!(
            model = %self.config.model,
            status = status.as_u16(),
            bytes = text.len(),
            "chat completion response"
        );

        if !status.is_success() {
            return Err(ProviderError::Status {
                status: status.as_u16(),
                body: truncate(&text, 512),
            });
        }

        let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
            ProviderError::InvalidResponse(format!(
                "response is not JSON: {err} (body: {})",
                truncate(&text, 200)
            ))
        })?;
        let content = extract_content(&value).ok_or_else(|| {
            ProviderError::InvalidResponse(format!(
                "response has no choices[0].message.content (body: {})",
                truncate(&text, 200)
            ))
        })?;

        Self::parse_decision(&content)
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal bounded context (no socket, no runtime).
    fn context(task: &str) -> AgentContext {
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

    fn png(data: &str) -> ImagePayload {
        ImagePayload {
            width: 4,
            height: 2,
            format: ImageFormat::Png,
            stride: None,
            data: data.to_owned(),
            scale: 1.0,
        }
    }

    fn config() -> OpenAiConfig {
        OpenAiConfig {
            model: String::from("test-model"),
            api_key: String::from("sk-test"),
            temperature: 0.25,
            max_tokens: 64,
            system_prompt: String::from("SYS"),
            ..OpenAiConfig::default()
        }
    }

    #[test]
    fn request_without_images_uses_a_plain_string_content() {
        let ctx = context("open the settings dialog");
        let body = OpenAiCompatProvider::build_chat_request(&ctx, &config());

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["temperature"], 0.25);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "SYS");
        assert_eq!(body["messages"][1]["role"], "user");

        let content = body["messages"][1]["content"]
            .as_str()
            .expect("text-only requests send a plain string");
        assert!(content.contains("\"task\":\"open the settings dialog\""));
        assert!(!content.contains("\"image\""));
    }

    #[test]
    fn images_are_attached_as_png_data_urls_keyframe_first() {
        let mut ctx = context("look at the window");
        ctx.keyframe = Some(png("S0VZRlJBTUU="));
        ctx.image = Some(png("Q1VSUkVOVA=="));
        let config = OpenAiConfig {
            image_detail: ImageDetail::High,
            ..config()
        };

        let body = OpenAiCompatProvider::build_chat_request(&ctx, &config);
        let parts = body["messages"][1]["content"]
            .as_array()
            .expect("image requests send content parts");
        assert_eq!(parts.len(), 3, "one text part plus two images");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(
            parts[1]["image_url"]["url"],
            "data:image/png;base64,S0VZRlJBTUU="
        );
        assert_eq!(
            parts[2]["image_url"]["url"],
            "data:image/png;base64,Q1VSUkVOVA=="
        );
        assert_eq!(parts[1]["image_url"]["detail"], "high");

        // The base64 payloads must not be duplicated inside the text prompt.
        let text = parts[0]["text"].as_str().expect("text part");
        assert!(!text.contains("S0VZRlJBTUU="));
        assert!(!text.contains("Q1VSUkVOVA=="));
    }

    #[test]
    fn image_detail_defaults_to_auto() {
        let mut ctx = context("look");
        ctx.image = Some(png("QUJD"));
        let body = OpenAiCompatProvider::build_chat_request(&ctx, &config());
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["detail"],
            "auto"
        );
    }

    #[test]
    fn rgba8_payloads_are_never_sent() {
        let mut ctx = context("look");
        ctx.image = Some(ImagePayload {
            width: 2,
            height: 2,
            format: ImageFormat::Rgba8,
            stride: Some(8),
            data: String::from("QUJDRA=="),
            scale: 1.0,
        });

        let body = OpenAiCompatProvider::build_chat_request(&ctx, &config());
        assert!(
            body["messages"][1]["content"].is_string(),
            "a non-PNG payload leaves a text-only request"
        );
        assert!(!body.to_string().contains("QUJDRA=="));
    }

    #[test]
    fn parse_decision_accepts_raw_and_fenced_json() {
        assert_eq!(
            OpenAiCompatProvider::parse_decision(r#"{"op":"list_windows"}"#).unwrap(),
            AgentDecision::ListWindows
        );

        let fenced = "```json\n{\"op\":\"finish\",\"success\":true,\"summary\":\"done\"}\n```";
        assert_eq!(
            OpenAiCompatProvider::parse_decision(fenced).unwrap(),
            AgentDecision::Finish {
                success: true,
                summary: String::from("done"),
            }
        );

        let unclosed = "```json\n{\"op\":\"list_apps\",\"query\":\"term\"}";
        assert_eq!(
            OpenAiCompatProvider::parse_decision(unclosed).unwrap(),
            AgentDecision::ListApps {
                query: Some(String::from("term")),
            }
        );

        let with_prose =
            "Here is the next action:\n{\"op\":\"wait\",\"until\":{\"type\":\"change\"}}\n";
        assert!(matches!(
            OpenAiCompatProvider::parse_decision(with_prose).unwrap(),
            AgentDecision::Wait { .. }
        ));
    }

    #[test]
    fn parse_decision_rejects_non_decisions() {
        for bad in [
            "",
            "   ",
            "not json",
            "{\"op\":\"teleport\"}",
            "```json\nnot json\n```",
            "{}",
        ] {
            let err = OpenAiCompatProvider::parse_decision(bad).unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidResponse(_)),
                "{bad:?} produced {err}"
            );
        }
    }

    #[test]
    fn extract_content_handles_string_and_part_arrays() {
        let string = serde_json::json!({"choices": [{"message": {"content": "hi"}}]});
        assert_eq!(extract_content(&string).as_deref(), Some("hi"));

        let parts = serde_json::json!({
            "choices": [{"message": {"content": [
                {"type": "text", "text": "a"},
                {"type": "text", "text": "b"},
            ]}}]
        });
        assert_eq!(extract_content(&parts).as_deref(), Some("ab"));

        let missing = serde_json::json!({"error": {"message": "nope"}});
        assert_eq!(extract_content(&missing), None);
    }

    #[test]
    fn new_requires_a_non_blank_api_key() {
        assert!(matches!(
            OpenAiCompatProvider::new(OpenAiConfig::default()).unwrap_err(),
            ProviderError::MissingApiKey
        ));
        let blank = OpenAiConfig {
            api_key: String::from("   "),
            ..OpenAiConfig::default()
        };
        assert!(matches!(
            OpenAiCompatProvider::new(blank).unwrap_err(),
            ProviderError::MissingApiKey
        ));

        let provider = OpenAiCompatProvider::new(config()).unwrap();
        assert_eq!(provider.name(), "openai");
        assert!(provider.supports_images());
        assert_eq!(provider.config().model, "test-model");
        assert_eq!(provider.config().timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn truncate_is_char_boundary_safe() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("日本語です", 2), "日本…");
        assert_eq!(truncate("", 0), "");
    }

    #[test]
    fn detail_names_match_the_api() {
        assert_eq!(detail_name(ImageDetail::Auto), "auto");
        assert_eq!(detail_name(ImageDetail::Low), "low");
        assert_eq!(detail_name(ImageDetail::High), "high");
    }
}
