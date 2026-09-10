//! Crate error type, provider error type, and the loop's error classification.
//!
//! Every workspace crate exposes a `thiserror` enum plus `pub type Result<T>`;
//! this is `adesk-agent`'s. Runtime errors reuse [`adesk_core::Error`] so the AGP
//! `ErrorCode` values survive unchanged and can be matched on for recovery.

use adesk_core::ErrorCode;
use serde::{Deserialize, Serialize};

/// Errors produced by `adesk-agent`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An AGP request failed; the runtime's `ErrorCode` is preserved.
    #[error(transparent)]
    Client(#[from] adesk_core::Error),
    /// The LLM provider failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Transport-level failure (socket connect/read/write).
    #[error("transport error: {0}")]
    Transport(String),
    /// The runtime speaks a different AGP version than this crate implements.
    #[error("AGP protocol version mismatch: agent expects {expected}, runtime speaks {got}")]
    ProtocolVersion {
        /// Protocol version this crate implements ([`crate::client::PROTOCOL_VERSION`]).
        expected: u32,
        /// Protocol version reported by the runtime's `ping`.
        got: u32,
    },
    /// The loop exhausted its step budget without finishing.
    #[error("step budget exhausted after {0} steps")]
    StepBudgetExhausted(u32),
    /// The loop hit its consecutive-failure budget.
    #[error("failure budget exhausted after {0} consecutive failures")]
    FailureBudgetExhausted(u32),
    /// A single step exceeded its deadline.
    #[error("step {step} timed out after {timeout_ms} ms")]
    StepTimeout {
        /// Loop step that timed out.
        step: u32,
        /// Configured deadline in milliseconds.
        timeout_ms: u64,
    },
    /// The provider returned a decision that cannot be executed.
    #[error("invalid decision: {0}")]
    InvalidDecision(String),
    /// Local I/O failure (report writing, script files).
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Provider-side failures, kept separate from runtime errors.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum ProviderError {
    /// No API key configured for a provider that requires one.
    #[error("missing API key: pass --api-key or set ADESK_AGENT_API_KEY")]
    MissingApiKey,
    /// The provider request could not be sent (connect/TLS/read failure).
    #[error("provider transport error: {0}")]
    Transport(String),
    /// The provider answered with a non-success HTTP status.
    #[error("provider returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated by the provider implementation).
        body: String,
    },
    /// The provider did not answer within the configured timeout.
    #[error("provider timed out after {0} ms")]
    Timeout(u64),
    /// The provider response could not be parsed into an `AgentDecision`.
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
}

/// How the loop reacts to an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Transient (transport, timeout, busy): retry the same step.
    Retryable,
    /// Recoverable (unknown window/app, capture failure): record the failure,
    /// refresh the context and let the agent decide again.
    Recoverable,
    /// Fatal (protocol mismatch, configuration, shutting down): stop the run.
    Fatal,
}

impl Error {
    /// Classify this error for the loop's recovery policy.
    ///
    /// Mapping: `Client(Timeout | Busy)`, [`Error::Transport`], [`Error::Io`],
    /// and provider `Transport | Timeout` are [`ErrorClass::Retryable`];
    /// `Client(UnknownWindow | UnknownApp | CaptureFailed | RenderFailed)` is
    /// [`ErrorClass::Recoverable`]; everything else (protocol mismatch, invalid
    /// decision, shutting down, internal errors) is [`ErrorClass::Fatal`].
    pub fn class(&self) -> ErrorClass {
        match self {
            Error::Client(err) => match err.code {
                // Transient runtime states: retry the same step.
                ErrorCode::Timeout | ErrorCode::Busy => ErrorClass::Retryable,
                // Stale facts or a failed readback: refresh and let the agent
                // decide again.
                ErrorCode::UnknownWindow
                | ErrorCode::UnknownApp
                | ErrorCode::CaptureFailed
                | ErrorCode::RenderFailed => ErrorClass::Recoverable,
                _ => ErrorClass::Fatal,
            },
            // Transport hiccups and local I/O failures are worth another try.
            Error::Transport(_) | Error::Io(_) => ErrorClass::Retryable,
            Error::Provider(ProviderError::Transport(_) | ProviderError::Timeout(_)) => {
                ErrorClass::Retryable
            }
            Error::Provider(_) => ErrorClass::Fatal,
            // Protocol mismatch, invalid decisions, budget exhaustion, deadlines
            // and internal/JSON failures end the run.
            _ => ErrorClass::Fatal,
        }
    }

