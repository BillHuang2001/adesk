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

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adesk_viewer::{PeerInfo, ViewerServer, ViewerServerConfig};
use tokio::io::{AsyncRead, AsyncWrite};

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

    tokio::spawn(accept_loop(
        context.clone(),
        Arc::clone(&server),
        UnixTransport {
            listener: unix,
            path: unix_path,
        },
    ));

    let Some(tcp) = tcp else {
        return;
    };
    tokio::spawn(accept_loop(context, server, TcpTransport(tcp)));
}

/// One bound viewer transport: accepts a connection and labels it with its peer.
///
/// Both transports are served identically — same accept-error handling, same
/// biased shutdown cancellation, same per-connection task — so the accept loop is
/// written once against this trait and only the accepted stream type and the
/// [`PeerInfo`] variant differ.
trait ViewerTransport {
    /// The accepted connection, handed to [`ViewerServer::serve`].
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Accepts the next connection, labelled with its peer for logging.
    fn accept(&self) -> impl Future<Output = std::io::Result<(Self::Stream, PeerInfo)>> + Send;
}

/// The Unix transport: one socket every viewer can always reach.
struct UnixTransport {
    listener: SocketListener,
    path: PathBuf,
}

impl ViewerTransport for UnixTransport {
    type Stream = tokio::net::UnixStream;

    async fn accept(&self) -> std::io::Result<(Self::Stream, PeerInfo)> {
        let stream = self.listener.accept().await?;
        Ok((stream, PeerInfo::Unix(self.path.clone())))
    }
}

/// The optional TCP transport (opt-in; `--viewer-tcp`).
struct TcpTransport(tokio::net::TcpListener);

impl ViewerTransport for TcpTransport {
    type Stream = tokio::net::TcpStream;

    async fn accept(&self) -> std::io::Result<(Self::Stream, PeerInfo)> {
        let (stream, addr) = self.0.accept().await?;
        Ok((stream, PeerInfo::Tcp(addr)))
    }
}

/// Serves one bound transport until the runtime shuts down.
///
/// Each accepted connection is served by a spawned task sharing the single
/// [`ViewerServer`]; a serve failure is logged at debug and never tears the loop
/// down. The shutdown arm is biased, so a fired shutdown token wins over a pending
/// accept, and a transient accept error backs off for 10 ms before retrying — but
/// stops immediately when the runtime is shutting down.
async fn accept_loop<T>(
    context: ServerContext,
    server: Arc<ViewerServer<ViewerBackendImpl>>,
    transport: T,
) where
    T: ViewerTransport + Send + 'static,
{
    loop {
        tokio::select! {
            biased;
            () = context.shutdown.cancelled() => break,
            accepted = transport.accept() => match accepted {
                Ok((stream, peer)) => {
                    let server = Arc::clone(&server);
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
}
