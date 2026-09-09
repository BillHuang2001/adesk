//! Shutdown coordination: stop accepting, fail in-flight requests with
//! `shutting_down`, drop the Wayland display, remove the socket file.
//!
//! [`ShutdownHandle`] is a watch-based token so every task can await
//! cancellation without polling; [`run`] performs the ordered teardown exactly
//! once (`docs/architecture.md` §9).

use std::path::Path;

use tokio::sync::watch;

use crate::context::ServerContext;
use crate::error::Result;

/// Cloneable shutdown token.
#[derive(Debug, Clone)]
pub struct ShutdownHandle {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl ShutdownHandle {
    /// A token in the running state.
    pub fn new() -> ShutdownHandle {
        let (tx, rx) = watch::channel(false);
        ShutdownHandle { tx, rx }
    }

    /// Flags shutdown; returns `true` if this call started it (idempotent).
    pub fn initiate(&self) -> bool {
        !self.tx.send_replace(true)
    }

    /// Whether shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolves once shutdown has been requested (immediately if it already was).
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Default for ShutdownHandle {
    fn default() -> ShutdownHandle {
        ShutdownHandle::new()
    }
}

/// Runs the ordered shutdown sequence exactly once:
///
/// 1. flag shutdown and stop accepting new connections
/// 2. fail in-flight requests with `shutting_down` and drain connection tasks
/// 3. drop the Wayland display (`CompositorHandle::shutdown`)
/// 4. remove the socket file
///
/// # Errors
///
/// Returns the first failure but always attempts the remaining steps.
pub async fn run(context: &ServerContext) -> Result<()> {
    todo!()
}

/// Installs SIGINT/SIGTERM handlers that call [`ShutdownHandle::initiate`].
///
/// Installed by [`crate::Server::start`]; repeated installation is harmless.
///
/// # Errors
///
/// Returns [`crate::ServerError::Io`] if the signal streams cannot be created.
pub async fn install_signal_handlers(handle: ShutdownHandle) -> Result<()> {
    todo!()
}

/// Removes the socket file if present, ignoring "not found".
pub fn remove_socket_file(path: &Path) {
    todo!()
}
