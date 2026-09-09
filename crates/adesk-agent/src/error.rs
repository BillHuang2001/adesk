//! Crate error type, provider error type, and the loop's error classification.
//!
//! Every workspace crate exposes a `thiserror` enum plus `pub type Result<T>`;
//! this is `adesk-agent`'s. Runtime errors reuse [`adesk_core::Error`] so the AGP
//! `ErrorCode` values survive unchanged and can be matched on for recovery.

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
    /// An action needed a window but none is active (recoverable).
    #[error("no active window")]
    NoActiveWindow,
    /// The provider returned a decision that cannot be executed.
    #[error("invalid decision: {0}")]
    InvalidDecision(String),
    /// Invalid CLI or environment configuration.
    #[error("configuration error: {0}")]
    Config(String),
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
    /// The provider/model cannot serve the request.
    #[error("provider does not support: {0}")]
    Unsupported(String),
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
    /// `Client(UnknownWindow | UnknownApp | CaptureFailed | RenderFailed)` and
    /// [`Error::NoActiveWindow`] are [`ErrorClass::Recoverable`]; everything else
    /// (protocol mismatch, configuration, invalid decision, shutting down,
    /// internal errors) is [`ErrorClass::Fatal`].
    pub fn class(&self) -> ErrorClass {
        todo!("phase 2: map Error -> ErrorClass per the documented table")
    }

    /// Stable key used for `MetricsReport::failures_by_kind`, e.g.
    /// `"unknown_window"`, `"transport"`, `"provider_timeout"`.
    pub fn kind_key(&self) -> String {
        todo!("phase 2: stable per-error key for metrics")
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
