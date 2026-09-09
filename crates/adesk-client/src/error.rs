//! The client error type and its mapping from AGP.
//!
//! One enum covers every failure mode of the SDK:
//!
//! | Cause | Variant |
//! |---|---|
//! | connect/read/write failure | [`ClientError::Io`] |
//! | malformed/oversized/unexpected frame | [`ClientError::Protocol`] |
//! | peer closed the socket | [`ClientError::Closed`] |
//! | AGP error frame (protocol §6) | [`ClientError::Server`] |
//! | `ping.protocol_version` mismatch | [`ClientError::VersionMismatch`] |
//! | response JSON of unexpected shape | [`ClientError::InvalidPayload`] |
//! | image payload cannot be decoded | [`ClientError::Image`] |
//! | subscriber fell behind the event rate | [`ClientError::Lagged`] |
//!
//! AGP *semantic* outcomes are **not** errors: a `wait_for_quiet` that expires
//! returns an [`Observation`](adesk_core::Observation) with `timed_out = true`,
//! and `type_text` reports unmapped characters in its result.

use adesk_core::ErrorCode;

/// Result alias used throughout this crate.
///
/// The error parameter defaults to [`ClientError`], so `Result<T>` reads the
/// same as in `adesk-core` while still allowing explicit overrides.
pub type Result<T, E = ClientError> = std::result::Result<T, E>;

/// Everything that can go wrong while talking AGP.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// Transport failure while connecting, reading, writing or flushing the
    /// Unix socket.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The peer violated the wire contract: a malformed JSON frame, a line
    /// longer than the configured limit, or a frame kind that is not valid in
    /// this direction.
    #[error("protocol violation: {message}")]
    Protocol {
        /// Human-readable description of the violation.
        message: String,
    },

    /// The connection ended (EOF) while a request was in flight or while events
    /// were being streamed.
    #[error("connection closed")]
    Closed,

    /// The server answered a request with an AGP error frame.
    #[error("server error {code}: {message}")]
    Server {
        /// AGP error code (`docs/protocol.md` §6).
        code: ErrorCode,
        /// Human-readable message supplied by the server.
        message: String,
    },

    /// The server speaks a different AGP version; the client refuses to
    /// continue (protocol §5.1: "clients MUST refuse a mismatch").
    #[error("protocol version mismatch: client speaks v{client}, server speaks v{server}")]
    VersionMismatch {
        /// Version implemented by this crate.
        client: u32,
        /// Version reported by the server's `ping` result.
        server: u32,
    },

    /// A response payload did not have the shape the method requires (missing
    /// field, wrong type). Distinct from [`ClientError::Protocol`] because the
    /// frame itself was well-formed.
    #[error("invalid payload: {message}")]
    InvalidPayload {
        /// What was expected and what arrived.
        message: String,
    },

    /// An [`ImagePayload`](crate::ImagePayload) could not be decoded (bad
    /// base64, unknown format, or a PNG decoder failure).
    #[error("image decode failed: {message}")]
    Image {
        /// Underlying decoder message.
        message: String,
    },

    /// An event subscriber could not keep up with the connection's event rate
    /// and events were dropped. The stream stays usable; the consumer should
    /// re-synchronise (for example with `list_windows` or a fresh `observe`).
    #[error("event stream lagged: {skipped} events dropped")]
    Lagged {
        /// Number of events dropped before the next delivered one.
        skipped: u64,
    },
}

impl From<adesk_core::Error> for ClientError {
    fn from(error: adesk_core::Error) -> Self {
        ClientError::Server {
            code: error.code,
            message: error.message,
        }
    }
}
