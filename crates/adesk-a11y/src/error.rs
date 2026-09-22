//! Crate-local error type, mapped onto `adesk_core::Error` at the crate boundary.
//!
//! Convention (root `CONTEXT.md`): every crate exposes a `thiserror` enum plus
//! `pub type Result<T>`; `adesk_core::Error` is the umbrella used at crate
//! boundaries and carries the AGP `ErrorCode` values crossed with the wire
//! (`docs/protocol.md` §6). The mapping is deliberately total: no accessibility
//! failure may reach a client as a bare `internal` where the spec names a
//! specific code.

use adesk_core::{AccessibleId, Error as CoreError, WindowId};

/// Errors produced by the accessibility subsystem.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum A11yError {
    /// No accessibility backend is available in this environment: there is no
    /// accessibility bus to connect to, or the runtime was started with
    /// accessibility turned off. A normal answer (`not_supported`), never a
    /// partially filled tree.
    #[error("accessibility backend unavailable: {0}")]
    Unavailable(String),

    /// No accessible subtree correlates with the given window: no accessible
    /// frame matched its title, its application or its pid. Reported rather than
    /// guessed (`not_supported`) — a wrong tree is worse than a missing one.
    #[error("no accessible subtree correlates with window {0}")]
    NotCorrelated(WindowId),

    /// The referenced accessible element is unknown: it was never seen, or its
    /// backing object is gone (`unknown_accessible`).
    #[error("unknown accessible node {0}")]
    UnknownNode(AccessibleId),

    /// The element does not expose the requested action.
    #[error("accessible node {id} exposes no action {action}")]
    UnknownAction {
        /// The element addressed.
        id: AccessibleId,
        /// The action name that was not found.
        action: String,
    },

    /// The request was malformed (e.g. `max_results` of zero).
    #[error("invalid accessibility request: {0}")]
    InvalidRequest(String),

    /// The accessibility backend failed: a D-Bus error, a malformed reply, or a
    /// walk that exceeded its time bound.
    #[error("accessibility backend error: {0}")]
    Backend(String),
}

impl From<A11yError> for CoreError {
    fn from(value: A11yError) -> Self {
        match value {
            A11yError::Unavailable(message) => {
                CoreError::not_supported(format!("accessibility backend unavailable: {message}"))
            }
            A11yError::NotCorrelated(window_id) => CoreError::not_supported(format!(
                "no accessible subtree correlates with window {window_id}"
            )),
            A11yError::UnknownNode(id) => CoreError::unknown_accessible(id),
            A11yError::UnknownAction { id, action } => CoreError::invalid_request(format!(
                "accessible node {id} exposes no action {action}"
            )),
            A11yError::InvalidRequest(message) => CoreError::invalid_request(message),
            A11yError::Backend(message) => {
                CoreError::internal(format!("accessibility backend error: {message}"))
            }
        }
    }
}

/// Convenience result alias for this crate.
pub type Result<T> = std::result::Result<T, A11yError>;

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;

    #[test]
    fn unavailable_maps_to_not_supported_and_keeps_the_detail() {
        let mapped: CoreError = A11yError::Unavailable("no session bus".into()).into();
        assert_eq!(mapped.code, ErrorCode::NotSupported);
        assert!(mapped.message.contains("no session bus"));
    }

    #[test]
    fn not_correlated_maps_to_not_supported_and_names_the_window() {
        let mapped: CoreError = A11yError::NotCorrelated(WindowId(42)).into();
        assert_eq!(mapped.code, ErrorCode::NotSupported);
        assert!(mapped.message.contains("42"));
    }

    #[test]
    fn unknown_node_maps_to_unknown_accessible() {
        let mapped: CoreError = A11yError::UnknownNode(AccessibleId(7)).into();
        assert_eq!(mapped.code, ErrorCode::UnknownAccessible);
        assert_eq!(mapped.message, "unknown accessible 7");
    }

    #[test]
    fn unknown_action_maps_to_invalid_request_and_names_both() {
        let mapped: CoreError = A11yError::UnknownAction {
            id: AccessibleId(7),
            action: "press".into(),
        }
        .into();
        assert_eq!(mapped.code, ErrorCode::InvalidRequest);
        assert!(mapped.message.contains('7'));
        assert!(mapped.message.contains("press"));
    }

    #[test]
    fn invalid_request_maps_to_invalid_request() {
        let mapped: CoreError = A11yError::InvalidRequest("max_results is zero".into()).into();
        assert_eq!(mapped.code, ErrorCode::InvalidRequest);
        assert_eq!(mapped.message, "max_results is zero");
    }

    #[test]
    fn backend_maps_to_internal_and_keeps_the_detail() {
        let mapped: CoreError = A11yError::Backend("walk timed out".into()).into();
        assert_eq!(mapped.code, ErrorCode::Internal);
        assert!(mapped.message.contains("walk timed out"));
    }

    #[test]
    fn error_is_a_std_error_with_a_stable_display() {
        let err = A11yError::UnknownNode(AccessibleId(3));
        assert_eq!(err.to_string(), "unknown accessible node 3");
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
        assert_eq!(boxed.to_string(), "unknown accessible node 3");
    }
}
