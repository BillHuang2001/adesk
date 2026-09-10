//! The crate error type shared by the server session, the client SDK and the
//! capture helpers.
//!
//! One enum covers every failure mode of `adesk-viewer`:
//!
//! | Cause | Variant |
//! |---|---|
//! | connect/read/write/flush failure | [`ViewerError::Io`] |
//! | malformed VAP wire message | [`ViewerError::Protocol`] |
//! | peer (or local stream) closed | [`ViewerError::Closed`] |
//! | VAP handshake refused | [`ViewerError::Handshake`] |
//! | the runtime backend failed | [`ViewerError::Backend`] |
//! | peer speaks a different VAP version | [`ViewerError::VersionMismatch`] |
//! | NDJSON framing failure | [`ViewerError::Transport`] |
//!
//! VAP reports every wire failure through `adesk_viewer_proto::ViewerProtoError`;
//! this type wraps it ([`ViewerError::Protocol`]) so a single `Result` covers the
//! protocol and its transport.

use thiserror::Error;

/// Everything that can go wrong in `adesk-viewer`.
#[derive(Debug, Error)]
pub enum ViewerError {
    /// Transport failure while reading, writing or flushing a stream.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A VAP message could not be encoded or decoded (`docs/viewer.md` §1).
    #[error(transparent)]
    Protocol(#[from] adesk_viewer_proto::ViewerProtoError),

    /// The peer (or the local stream) closed before the exchange completed.
    #[error("connection closed")]
    Closed,

    /// The VAP handshake failed: a missing or wrong first message, or a
    /// handshake timeout (`docs/viewer.md` §2).
    #[error("handshake failed: {0}")]
    Handshake(String),

    /// The runtime backend could not complete an operation: rendering a frame,
    /// reading the desktop state, or applying viewer input.
    #[error("backend error: {0}")]
    Backend(String),

    /// The peer speaks a different VAP protocol version (`docs/viewer.md` §2, §7).
    ///
    /// The convention matches `adesk_viewer_proto::ViewerProtoError::VersionMismatch`:
    /// `client` is the version the peer reported and `server` is the version this
    /// build implements.
    #[error("protocol version mismatch: this build speaks v{server}, peer sent v{client}")]
    VersionMismatch {
        /// Version reported by the peer.
        client: u32,
        /// Version this build implements.
        server: u32,
    },

    /// NDJSON framing failed: a line exceeded the length cap or was not valid
    /// UTF-8 (`docs/viewer.md` §1).
    #[error("transport error: {0}")]
    Transport(String),
}

/// Result alias used throughout the crate.
///
/// The error parameter defaults to [`ViewerError`], so `Result<T>` reads the same
/// as in the sibling crates while still allowing explicit overrides.
pub type Result<T, E = ViewerError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_convert() {
        let error: ViewerError = std::io::Error::from(std::io::ErrorKind::NotFound).into();
        assert!(matches!(error, ViewerError::Io(_)));
    }

    #[test]
    fn proto_errors_convert() {
        let error: ViewerError = adesk_viewer_proto::ViewerProtoError::Malformed.into();
        assert!(matches!(error, ViewerError::Protocol(_)));
    }

    #[test]
    fn version_mismatch_reports_both_versions() {
        let error = ViewerError::VersionMismatch {
            client: 2,
            server: 1,
        };
        let text = error.to_string();
        assert!(text.contains("v1"), "{text}");
        assert!(text.contains("v2"), "{text}");
    }
}
