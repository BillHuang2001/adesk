//! Shutdown coordination: stop accepting, fail in-flight requests with
//! `shutting_down`, drop the Wayland display, remove the socket file.
//!
//! [`ShutdownHandle`] is a watch-based token so every task can await
//! cancellation without polling; [`run`] performs the ordered teardown exactly
//! once (`docs/architecture.md` §9).

use std::path::Path;

use adesk_compositor::CompositorError;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;

use crate::context::ServerContext;
use crate::error::{Result, ServerError};

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
/// Step 1 is the only thing this function needs to do for step 2: the accept
/// loop, every connection task and every background pump hold a clone of the
/// token and observe it themselves, so flagging shutdown is what makes them
/// stop accepting, answer in-flight requests with [`ServerError::ShuttingDown`]
/// and finish their writes. Draining is therefore cooperative and owned by
/// `socket.rs`/`connection.rs`; `run` only waits for the compositor.
///
/// # Errors
///
/// Returns the first failure but always attempts the remaining steps.
pub async fn run(context: &ServerContext) -> Result<()> {
    // 1. Flag shutdown. Idempotent and non-blocking; a second caller (signal
    //    handler task, a second `RunningServer::shutdown`) is harmless.
    context.shutdown.initiate();

    let mut first_error: Option<ServerError> = None;

    // 3. Drop the Wayland display: clients lose their connection immediately.
    match context.compositor.shutdown().await {
        Ok(()) => {}
        // The compositor thread is already gone — exactly the state this step
        // establishes. Treat it as success so repeated teardown stays
        // idempotent (`RunningServer::shutdown` may be called more than once).
        Err(CompositorError::Stopped) => {
            tracing::debug!("compositor already stopped");
        }
        Err(error) => first_error = Some(error.into()),
    }

    // 4. Remove the socket file, so the path is free before `wait()` resolves.
    remove_socket_file(&context.config.socket_path);

    match first_error {
        Some(error) => {
            tracing::warn!(%error, "shutdown finished with errors");
            Err(error)
        }
        None => {
            tracing::info!("shutdown complete");
            Ok(())
        }
    }
}

/// Installs SIGINT/SIGTERM handlers that call [`ShutdownHandle::initiate`].
///
/// Installed by [`crate::Server::start`]; repeated installation is harmless.
///
/// # Errors
///
/// Returns [`crate::ServerError::Io`] if the signal streams cannot be created.
pub async fn install_signal_handlers(handle: ShutdownHandle) -> Result<()> {
    // Create both streams before spawning anything: a failure leaves no
    // half-installed handlers behind.
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;

    let interrupt_handle = handle.clone();
    tokio::spawn(async move {
        if interrupt.recv().await.is_some() {
            tracing::info!("received SIGINT; initiating shutdown");
            interrupt_handle.initiate();
        }
    });

    tokio::spawn(async move {
        if terminate.recv().await.is_some() {
            tracing::info!("received SIGTERM; initiating shutdown");
            handle.initiate();
        }
    });

    Ok(())
}

/// Removes the socket file if present, ignoring "not found".
pub fn remove_socket_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(path = %path.display(), "removed socket file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "failed to remove socket file");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Generous bound: a correct implementation resolves at once, a broken one
    /// must fail the test instead of hanging it.
    const TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn initiate_returns_true_exactly_once() {
        let handle = ShutdownHandle::new();
        assert!(!handle.is_shutting_down());

        assert!(handle.initiate(), "the first initiate starts shutdown");
        assert!(handle.is_shutting_down());

        assert!(
            !handle.initiate(),
            "a second initiate must report it was already flagged"
        );
        assert!(handle.is_shutting_down());
        assert!(!handle.initiate());
    }

    #[test]
    fn default_is_running() {
        let handle = ShutdownHandle::default();
        assert!(!handle.is_shutting_down());
        assert!(handle.initiate());
    }

    #[test]
    fn clones_share_one_token() {
        let handle = ShutdownHandle::new();
        let clone = handle.clone();

        assert!(clone.initiate());
        assert!(handle.is_shutting_down(), "clones observe the same flag");
        assert!(!handle.initiate());
    }

    #[tokio::test]
    async fn cancelled_resolves_immediately_when_already_flagged() {
        let handle = ShutdownHandle::new();
        assert!(handle.initiate());

        tokio::time::timeout(TIMEOUT, handle.cancelled())
            .await
            .expect("cancelled() must resolve immediately for a flagged handle");
    }

    #[tokio::test]
    async fn cancelled_resolves_after_concurrent_initiate() {
        let handle = ShutdownHandle::new();
        let trigger = handle.clone();

        let task = tokio::spawn(async move { trigger.initiate() });

        tokio::time::timeout(TIMEOUT, handle.cancelled())
            .await
            .expect("cancelled() must resolve once another task flags shutdown");

        assert!(task.await.expect("initiate task must not panic"));
        assert!(handle.is_shutting_down());
    }

    #[tokio::test]
    async fn cancelled_resolves_for_every_clone() {
        let handle = ShutdownHandle::new();
        let waiter = handle.clone();
        let waiting = tokio::spawn(async move { waiter.cancelled().await });

        handle.initiate();

        tokio::time::timeout(TIMEOUT, waiting)
            .await
            .expect("a clone must observe the flag set by the original")
            .expect("waiter task must not panic");
    }

    #[test]
    fn remove_socket_file_removes_an_existing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("adesk.sock");
        std::fs::write(&path, b"stale").expect("create socket stand-in");
        assert!(path.exists());

        remove_socket_file(&path);

        assert!(!path.exists(), "the socket file must be gone");
    }

    #[test]
    fn remove_socket_file_ignores_a_missing_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("absent.sock");

        // Must be a silent no-op, never a panic.
        remove_socket_file(&path);

        assert!(!path.exists());
    }

    #[test]
    fn remove_socket_file_warns_but_does_not_panic_on_other_errors() {
        let dir = tempfile::tempdir().expect("temp dir");

        // A directory is not removable by `remove_file`: the non-`NotFound`
        // branch must only warn.
        remove_socket_file(dir.path());

        assert!(dir.path().exists(), "the directory must be left alone");
    }

    #[tokio::test]
    async fn install_signal_handlers_succeeds_and_does_not_flag_shutdown() {
        let handle = ShutdownHandle::new();

        install_signal_handlers(handle.clone())
            .await
            .expect("installing SIGINT/SIGTERM handlers must succeed");

        assert!(
            !handle.is_shutting_down(),
            "installing handlers must not initiate shutdown"
        );

        // Repeated installation is harmless (tests never raise signals).
        install_signal_handlers(handle.clone())
            .await
            .expect("re-installing handlers must succeed");
        assert!(!handle.is_shutting_down());
    }

    // `run()` is deliberately not unit-tested: it needs a live compositor and a
    // fully composed `ServerContext`. It is covered at E2E level by
    // `crates/adesk-server/tests/` (shutdown removes the socket file, `wait()`
    // resolves, a second `shutdown()` stays idempotent).
}
