//! Binding and accepting the AGP Unix socket.

use std::io;
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use crate::error::{Result, ServerError};

/// A bound AGP socket; removes its socket file when dropped.
pub struct SocketListener {
    listener: UnixListener,
    path: PathBuf,
}

impl SocketListener {
    /// Binds `path` after [`prepare_socket_path`], and removes the socket file
    /// when dropped (best effort).
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Io`] when the parent directory cannot be created,
    /// the path is already a live socket, or `bind` fails.
    pub async fn bind(path: &Path) -> Result<SocketListener, ServerError> {
        todo!()
    }

    /// The bound path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accepts the next connection.
    ///
    /// # Errors
    ///
    /// Returns the underlying accept error; callers log and continue unless the
    /// runtime is shutting down.
    pub async fn accept(&self) -> io::Result<UnixStream> {
        todo!()
    }
}

impl Drop for SocketListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Makes `path` bindable: creates the parent directory, refuses to clobber a
/// live socket, and removes a stale socket file.
///
/// A socket file is considered stale when connecting to it fails — an existing
/// runtime keeps its socket alive, so a successful connect is reported as an
/// error rather than silently replaced.
///
/// # Errors
///
/// Returns [`ServerError::Io`] when the path exists and is live, or when it
/// exists and is not a socket.
pub fn prepare_socket_path(path: &Path) -> Result<()> {
    todo!()
}
