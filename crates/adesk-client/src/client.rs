//! The `Client` handle and its connection options.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::transport::Connection;
use crate::wire::PROTOCOL_VERSION;
use crate::Result;

/// Default cap on an inbound NDJSON line (16 MiB).
///
/// A 1280x800 RGBA8 payload is ~4 MiB, or ~5.5 MiB base64-encoded; PNG is
/// smaller. The cap exists to bound memory on a hostile or broken peer.
pub const DEFAULT_MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Default connect timeout (5 s).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve the default socket path the runtime listens on.
///
/// Order: `$ADESK_SOCKET`, then `$XDG_RUNTIME_DIR/adesk.sock`, then
/// `<system temp directory>/adesk.sock` (protocol §1).
///
/// The final fallback is `std::env::temp_dir().join("adesk.sock")`, which
/// honours `$TMPDIR`; it is the exact expression
/// `adesk_server::default_socket_path()` uses, so a default-configured client
/// and a default-configured server meet under any `TMPDIR`.
pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ADESK_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("adesk.sock");
    }
    std::env::temp_dir().join("adesk.sock")
}

/// Options for [`Client::connect_with`].
///
/// `#[non_exhaustive]`: use [`ConnectOptions::new`] plus the builder methods so
/// new options can be added without breaking callers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConnectOptions {
    /// Unix socket path to connect to.
    pub path: PathBuf,
    /// Hard cap on one inbound NDJSON line, in bytes.
    pub max_frame_len: usize,
    /// How long to wait for the socket to accept the connection; `None` waits
    /// indefinitely.
    pub connect_timeout: Option<Duration>,
    /// Perform a `ping` right after connecting and refuse a protocol version
    /// mismatch (protocol §5.1). Default `true`.
    pub verify_version: bool,
}

impl ConnectOptions {
    /// Options for `path` with protocol defaults.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            verify_version: true,
        }
    }

    /// Override the socket path.
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }

    /// Override the inbound line cap.
    pub fn max_frame_len(mut self, max_frame_len: usize) -> Self {
        self.max_frame_len = max_frame_len;
        self
    }

    /// Override the connect timeout (`None` = wait forever).
    pub fn connect_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Enable/disable the post-connect `ping` version check.
    pub fn verify_version(mut self, verify: bool) -> Self {
        self.verify_version = verify;
        self
    }
}

impl Default for ConnectOptions {
    /// Options for [`default_socket_path`].
    fn default() -> Self {
        Self::new(default_socket_path())
    }
}

/// Async handle to one AGP connection.
///
/// `Client` is cheap to clone: all clones share the same socket, request-id
/// space, pending-request map and event fan-out. Every method takes `&self`, so
/// one handle can be shared across tasks.
///
/// Requests are multiplexed: callers may have many in flight and responses are
/// matched by id, so out-of-order replies are handled. Input methods on the
/// same connection are executed by the server in submission order (protocol
/// §5.5), which is what preserves `click` → `type_text` causality.
pub struct Client {
    /// Shared connection state.
    pub(crate) inner: Arc<Connection>,
}

impl Client {
    /// Connect to `path` with protocol defaults.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Client> {
        Self::connect_with(ConnectOptions::new(path.as_ref())).await
    }

    /// Connect to [`default_socket_path`] with protocol defaults.
    pub async fn connect_default() -> Result<Client> {
        Self::connect_with(ConnectOptions::default()).await
    }

    /// Connect with explicit options.
    ///
    /// When `options.verify_version` is set (the default), a `ping` is
    /// performed immediately and a `protocol_version` mismatch fails the
    /// connect with [`ClientError::VersionMismatch`](crate::ClientError::VersionMismatch).
    pub async fn connect_with(options: ConnectOptions) -> Result<Client> {
        let verify_version = options.verify_version;
        let inner = Connection::connect(options).await?;
        let client = Client { inner };
        if verify_version {
            client.ping().await?;
        }
        Ok(client)
    }

    /// The socket path this client is connected to.
    pub fn socket_path(&self) -> &Path {
        self.inner.path()
    }

    /// Whether the connection has been closed or broken.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Close the shared connection (all clones become unusable) and wait for
    /// the background tasks to finish.
    ///
    /// In-flight requests fail with [`ClientError::Closed`](crate::ClientError::Closed);
    /// event streams end.
    pub async fn close(self) -> Result<()> {
        self.inner.close().await
    }

    /// The protocol version this client speaks (`docs/protocol.md` §5.1).
    pub fn protocol_version(&self) -> u32 {
        PROTOCOL_VERSION
    }

    /// Send one request and deserialise its result (crate-internal helper).
    pub(crate) async fn request<P, R>(&self, method: &str, params: &P) -> Result<R>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.inner.request(method, params).await
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Client {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("path", &self.inner.path())
            .field("closed", &self.inner.is_closed())
            .field("protocol_version", &PROTOCOL_VERSION)
            .finish()
    }
}
