//! Crate-local error type and its mapping to AGP error codes.
//!
//! Every variant maps to exactly one AGP error code (`docs/protocol.md` §6)
//! through [`ViewerProtoError::error_code`], so the runtime can answer a
//! malformed viewer message without inventing codes (`docs/viewer.md` §6).

use adesk_core::ErrorCode;
use thiserror::Error;

/// Errors produced while encoding or decoding VAP messages (`docs/viewer.md` §1).
#[derive(Debug, Error)]
pub enum ViewerProtoError {
    /// The payload is not a well-formed VAP message: invalid JSON, not a JSON
    /// object, or a missing/non-string `"type"` discriminator (§1).
    #[error("malformed message")]
    Malformed,
    /// A message type could not be recognised where one was required.
    ///
    /// Wire decoding never produces this — an unrecognised `"type"` becomes the
    /// `Unknown` *message* variant so a peer stays forward-compatible (§1) — it
    /// exists for callers that must reject rather than forward an unknown type.
    #[error("unknown message type: {message_type}")]
    Unknown {
        /// The unrecognised `"type"` value.
        message_type: String,
    },
    /// The payload carried well-formed JSON that does not match the message
    /// schema (e.g. a known `"type"` with an invalid field).
    #[error("invalid message parameters: {message}")]
    InvalidParams {
        /// Description of the schema mismatch.
        message: String,
    },
    /// The peer speaks a different protocol version (§2, §7).
    #[error("protocol version mismatch: this build speaks v{server}, peer sent v{client}")]
    VersionMismatch {
        /// Version reported by the peer.
        client: u32,
        /// Version this build implements ([`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION)).
        server: u32,
    },
    /// The payload is not valid JSON.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl ViewerProtoError {
    /// AGP error code (`docs/protocol.md` §6) this failure maps to.
    ///
    /// A version mismatch is [`ErrorCode::ProtocolVersionMismatch`]; every other
    /// failure is [`ErrorCode::InvalidRequest`].
    pub fn error_code(&self) -> ErrorCode {
        match self {
            ViewerProtoError::VersionMismatch { .. } => ErrorCode::ProtocolVersionMismatch,
            ViewerProtoError::Malformed
            | ViewerProtoError::Unknown { .. }
            | ViewerProtoError::InvalidParams { .. }
            | ViewerProtoError::Json(_) => ErrorCode::InvalidRequest,
        }
    }
}

/// Crate-local result type; the workspace convention is one per `thiserror` enum.
pub type Result<T> = std::result::Result<T, ViewerProtoError>;
