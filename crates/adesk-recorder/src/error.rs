//! Errors produced by the recorder.
//!
//! Every failure path in this crate is a [`RecorderError`]; recording never
//! panics on the frame path. [`RecorderError::code`] maps a failure to the AGP
//! [`ErrorCode`] that `adesk-server` reports, and the `From` impl turns it into
//! the umbrella [`adesk_core::Error`] at crate boundaries.

use adesk_core::ErrorCode;

/// Errors produced while recording rendered frames to a video file.
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    /// An I/O operation failed (creating the output, writing frame data, ...).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A frame could not be encoded (bad dimensions, malformed pixel data, ...).
    #[error("frame encoding failed: {0}")]
    Encode(String),
    /// The requested backend or encoding is not available on this platform.
    #[error("recording not supported: {0}")]
    Unsupported(String),
    /// The video container could not be muxed (RIFF/AVI structure, index, ...).
    #[error("container muxing failed: {0}")]
    Mux(String),
    /// An encoder backend (child process / external tool) failed.
    #[error("encoder backend failed: {0}")]
    Backend(String),
}

impl RecorderError {
    /// The AGP [`ErrorCode`] this failure is reported as.
    ///
    /// An unavailable backend maps to [`ErrorCode::NotSupported`]; encoding and
    /// muxing failures map to [`ErrorCode::CaptureFailed`]; I/O failures map to
    /// [`ErrorCode::Internal`]; backend (child-process) failures map to
    /// [`ErrorCode::RenderFailed`].
    pub fn code(&self) -> ErrorCode {
        match self {
            RecorderError::Unsupported(_) => ErrorCode::NotSupported,
            RecorderError::Encode(_) | RecorderError::Mux(_) => ErrorCode::CaptureFailed,
            RecorderError::Io(_) => ErrorCode::Internal,
            RecorderError::Backend(_) => ErrorCode::RenderFailed,
        }
    }
}

impl From<RecorderError> for adesk_core::Error {
    fn from(err: RecorderError) -> Self {
        adesk_core::Error::new(err.code(), err.to_string())
    }
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, RecorderError>;
