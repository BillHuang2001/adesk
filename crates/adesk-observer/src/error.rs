//! Crate-local error type, mapped onto `adesk_core::Error` at the crate boundary.
//!
//! Convention (root `CONTEXT.md`): every crate exposes a `thiserror` enum plus
//! `pub type Result<T>`; `adesk_core::Error` is the umbrella used at crate
//! boundaries and carries the AGP `ErrorCode` values.

use adesk_core::{ActionId, Error as CoreError, WindowId};

/// Errors produced by the observation engine.
///
/// Waits never fail because of time: a deadline produces an [`adesk_core::Observation`]
/// with `timed_out: true`. `Error` is reserved for unusable *requests*.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `window_id` filter names a window the observer does not (or no longer)
    /// tracks — it was never created, or it was destroyed and its state dropped.
    #[error("window {0} is not known")]
    UnknownWindow(WindowId),

    /// An `after_action` filter names an action that was never recorded by this
    /// observer. Observations must not silently guess the causal point.
    #[error("action {0} is not known")]
    UnknownAction(ActionId),

    /// Internal invariant violated. Never returned for client input.
    #[error("observer internal error: {0}")]
    Internal(String),
}

impl From<Error> for CoreError {
    fn from(value: Error) -> Self {
        match value {
            Error::UnknownWindow(window_id) => CoreError::unknown_window(window_id),
            Error::UnknownAction(action_id) => {
                CoreError::invalid_request(format!("action {action_id} is not known"))
            }
            Error::Internal(message) => CoreError::internal(message),
        }
    }
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, Error>;
