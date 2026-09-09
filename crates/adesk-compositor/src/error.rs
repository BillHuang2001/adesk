//! Error type for the compositor crate.
//!
//! Every fallible public entry point returns [`Result<T>`].
//! [`CompositorError`] converts into [`adesk_core::Error`] at crate boundaries so
//! the AGP layer can report a structured `ErrorCode`.

use adesk_core::{Error as CoreError, ErrorCode, WindowId};

/// Errors produced by `adesk-compositor`.
///
/// The variants group by failure domain: startup (thread/display/socket/loop/
/// renderer/keyboard), lifecycle (not ready / aborted / stopped), and request
/// handling (unknown window / window management / rendering / internal).
#[derive(Debug, thiserror::Error)]
pub enum CompositorError {
    /// The compositor thread could not be spawned.
    #[error("failed to spawn compositor thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    /// The Wayland display could not be created.
    #[error("failed to create wayland display: {0}")]
    Display(String),
    /// The Wayland listening socket could not be bound.
    #[error("failed to bind wayland socket: {0}")]
    Socket(String),
    /// The calloop event loop could not be created or configured.
    #[error("failed to create compositor event loop: {0}")]
    EventLoop(String),
    /// The headless renderer could not be created (neither GL nor pixman).
    #[error("renderer initialization failed: {0}")]
    Renderer(String),
    /// The keyboard (xkb keymap) could not be created.
    #[error("keyboard initialization failed: {0}")]
    Keyboard(String),
    /// Readiness was requested before the compositor finished starting up.
    #[error("compositor is not ready yet")]
    NotReady,
    /// The compositor thread terminated before reporting readiness.
    #[error("compositor thread terminated before reporting readiness")]
    StartupAborted,
    /// The compositor thread has terminated; no further commands can be served.
    #[error("compositor thread has terminated")]
    Stopped,
    /// The requested window is not known to the window manager.
    #[error("window {0} is not known")]
    UnknownWindow(WindowId),
    /// The window manager rejected or failed an operation.
    #[error("window management error: {0}")]
    WindowManagement(String),
    /// Rendering a window or output failed.
    #[error("rendering failed: {0}")]
    Render(String),
    /// An invariant was violated; this is a bug, not a client error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, CompositorError>;

impl CompositorError {
    /// The AGP error code this failure maps to.
    pub fn code(&self) -> ErrorCode {
        match self {
            CompositorError::UnknownWindow(_) => ErrorCode::UnknownWindow,
            CompositorError::Render(_) => ErrorCode::RenderFailed,
            CompositorError::NotReady
            | CompositorError::StartupAborted
            | CompositorError::Stopped => ErrorCode::ShuttingDown,
            CompositorError::ThreadSpawn(_)
            | CompositorError::Display(_)
            | CompositorError::Socket(_)
            | CompositorError::EventLoop(_)
            | CompositorError::Renderer(_)
            | CompositorError::Keyboard(_)
            | CompositorError::WindowManagement(_)
            | CompositorError::Internal(_) => ErrorCode::Internal,
        }
    }
}

impl From<CompositorError> for CoreError {
    fn from(error: CompositorError) -> Self {
        CoreError::new(error.code(), error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_window_maps_to_unknown_window_code() {
        let error = CompositorError::UnknownWindow(WindowId(7));
        assert_eq!(error.code(), ErrorCode::UnknownWindow);
        let core: CoreError = error.into();
        assert_eq!(core.code, ErrorCode::UnknownWindow);
        assert!(core.message.contains('7'));
    }

    #[test]
    fn render_failure_maps_to_render_failed() {
        assert_eq!(CompositorError::Render("boom".into()).code(), ErrorCode::RenderFailed);
    }

    #[test]
    fn startup_failures_map_to_internal() {
        for error in [
            CompositorError::Display("x".into()),
            CompositorError::Socket("x".into()),
            CompositorError::EventLoop("x".into()),
            CompositorError::Renderer("x".into()),
            CompositorError::Keyboard("x".into()),
            CompositorError::Internal("x".into()),
        ] {
            assert_eq!(error.code(), ErrorCode::Internal);
        }
    }

    #[test]
    fn lifecycle_failures_map_to_shutting_down() {
        for error in [
            CompositorError::NotReady,
            CompositorError::StartupAborted,
            CompositorError::Stopped,
        ] {
            assert_eq!(error.code(), ErrorCode::ShuttingDown);
        }
    }
}
