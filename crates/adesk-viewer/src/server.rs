//! `ViewerServer` façade, `ViewerServerConfig` and `PeerInfo`.
//!
//! [`ViewerServer`] is the entry point `adesk-server` uses to serve the Viewer
//! Attachment Protocol (`docs/viewer.md`). It owns a [`ViewerBackend`] and a
//! [`ViewerServerConfig`]; [`ViewerServer::serve`] runs one already-connected
//! viewer to completion by delegating to the per-connection session in
//! [`crate::session`].
//!
//! Binding the transport (a `UnixListener` or `TcpListener`) is deliberately
//! **not** done here — the runtime owns the socket, and this crate stays
//! transport-agnostic (`docs/viewer.md` §1, §8). [`PeerInfo`] is only a display
//! label for logs.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use adesk_core::OverlayKind;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::backend::ViewerBackend;
use crate::error::Result;
use crate::transport::DEFAULT_MAX_FRAME_LEN;

/// A VAP server that serves viewer connections over a [`ViewerBackend`].
///
/// A server is cheap to clone-by-reference: it holds an `Arc` of the backend and
/// a [`ViewerServerConfig`]. Call [`ViewerServer::serve`] once per accepted
/// connection (typically from a task spawned per connection).
pub struct ViewerServer<B: ViewerBackend> {
    /// The runtime side of every connection.
    backend: Arc<B>,
    /// Handshake/pacing/framing policy for every connection.
    config: ViewerServerConfig,
}

impl<B: ViewerBackend> ViewerServer<B> {
    /// Creates a server over `backend` with the default configuration.
    pub fn new(backend: Arc<B>) -> ViewerServer<B> {
        ViewerServer {
            backend,
            config: ViewerServerConfig::default(),
        }
    }

    /// Returns this server with `config` applied.
    ///
    /// Consuming builder mirroring the workspace's other config types.
    pub fn with_config(mut self, config: ViewerServerConfig) -> ViewerServer<B> {
        self.config = config;
        self
    }

    /// The configuration this server applies to every connection.
    pub fn config(&self) -> &ViewerServerConfig {
        &self.config
    }

