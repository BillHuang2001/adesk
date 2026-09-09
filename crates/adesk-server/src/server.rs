//! [`Server::start`] and the [`RunningServer`] handle.

use std::path::Path;
use std::sync::Arc;

use adesk_app_registry::AppRegistry;
use adesk_compositor::CompositorHandle;
use adesk_observer::ObserverService;

use crate::config::ServerConfig;
use crate::context::ServerContext;
use crate::error::{Result, ServerError};
use crate::shutdown::ShutdownHandle;

/// Runtime entry point.
pub struct Server;

impl Server {
    /// Starts the runtime and returns once the socket accepts connections.
    ///
    /// Order (`docs/architecture.md` §9):
    ///
    /// 1. `adesk_compositor::spawn` + `CompositorHandle::wait_ready`
    /// 2. build the app registry (`scan()` on a blocking task) and correlator
    /// 3. bind the Unix socket (stale file replaced, live socket refused)
    /// 4. spawn the event pump (observer + fan-out + `QueryState` resync)
    /// 5. spawn the accept loop; install SIGINT/SIGTERM handlers
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Compositor`] if the compositor cannot start,
    /// [`ServerError::Io`] if the socket cannot be bound and
    /// [`ServerError::Registry`] if the registry cannot be built.
    pub async fn start(config: ServerConfig) -> Result<RunningServer, ServerError> {
        todo!()
    }
}

/// Handle to a running runtime; cheap to clone.
///
/// Dropping the handle does **not** stop the runtime — call
/// [`RunningServer::shutdown`] or send SIGINT/SIGTERM.
#[derive(Clone)]
pub struct RunningServer {
    inner: Arc<RunningInner>,
}

/// Prints only the socket path: the handle's internals (compositor handle,
/// observer, background tasks) are opaque runtime state, so the output stays
/// small and stable. `adesk-testkit::TestRuntime` derives `Debug` and needs
/// this impl.
impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("socket_path", &self.socket_path())
            .finish_non_exhaustive()
    }
}

struct RunningInner {
    config: Arc<ServerConfig>,
    compositor: CompositorHandle,
    observer: ObserverService,
    registry: Arc<AppRegistry>,
    context: ServerContext,
    shutdown: ShutdownHandle,
    done: tokio::sync::watch::Receiver<bool>,
}

impl RunningServer {
    /// The socket the server is listening on.
    pub fn socket_path(&self) -> &Path {
        &self.inner.config.socket_path
    }

    /// Handle to the compositor thread.
    pub fn compositor(&self) -> &CompositorHandle {
        &self.inner.compositor
    }

    /// The observer fed by the event pump.
    pub fn observer(&self) -> &ObserverService {
        &self.inner.observer
    }

    /// The application registry.
    pub fn registry(&self) -> &Arc<AppRegistry> {
        &self.inner.registry
    }

    /// The shared server context (advanced use and `adesk-testkit`).
    pub fn context(&self) -> &ServerContext {
        &self.inner.context
    }

    /// The shutdown token, for callers that coordinate their own teardown.
    pub fn shutdown_handle(&self) -> &ShutdownHandle {
        &self.inner.shutdown
    }

    /// Resolves when the runtime stops serving.
    ///
    /// Returns `Ok(())` after a clean shutdown and [`ServerError::Internal`] if
    /// a background task failed.
    pub async fn wait(&self) -> Result<(), ServerError> {
        todo!()
    }

    /// Initiates and awaits a clean shutdown; idempotent.
    ///
    /// Stops accepting, fails in-flight requests with `shutting_down`, drops the
    /// Wayland display, removes the socket file.
    pub async fn shutdown(&self) -> Result<(), ServerError> {
        todo!()
    }
}
