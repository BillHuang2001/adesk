//! Crate-local error type, mapped onto `adesk_core::Error` at the crate boundary.
//!
//! Convention (root `CONTEXT.md`): every crate exposes a `thiserror` enum plus
//! `pub type Result<T>`; `adesk_core::Error` is the umbrella used at crate
//! boundaries and carries the AGP `ErrorCode` values.
//!
//! `wait_for_events` never fails because of time — a deadline produces an
//! [`crate::EventBatch`] with `timed_out: true` — so [`Error`] is reserved for
//! unusable *requests* (the §5.9 store mutations).

use adesk_core::{Error as CoreError, ErrorCode, NotificationId};

/// Errors produced by the notification store and the event inbox.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `notification_id` names a notification the store does not hold —
    /// it was never posted (`docs/protocol.md` §5.9, `unknown_notification`).
    #[error("notification {0} is not known")]
    UnknownNotification(NotificationId),

    /// The request is structurally unusable: an empty `title` for
    /// `post_notification`, or an `action_key` not present in the
    /// notification's `actions` (`docs/protocol.md` §5.9, `invalid_request`).
    #[error("invalid notification request: {0}")]
    InvalidRequest(String),

    /// Internal invariant violated. Never returned for client input.
    #[error("notify internal error: {0}")]
    Internal(String),
}

impl From<Error> for CoreError {
    fn from(value: Error) -> Self {
        match value {
            Error::UnknownNotification(notification_id) => CoreError::new(
                ErrorCode::UnknownNotification,
                format!("unknown notification {notification_id}"),
            ),
            Error::InvalidRequest(message) => CoreError::invalid_request(message),
            Error::Internal(message) => CoreError::internal(message),
        }
    }
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_notification_maps_to_the_agp_code() {
        let mapped: CoreError = Error::UnknownNotification(NotificationId(7)).into();
        assert_eq!(mapped.code, ErrorCode::UnknownNotification);
        assert!(mapped.message.contains('7'));
    }

    #[test]
    fn invalid_request_maps_to_the_agp_code() {
        let mapped: CoreError = Error::InvalidRequest("empty title".into()).into();
        assert_eq!(mapped.code, ErrorCode::InvalidRequest);
        assert_eq!(mapped.message, "empty title");
    }

    #[test]
    fn internal_maps_to_the_agp_code() {
        let mapped: CoreError = Error::Internal("boom".into()).into();
        assert_eq!(mapped.code, ErrorCode::Internal);
    }
}