    /// Serves one already-connected viewer until it leaves or the connection
    /// closes.
    ///
    /// `stream` may be any bidirectional byte stream (a Unix socket, a TCP
    /// socket, an in-memory duplex in tests); `peer` is only used as a log label.
    /// Binding/accepting the transport is the caller's job (`docs/viewer.md`
    /// §1, §8).
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ViewerError`] when the transport fails. A viewer
    /// that violates the handshake is refused with a VAP `error` and this method
    /// still returns `Ok(())`; the session never panics.
    pub async fn serve<S>(&self, stream: S, peer: PeerInfo) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        crate::session::run_session(&self.backend, &self.config, stream, &peer).await
    }
}

/// Handshake, framing and pacing policy for a [`ViewerServer`]
/// (`docs/viewer.md` §1, §2, §5).
///
/// The defaults are the ones the protocol recommends: a 5-second handshake
/// timeout, the [`DEFAULT_MAX_FRAME_LEN`] line cap, no default pacing (a viewer
/// that asks for `min_interval_ms == 0` gets a frame per desktop change) and the
/// [`adesk_viewer_proto::DEFAULT_OVERLAYS`] overlay set.
#[derive(Debug, Clone)]
pub struct ViewerServerConfig {
    /// How long a viewer has to send its `hello` before the connection is
    /// refused (§2).
    pub handshake_timeout: Duration,
    /// Maximum length of one inbound NDJSON line, in bytes (§1).
    pub max_frame_len: usize,
    /// Pacing used when a viewer asks for `min_interval_ms == 0` (§2, §5).
    pub default_min_interval_ms: u64,
    /// Overlay set used when a viewer sends an empty `overlays` list (§2).
    pub default_overlays: Vec<OverlayKind>,
}

impl Default for ViewerServerConfig {
    fn default() -> ViewerServerConfig {
        ViewerServerConfig {
            handshake_timeout: Duration::from_secs(5),
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
            default_min_interval_ms: 0,
            default_overlays: adesk_viewer_proto::DEFAULT_OVERLAYS.to_vec(),
        }
    }
}

impl ViewerServerConfig {
    /// Returns this config with a different handshake timeout.
    pub fn with_handshake_timeout(mut self, handshake_timeout: Duration) -> ViewerServerConfig {
        self.handshake_timeout = handshake_timeout;
        self
    }

    /// Returns this config with a different inbound line cap, in bytes.
    pub fn with_max_frame_len(mut self, max_frame_len: usize) -> ViewerServerConfig {
        self.max_frame_len = max_frame_len;
        self
    }

    /// Returns this config with a different default pacing interval, in
    /// milliseconds.
    pub fn with_default_min_interval_ms(
        mut self,
        default_min_interval_ms: u64,
    ) -> ViewerServerConfig {
        self.default_min_interval_ms = default_min_interval_ms;
        self
    }

    /// Returns this config with a different default overlay set.
    pub fn with_default_overlays(
        mut self,
        default_overlays: Vec<OverlayKind>,
    ) -> ViewerServerConfig {
        self.default_overlays = default_overlays;
        self
    }
}

/// A human-readable label for a viewer connection, used only in logs.
///
/// Constructed by whoever accepts the connection (`adesk-server`), since binding
/// the transport is not this crate's job (`docs/viewer.md` §1).
#[derive(Debug, Clone)]
pub enum PeerInfo {
    /// A viewer on a local Unix domain socket.
    Unix(PathBuf),
    /// A viewer on a TCP connection.
    Tcp(SocketAddr),
    /// A viewer on any other transport, labelled by a caller-supplied string.
    Other(String),
}

impl fmt::Display for PeerInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerInfo::Unix(path) => write!(f, "unix:{}", path.display()),
            PeerInfo::Tcp(addr) => write!(f, "tcp:{addr}"),
            PeerInfo::Other(label) => f.write_str(label),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::test_support::FakeBackend;

    /// A fake backend configured differently from the session-test default (an
    /// `8x4` output reporting no action), so the façade is exercised over a
    /// non-default [`ViewerBackend`] configuration.
    fn backend() -> Arc<FakeBackend> {
        Arc::new(
            FakeBackend::new()
                .with_output_size(8, 4)
                .with_action_id(None),
        )
    }
    #[test]
    fn default_config_matches_the_protocol_defaults() {
        let config = ViewerServerConfig::default();
        assert_eq!(config.handshake_timeout, Duration::from_secs(5));
        assert_eq!(config.max_frame_len, DEFAULT_MAX_FRAME_LEN);
        assert_eq!(config.default_min_interval_ms, 0);
        assert_eq!(
            config.default_overlays,
            adesk_viewer_proto::DEFAULT_OVERLAYS.to_vec()
        );
    }

    #[test]
    fn config_builders_override_each_field() {
        let config = ViewerServerConfig::default()
            .with_handshake_timeout(Duration::from_millis(250))
            .with_max_frame_len(1024)
            .with_default_min_interval_ms(33)
            .with_default_overlays(vec![OverlayKind::WindowIds]);
        assert_eq!(config.handshake_timeout, Duration::from_millis(250));
        assert_eq!(config.max_frame_len, 1024);
        assert_eq!(config.default_min_interval_ms, 33);
        assert_eq!(config.default_overlays, vec![OverlayKind::WindowIds]);
    }

    #[test]
    fn server_exposes_its_configuration() {
        let server = ViewerServer::new(backend());
        assert_eq!(server.config().max_frame_len, DEFAULT_MAX_FRAME_LEN);

        let configured = server.with_config(ViewerServerConfig::default().with_max_frame_len(7));
        assert_eq!(configured.config().max_frame_len, 7);
    }

    #[test]
    fn peer_info_renders_a_display_label() {
        assert_eq!(
            PeerInfo::Unix(PathBuf::from("/run/adesk-viewer.sock")).to_string(),
            "unix:/run/adesk-viewer.sock"
        );
        assert_eq!(
            PeerInfo::Tcp("127.0.0.1:1234".parse().unwrap()).to_string(),
            "tcp:127.0.0.1:1234"
        );
        assert_eq!(
            PeerInfo::Other("in-memory".to_owned()).to_string(),
            "in-memory"
        );
    }

    /// A duplicate `hello` is refused and closes the connection, while the
    /// façade reports success (`docs/viewer.md` §2).
    #[tokio::test]
    async fn serve_refuses_a_duplicate_hello_and_returns_ok() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let server = ViewerServer::new(backend());
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let serve = tokio::spawn(async move {
            server
                .serve(server_stream, PeerInfo::Other("test".to_owned()))
                .await
        });

        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let mut lines = BufReader::new(client_read).lines();

        let hello = adesk_viewer_proto::encode_client(&adesk_viewer_proto::ClientMessage::Hello(
            adesk_viewer_proto::ViewerHello::new(),
        ));
        client_write.write_all(hello.as_bytes()).await.unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.flush().await.unwrap();
        // The server replies with its hello.
        let first = lines.next_line().await.unwrap().unwrap();
        assert!(first.contains("\"hello\""), "{first}");

        // A second hello is a duplicate.
        client_write.write_all(hello.as_bytes()).await.unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.flush().await.unwrap();
        let second = lines.next_line().await.unwrap().unwrap();
        assert!(second.contains("duplicate hello"), "{second}");

        serve.await.unwrap().unwrap();
    }
}
