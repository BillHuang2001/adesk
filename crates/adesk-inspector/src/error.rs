//! Crate-local error type and its mapping to the umbrella [`adesk_core::Error`].
//!
//! The server maps [`Error`] into the AGP error object: `invalid_request` for
//! bad requests, `render_failed` for anything the frame pipeline produced.

/// Errors produced while composing an inspection frame.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The supplied base frame or render target is unusable (for example a
    /// target whose dimensions differ from the input frame).
    #[error("invalid inspection frame: {0}")]
    InvalidFrame(String),
    /// The inspection request is invalid: an empty region or a zero
    /// `max_dimension`.
    #[error("invalid inspection request: {0}")]
    InvalidRequest(String),
    /// A post-processing step delegated to `adesk-render` failed.
    #[error(transparent)]
    Render(#[from] adesk_render::RenderError),
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for adesk_core::Error {
    fn from(error: Error) -> adesk_core::Error {
        match error {
            Error::InvalidRequest(message) => {
                adesk_core::Error::new(adesk_core::ErrorCode::InvalidRequest, message)
            }
            Error::InvalidFrame(message) => {
                adesk_core::Error::new(adesk_core::ErrorCode::RenderFailed, message)
            }
            Error::Render(error) => {
                adesk_core::Error::new(adesk_core::ErrorCode::RenderFailed, error.to_string())
            }
        }
    }
}
