//! The crate error type shared by the GTK-free modules and (later) the GTK layer.
//!
//! `adesk-viewer-gui` composes a CLI/config layer, a VAP client and a local
//! image decoder, so one small enum covers every failure the front-end can hit:
//!
//! | Cause | Variant |
//! |---|---|
//! | a CLI/configuration problem | [`GuiError::Config`] |
//! | a VAP connection/protocol failure | [`GuiError::Client`] |
//! | a local image-decode failure | [`GuiError::Image`] |

use thiserror::Error;

/// Everything that can go wrong in `adesk-viewer-gui`.
#[derive(Debug, Error)]
pub enum GuiError {
    /// A CLI or configuration problem: an unknown flag combination or an
    /// unparseable `--tcp` address.
    #[error("configuration error: {0}")]
    Config(String),

    /// A VAP connection or protocol failure, forwarded verbatim from the
    /// `adesk-viewer` client SDK.
    #[error(transparent)]
    Client(#[from] adesk_viewer::ViewerError),

    /// A local image-decode failure (an undecodable PNG payload, malformed
    /// RGBA8 bytes, …).
    #[error("image error: {0}")]
    Image(String),
}

/// Result alias used throughout the crate.
///
/// The error parameter defaults to [`GuiError`], matching the workspace
/// convention that every crate exposes its own `Result<T>`.
pub type Result<T> = std::result::Result<T, GuiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_errors_convert_into_client() {
        let error: GuiError = adesk_viewer::ViewerError::Closed.into();
        assert!(matches!(error, GuiError::Client(_)), "{error:?}");
    }

    #[test]
    fn config_and_image_errors_render_their_reason() {
        let error = GuiError::Config("bad --tcp".to_owned());
        assert!(error.to_string().contains("bad --tcp"), "{error}");

        let error = GuiError::Image("truncated PNG".to_owned());
        assert!(error.to_string().contains("truncated PNG"), "{error}");
    }
}
