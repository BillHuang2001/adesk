//! Viewer-facing VAP v1 endpoint (`docs/viewer.md`).
//!
//! The runtime serves the Viewer Attachment Protocol so that a human outside the
//! AI machine can observe the desktop and — within the limits the protocol
//! grants — act on it. The wire types, the codec and the per-connection session
//! belong to the `adesk-viewer-proto` / `adesk-viewer` crates; this module is only
//! the *runtime* half: it implements [`adesk_viewer::ViewerBackend`] over
//! [`crate::context::ServerContext`], binds the transports and accepts viewer
//! connections.
//!
//! Viewer input goes through the **same seat path** as AGP §5.5 input: the
//! backend records an `ActionId` on the observer before issuing the compositor
//! command, runs the command through a per-backend [`crate::session::InputQueue`]
//! so it keeps submission order, and resolves the pointer against the active
//! window through the window model. Native operations (`activate_if_needed`) stay
//! compositor state changes, never synthesized input (invariant 3). There is no
//! viewer-only input path.
//!
//! Rendering is on demand exactly like §5.7 inspection: a frame is rendered only
//! when the session asks for one, and the frame's `seq` is reserved from the
//! compositor's single global monotonic counter
//! (`crate::dispatch::windows::reserve_seq`), never derived from the observed
//! watermark.

mod backend;
mod listener;

pub(crate) use backend::ViewerBackendImpl;
pub(crate) use listener::{bind, spawn_accept_loops, ViewerListener};

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::ServerConfig;
use crate::context::ServerContext;
use crate::error::{Result, ServerError};

/// Starts the viewer endpoint described by `config` (binds the transports and
/// spawns the accept loops) and returns the resolved Unix socket path, or `None`
/// when the endpoint is disabled.
///
/// Binding happens here so [`crate::Server::start`] returns only once the viewer
/// socket accepts connections, exactly like the AGP socket.
///
/// # Errors
///
/// Returns [`ServerError::Io`] when either transport cannot be bound (including
/// a live socket at the resolved path, which is never clobbered).
pub(crate) async fn start(
    config: &ServerConfig,
    context: &ServerContext,
) -> Result<Option<PathBuf>, ServerError> {
    let Some(unix_path) = config.viewer_socket_path() else {
        return Ok(None);
    };
    let backend = Arc::new(ViewerBackendImpl::new(context.clone()));
    // `ViewerListener` is named explicitly so the re-export above is a real
    // reference rather than a decoration.
    let listener: ViewerListener = bind(&unix_path, config.viewer.tcp).await?;
    tracing::info!(
        socket = %unix_path.display(),
        tcp = ?config.viewer.tcp,
        "viewer endpoint bound"
    );
    spawn_accept_loops(listener, context.clone(), backend);
    Ok(Some(unix_path))
}
