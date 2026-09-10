//! Crate-local error type.

use adesk_core::WindowId;

/// Errors produced by the window model.
///
/// Pure policy has exactly one failure mode: a window id the manager does not
/// track. Mutating calls ignore unknown ids instead of failing (the compositor
/// may legitimately observe an event after the window was destroyed); this
/// error exists for command paths that must answer the AGP `unknown_window`
/// code, see [`crate::WindowManager::require_window`].
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The window id is not tracked by the manager.
    #[error("unknown window {0}")]
    UnknownWindow(WindowId),
}

/// Crate-wide result alias: `Result<T, adesk_wm::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_stable() {
        assert_eq!(
            Error::UnknownWindow(WindowId(9)).to_string(),
            "unknown window 9"
        );
    }
}