    /// Stable key used for `MetricsReport::failures_by_kind`, e.g.
    /// `"unknown_window"`, `"transport"`, `"provider_timeout"`.
    ///
    /// Runtime errors reuse the AGP `ErrorCode` name; agent-local and provider
    /// errors use their own snake_case key:
    ///
    /// | Error | Key |
    /// |---|---|
    /// | `Client(e)` | `e.code.as_str()` (`unknown_window`, `timeout`, ...) |
    /// | `Provider(Transport)` | `provider_transport` |
    /// | `Provider(Timeout)` | `provider_timeout` |
    /// | `Provider(MissingApiKey)` | `provider_missing_api_key` |
    /// | `Provider(Status{..})` | `provider_status` |
    /// | `Provider(InvalidResponse)` | `provider_invalid_response` |
    /// | `Transport` / `Io` / `Json` | `transport` / `io` / `json` |
    /// | `ProtocolVersion{..}` | `protocol_version_mismatch` |
    /// | `StepTimeout{..}` | `step_timeout` |
    /// | `StepBudgetExhausted` | `step_budget_exhausted` |
    /// | `FailureBudgetExhausted` | `failure_budget_exhausted` |
    /// | `InvalidDecision` | `invalid_decision` |
    pub fn kind_key(&self) -> String {
        match self {
            Error::Client(err) => err.code.as_str().to_string(),
            Error::Provider(err) => match err {
                ProviderError::MissingApiKey => "provider_missing_api_key".to_string(),
                ProviderError::Transport(_) => "provider_transport".to_string(),
                ProviderError::Status { .. } => "provider_status".to_string(),
                ProviderError::Timeout(_) => "provider_timeout".to_string(),
                ProviderError::InvalidResponse(_) => "provider_invalid_response".to_string(),
            },
            Error::Transport(_) => "transport".to_string(),
            Error::ProtocolVersion { .. } => "protocol_version_mismatch".to_string(),
            Error::StepBudgetExhausted(_) => "step_budget_exhausted".to_string(),
            Error::FailureBudgetExhausted(_) => "failure_budget_exhausted".to_string(),
            Error::StepTimeout { .. } => "step_timeout".to_string(),
            Error::InvalidDecision(_) => "invalid_decision".to_string(),
            Error::Io(_) => "io".to_string(),
            Error::Json(_) => "json".to_string(),
        }
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    fn client(code: ErrorCode) -> Error {
        Error::Client(adesk_core::Error::new(code, "boom"))
    }

    /// The documented classification table, case by case.
    #[test]
    fn class_follows_the_documented_table() {
        let retryable = [
            client(ErrorCode::Timeout),
            client(ErrorCode::Busy),
            Error::Transport("socket closed".into()),
            Error::Io(std::io::Error::other("disk")),
            Error::Provider(ProviderError::Transport("tls".into())),
            Error::Provider(ProviderError::Timeout(30_000)),
        ];
        for err in retryable {
            assert_eq!(err.class(), ErrorClass::Retryable, "{err}");
        }

        let recoverable = [
            client(ErrorCode::UnknownWindow),
            client(ErrorCode::UnknownApp),
            client(ErrorCode::CaptureFailed),
            client(ErrorCode::RenderFailed),
        ];
        for err in recoverable {
            assert_eq!(err.class(), ErrorClass::Recoverable, "{err}");
        }

        let fatal = [
            client(ErrorCode::InvalidRequest),
            client(ErrorCode::UnknownMethod),
            client(ErrorCode::LaunchFailed),
            client(ErrorCode::NotSupported),
            client(ErrorCode::Internal),
            client(ErrorCode::ShuttingDown),
            client(ErrorCode::ProtocolVersionMismatch),
            Error::Provider(ProviderError::MissingApiKey),
            Error::Provider(ProviderError::Status {
                status: 500,
                body: "oops".into(),
            }),
            Error::Provider(ProviderError::InvalidResponse("not json".into())),
            Error::ProtocolVersion {
                expected: 1,
                got: 999,
            },
            Error::StepBudgetExhausted(20),
            Error::FailureBudgetExhausted(3),
            Error::StepTimeout {
                step: 2,
                timeout_ms: 30_000,
            },
            Error::InvalidDecision("missing window_id".into()),
            Error::Json(serde_json::from_str::<u8>("nope").unwrap_err()),
        ];
        for err in fatal {
            assert_eq!(err.class(), ErrorClass::Fatal, "{err}");
        }
    }

    /// Runtime failures keep their AGP `ErrorCode` name; provider and agent
    /// errors get their own stable key.
    #[test]
    fn kind_key_is_stable_per_error() {
        assert_eq!(
            client(ErrorCode::UnknownWindow).kind_key(),
            "unknown_window"
        );
        assert_eq!(client(ErrorCode::Timeout).kind_key(), "timeout");
        assert_eq!(Error::Transport("x".into()).kind_key(), "transport");
        assert_eq!(
            Error::Provider(ProviderError::Timeout(1)).kind_key(),
            "provider_timeout"
        );
        assert_eq!(
            Error::Provider(ProviderError::MissingApiKey).kind_key(),
            "provider_missing_api_key"
        );
        assert_eq!(
            Error::ProtocolVersion {
                expected: 1,
                got: 2
            }
            .kind_key(),
            "protocol_version_mismatch"
        );
        // Every AGP code maps to its wire name, so metrics keys never drift.
        for code in [
            ErrorCode::InvalidRequest,
            ErrorCode::UnknownMethod,
            ErrorCode::UnknownWindow,
            ErrorCode::UnknownApp,
            ErrorCode::LaunchFailed,
            ErrorCode::CaptureFailed,
            ErrorCode::RenderFailed,
            ErrorCode::Timeout,
            ErrorCode::NotSupported,
            ErrorCode::Busy,
            ErrorCode::Internal,
            ErrorCode::ShuttingDown,
            ErrorCode::ProtocolVersionMismatch,
        ] {
            assert_eq!(client(code).kind_key(), code.as_str());
        }
    }
}
