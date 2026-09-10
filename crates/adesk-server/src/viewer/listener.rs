//! Binding and accepting the viewer (VAP v1) transports.
//!
//! The transport belongs to the runtime, not to `adesk-viewer`
//! (`docs/viewer.md` §1, §8): this module binds the Unix socket (through the
//! generic [`SocketListener`], so the live-socket and stale-file rules match the
//! AGP socket) and an optional TCP listener, then spawns one accept task per
//! transport. Each accepted stream is served by a shared
//! [`ViewerServer`], which owns the per-connection session.
//!
//! These tasks never run the runtime's ordered teardown: the AGP accept loop is
//! the only owner of [`crate::shutdown::run`]. They hold the shutdown token and
//! stop accepting when it fires; the socket file is removed by
//! [`crate::shutdown::run`] and, as a fallback, by [`SocketListener`]'s RAII
//! drop.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adesk_viewer::{PeerInfo, ViewerServer, ViewerServerConfig};

use crate::context::ServerContext;
use crate::error::{Result, ServerError};
use crate::socket::SocketListener;
use crate::viewer::ViewerBackendImpl;

/// The bound viewer transports.
pub(crate) struct ViewerListener {
    /// The Unix socket every viewer can always reach.
    unix: SocketListener,
    /// The optional TCP listener (opt-in; `None` = Unix only).
    tcp: Option<tokio::net::TcpListener>,
}

impl ViewerListener {
    /// The bound Unix socket path.
    pub(crate) fn unix_path(&self) -> &Path {
        self.unix.path()
    }
}

/// Binds the viewer transports: the Unix socket plus, when `tcp` is set, a TCP
/// listener.
///
/// # Errors
///
/// Returns [`ServerError::Io`] when the Unix socket cannot be bound (a live
/// socket at the path is refused, never clobbered) or the TCP address cannot be
/// bound.
pub(crate) async fn bind(
    unix_path: &Path,
    tcp: Option<SocketAddr>,
) -> Result<ViewerListener, ServerError> {
    let unix = SocketListener::bind(unix_path).await?;
    let tcp = match tcp {
        Some(addr) => Some(tokio::net::TcpListener::bind(addr).await?),
        None => None,
    };
    Ok(ViewerListener { unix, tcp })
}

/// Spawns one accept loop per bound transport.
///
/// `viewer_socket_path` in [`crate::RunningServer`] is only meaningful because
/// the caller binds before this returns, exactly like the AGP socket.
pub(crate) fn spawn_accept_loops(
    listener: ViewerListener,
    context: ServerContext,
    backend: Arc<ViewerBackendImpl>,
) {
    // One server for the whole runtime: the backend is shared and the per
    // connection state lives in the session, not here.
    let server = Arc::new(ViewerServer::new(backend).with_config(ViewerServerConfig::default()));

    let unix_path: PathBuf = listener.unix_path().to_path_buf();
    let ViewerListener { unix, tcp } = listener;

    let unix_context = context.clone();
    let unix_server = Arc::clone(&server);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = unix_context.shutdown.cancelled() => break,
                accepted = unix.accept() => match accepted {
                    Ok(stream) => {
                        let server = Arc::clone(&unix_server);
                        let peer = PeerInfo::Unix(unix_path.clone());
                        tokio::spawn(async move {
                            if let Err(error) = server.serve(stream, peer).await {
                                tracing::debug!(%error, "viewer connection ended with an error");
                            }
                        });
                    }
                    Err(error) => {
                        if unix_context.shutdown.is_shutting_down() {
                            break;
                        }
                        tracing::warn!(%error, "viewer accept failed");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                },
            }
        }
    });

    let Some(tcp) = tcp else {
        return;
    };
    let tcp_server = Arc::clone(&server);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = context.shutdown.cancelled() => break,
                accepted = tcp.accept() => match accepted {
                    Ok((stream, addr)) => {
                        let server = Arc::clone(&tcp_server);
                        let peer = PeerInfo::Tcp(addr);
                        tokio::spawn(async move {
                            if let Err(error) = server.serve(stream, peer).await {
                                tracing::debug!(%error, "viewer connection ended with an error");
                            }
                        });
                    }
                    Err(error) => {
                        if context.shutdown.is_shutting_down() {
                            break;
                        }
                        tracing::warn!(%error, "viewer accept failed");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                },
            }
        }
    });
}
