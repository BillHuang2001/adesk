//! The umbrella error type crossing crate boundaries.
//!
//! Every crate exposes its own `thiserror` enum with
//! `impl From<LocalError> for adesk_core::Error`; [`Error`] is what crosses
//! crate boundaries and what `adesk-proto` maps to AGP `error.code`
//! (`docs/protocol.md` §6).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AppId, WindowId};

/// Machine-readable error codes.
///
/// The string form ([`ErrorCode::as_str`]) is the AGP wire value and is part of
/// the protocol contract; it never changes for a given variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request frame or parameters were malformed.
    InvalidRequest,
    /// The requested method does not exist.
    UnknownMethod,
    /// The referenced window id is not known.
    UnknownWindow,
    /// The referenced app id is not known.
    UnknownApp,
    /// The application could not be launched.
    LaunchFailed,
    /// A capture/render request could not produce pixels.
    CaptureFailed,
    /// The renderer failed (see also [`ErrorCode::CaptureFailed`]).
    RenderFailed,
    /// A wait exceeded its deadline.
    Timeout,
    /// The operation is not supported by this runtime build or version.
    NotSupported,
    /// The runtime is busy with an incompatible operation.
    Busy,
    /// Internal invariant violation; never caused by client input.
    Internal,
    /// The runtime is shutting down and cannot serve the request.
    ShuttingDown,
    /// The client and server protocol versions are incompatible.
    ProtocolVersionMismatch,
}

impl ErrorCode {
    /// The snake_case AGP wire name of this code.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::UnknownMethod => "unknown_method",
            ErrorCode::UnknownWindow => "unknown_window",
            ErrorCode::UnknownApp => "unknown_app",
            ErrorCode::LaunchFailed => "launch_failed",
            ErrorCode::CaptureFailed => "capture_failed",
            ErrorCode::RenderFailed => "render_failed",
            ErrorCode::Timeout => "timeout",
            ErrorCode::NotSupported => "not_supported",
            ErrorCode::Busy => "busy",
            ErrorCode::Internal => "internal",
            ErrorCode::ShuttingDown => "shutting_down",
            ErrorCode::ProtocolVersionMismatch => "protocol_version_mismatch",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error that can cross crate boundaries.
///
/// `Display` renders as `"<code>: <message>"`; the message is human-readable
/// and never localized. Serialization matches the AGP error object:
/// `{"code":"unknown_window","message":"..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Human-readable description (never localized).
    pub message: String,
}

impl Error {
    /// Creates an error with an explicit code.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
        }
    }

    /// [`ErrorCode::InvalidRequest`] with a description.
    pub fn invalid_request(message: impl Into<String>) -> Error {
        Error::new(ErrorCode::InvalidRequest, message)
    }

    /// [`ErrorCode::UnknownWindow`] for `id`.
    pub fn unknown_window(id: WindowId) -> Error {
        Error::new(ErrorCode::UnknownWindow, format!("unknown window {id}"))
    }

    /// [`ErrorCode::UnknownApp`] for `id`.
    pub fn unknown_app(id: &AppId) -> Error {
        Error::new(ErrorCode::UnknownApp, format!("unknown app {id}"))
    }

    /// [`ErrorCode::Internal`] with a description.
    pub fn internal(message: impl Into<String>) -> Error {
        Error::new(ErrorCode::Internal, message)
    }

    /// [`ErrorCode::NotSupported`] with a description.
    pub fn not_supported(message: impl Into<String>) -> Error {
        Error::new(ErrorCode::NotSupported, message)
    }

    /// [`ErrorCode::Timeout`] with a description.
    pub fn timeout(message: impl Into<String>) -> Error {
        Error::new(ErrorCode::Timeout, message)
    }
}

/// Crate-wide result alias: `Result<T, adesk_core::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_protocol_error_codes() {
        let expected = [
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::UnknownMethod, "unknown_method"),
            (ErrorCode::UnknownWindow, "unknown_window"),
            (ErrorCode::UnknownApp, "unknown_app"),
            (ErrorCode::LaunchFailed, "launch_failed"),
            (ErrorCode::CaptureFailed, "capture_failed"),
            (ErrorCode::RenderFailed, "render_failed"),
            (ErrorCode::Timeout, "timeout"),
            (ErrorCode::NotSupported, "not_supported"),
            (ErrorCode::Busy, "busy"),
            (ErrorCode::Internal, "internal"),
            (ErrorCode::ShuttingDown, "shutting_down"),
            (
                ErrorCode::ProtocolVersionMismatch,
                "protocol_version_mismatch",
            ),
        ];
        assert_eq!(expected.len(), 13);
        for (code, name) in expected {
            assert_eq!(code.as_str(), name);
            assert_eq!(code.to_string(), name);
            assert_eq!(serde_json::to_value(code).unwrap(), serde_json::json!(name));
            assert_eq!(
                code,
                serde_json::from_value(serde_json::json!(name)).unwrap()
            );
        }
    }

    #[test]
    fn new_and_constructors_set_code_and_message() {
        let err = Error::new(ErrorCode::Busy, "another capture is running");
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(err.message, "another capture is running");

        assert_eq!(
            Error::invalid_request("bad").code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(Error::internal("boom").code, ErrorCode::Internal);
        assert_eq!(Error::not_supported("no").code, ErrorCode::NotSupported);
        assert_eq!(Error::timeout("late").code, ErrorCode::Timeout);
    }

    #[test]
    fn unknown_window_message_contains_id() {
        let err = Error::unknown_window(WindowId(99));
        assert_eq!(err.code, ErrorCode::UnknownWindow);
        assert_eq!(err.message, "unknown window 99");
    }

    #[test]
    fn unknown_app_message_contains_id() {
        let err = Error::unknown_app(&AppId::from("org.mozilla.firefox"));
        assert_eq!(err.code, ErrorCode::UnknownApp);
        assert_eq!(err.message, "unknown app org.mozilla.firefox");
    }

    #[test]
    fn display_includes_code_and_message() {
        let err = Error::new(ErrorCode::Internal, "invariant broken");
        assert_eq!(err.to_string(), "internal: invariant broken");
    }

    #[test]
    fn error_is_a_std_error() {
        let boxed: Box<dyn std::error::Error + Send + Sync> =
            Box::new(Error::new(ErrorCode::Internal, "boom"));
        assert_eq!(boxed.to_string(), "internal: boom");
    }

    #[test]
    fn error_round_trips_through_serde() {
        let err = Error::unknown_window(WindowId(7));
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(
            json,
            r#"{"code":"unknown_window","message":"unknown window 7"}"#
        );
        assert_eq!(serde_json::from_str::<Error>(&json).unwrap(), err);
    }

    #[test]
    fn result_alias_is_usable() {
        fn ok() -> Result<u32> {
            Ok(1)
        }
        fn fail() -> Result<u32> {
            Err(Error::internal("nope"))
        }
        assert_eq!(ok().unwrap(), 1);
        assert_eq!(fail().unwrap_err().code, ErrorCode::Internal);
    }
}
