//! Server-level error type and its AGP mapping.
//!
//! Every dispatch arm returns [`Result`]; the dispatcher converts failures into
//! `ErrorPayload`s through [`ServerError::payload`] so a failed request never
//! closes the connection (`docs/protocol.md` §6).

use adesk_core::ErrorCode;

/// Failures that can abort or fail a server operation.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The compositor failed or is unavailable.
    #[error("compositor error: {0}")]
    Compositor(#[from] adesk_compositor::CompositorError),
    /// Socket or filesystem I/O failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The application registry failed.
    #[error("app registry error: {0}")]
    Registry(#[from] adesk_app_registry::Error),
    /// Frame encoding or decoding failed.
    #[error("protocol error: {0}")]
    Proto(#[from] adesk_proto::ProtoError),
    /// The observer rejected the request (unknown window/action, internal).
    #[error("observer error: {0}")]
    Observer(#[from] adesk_observer::Error),
    /// The inspector could not compose an inspection frame.
    #[error("inspector error: {0}")]
    Inspector(#[from] adesk_inspector::Error),
    /// Rendering or image encoding failed.
    #[error("render error: {0}")]
    Render(#[from] adesk_render::RenderError),
    /// The runtime is shutting down and cannot serve the request.
    #[error("server is shutting down")]
    ShuttingDown,
    /// A background task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// An internal invariant was violated (never caused by client input).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience result for server operations.
///
/// The error type defaults to [`ServerError`], so `Result<T>` is the common
/// spelling while the startup/bind paths can name `ServerError` explicitly.
pub type Result<T, E = ServerError> = std::result::Result<T, E>;

impl ServerError {
    /// The AGP error code (`docs/protocol.md` §6) this failure maps to.
    pub fn code(&self) -> ErrorCode {
        todo!()
    }

    /// Builds the wire error payload for a failed request.
    pub fn payload(&self) -> adesk_proto::ErrorPayload {
        todo!()
    }
}

impl From<ServerError> for adesk_core::Error {
    /// Lossy conversion used at crate boundaries (`adesk_core::Error` is the
    /// umbrella error; its `code` mirrors [`ServerError::code`]).
    fn from(error: ServerError) -> adesk_core::Error {
        todo!()
    }
}
