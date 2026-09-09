//! Crate-local error type and its mapping to the umbrella [`adesk_core::Error`].

use adesk_core::WindowId;

/// Errors produced by the window model.
///
/// Pure policy has exactly one failure mode: a window id the manager does not
/// track. Mutating calls ignore unknown ids instead of failing (the compositor
/// may legitimately observe an event after the window was destroyed); this
/// error exists for command paths that must answer the AGP `unknown_window`
/// code, see [`crate::WindowManager::require_window`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The window id is not tracked by the manager.
    #[error("unknown window {0}")]
    UnknownWindow(WindowId),
}

impl From<Error> for adesk_core::Error {
    fn from(err: Error) -> adesk_core::Error {
        match err {
            Error::UnknownWindow(id) => adesk_core::Error::unknown_window(id),
        }
    }
}

/// Crate-wide result alias: `Result<T, adesk_wm::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;

    #[test]
    fn display_is_stable() {
        assert_eq!(
            Error::UnknownWindow(WindowId(9)).to_string(),
            "unknown window 9"
        );
    }

    #[test]
    fn unknown_window_maps_to_core_unknown_window() {
        let err: adesk_core::Error = Error::UnknownWindow(WindowId(9)).into();
        assert_eq!(err.code, ErrorCode::UnknownWindow);
        assert_eq!(err.message, "unknown window 9");
    }
}
