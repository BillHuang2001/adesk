//! [`Server::start`] and the [`RunningServer`] handle.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use adesk_app_registry::{AppRegistry, Clock, Correlator, MonotonicClock, RegistryOptions};
use adesk_compositor::CompositorHandle;
use adesk_observer::ObserverService;

use crate::config::ServerConfig;
use crate::connection::Connection;
use crate::context::ServerContext;
use crate::error::{Result, ServerError};
use crate::shutdown::ShutdownHandle;
use crate::socket::SocketListener;

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
    /// 5. bind the viewer (VAP v1) endpoint when it is enabled
    /// 6. spawn the accept loop; install SIGINT/SIGTERM handlers
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Compositor`] if the compositor cannot start,
    /// [`ServerError::Io`] if the AGP or viewer socket cannot be bound and
    /// [`ServerError::Registry`] if the registry cannot be built.
    pub async fn start(config: ServerConfig) -> Result<RunningServer, ServerError> {
        let config = Arc::new(config);

        // 1. Compositor thread (sync spawn) + readiness.
        let compositor = adesk_compositor::spawn(config.compositor.clone())?;
        let ready = compositor.wait_ready().await?;
        tracing::info!(
            display = %ready.display_name,
            renderer = ready.renderer.as_str(),
            width = ready.output_size.w,
            height = ready.output_size.h,
            "compositor ready"
        );

        // 2. Application registry and correlator over one shared clock.
        let clock: Arc<dyn Clock> = Arc::new(MonotonicClock::new());
        let options = match config.app_dirs.as_ref() {
            Some(dirs) => RegistryOptions::with_search_dirs(dirs.clone()),
            None => RegistryOptions::xdg(),
        }
        .with_clock(Arc::clone(&clock));
        let registry = Arc::new(AppRegistry::with_options(options));
        let scan_registry = Arc::clone(&registry);
        let report = tokio::task::spawn_blocking(move || scan_registry.scan()).await??;
        tracing::info!(
            apps = report.apps,
            dirs = report.dirs_scanned,
            skipped = report.skipped,
            "application registry scanned"
        );
        let correlator = Arc::new(Mutex::new(Correlator::new(clock)));

        // 3. Bind the AGP socket (stale file replaced, live socket refused).
        let listener = SocketListener::bind(&config.socket_path).await?;
        tracing::info!(socket = %listener.path().display(), "AGP socket bound");

        // 4. Shared context + event pump (observer, fan-out, resync).
        let observer = ObserverService::new();
        let context = ServerContext::new(
            Arc::clone(&config),
            compositor.clone(),
            observer.clone(),
            Arc::clone(&registry),
            Arc::clone(&correlator),
        );
        let _pump = crate::event_pump::spawn(context.clone());

        // 5. Viewer endpoint (VAP v1): bound before returning, so a returned
        //    `RunningServer` means the viewer socket accepts connections too.
        let viewer_socket_path = crate::viewer::start(&config, &context).await?;

        // 6. Signal handlers + accept loop.
        crate::shutdown::install_signal_handlers(context.shutdown.clone()).await?;
        let (done_tx, done_rx) = watch::channel(false);
        spawn_accept_loop(listener, context.clone(), done_tx);
        tracing::info!("adesk-server serving");

        let shutdown = context.shutdown.clone();
        Ok(RunningServer {
            inner: Arc::new(RunningInner {
                config,
                compositor,
                observer,
                registry,
                shutdown,
                done: done_rx,
                context,
                viewer_socket_path,
            }),
        })
    }
}

/// Accepts connections until shutdown, then runs the ordered teardown.
///
/// This task owns the listener and is the only place that runs
/// [`crate::shutdown::run`]: it is woken both by [`ShutdownHandle::initiate`]
/// (signal handlers or [`RunningServer::shutdown`]) and by accept failures. The
/// `done` watch resolves only after the teardown completed, so
/// [`RunningServer::wait`] never returns while the socket file still exists.
fn spawn_accept_loop(
    listener: SocketListener,
    context: ServerContext,
    done: watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = context.shutdown.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let connection = Connection::new(stream, context.clone());
                        tokio::spawn(async move {
                            if let Err(error) = connection.run().await {
                                tracing::debug!(%error, "connection ended with an error");
                            }
                        });
                    }
                    Err(error) => {
                        if context.shutdown.is_shutting_down() {
                            break;
                        }
                        tracing::warn!(%error, "accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                },
            }
        }
        tracing::debug!("accept loop stopped; running ordered shutdown");
        if let Err(error) = crate::shutdown::run(&context).await {
            tracing::error!(%error, "ordered shutdown failed");
        }
        let _ = done.send(true);
    })
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
    viewer_socket_path: Option<PathBuf>,
}

impl RunningServer {
    /// The socket the server is listening on.
    pub fn socket_path(&self) -> &Path {
        &self.inner.config.socket_path
    }

    /// The viewer (VAP v1) Unix socket this runtime bound, if the endpoint is
    /// enabled.
    pub fn viewer_socket_path(&self) -> Option<&Path> {
        self.inner.viewer_socket_path.as_deref()
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
        let mut done = self.inner.done.clone();
        while !*done.borrow_and_update() {
            done.changed().await.map_err(|_| {
                ServerError::Internal("runtime stopped before shutdown completed".to_owned())
            })?;
        }
        Ok(())
    }

    /// Initiates and awaits a clean shutdown; idempotent.
    ///
    /// Stops accepting, fails in-flight requests with `shutting_down`, drops the
    /// Wayland display, removes the socket file.
    pub async fn shutdown(&self) -> Result<(), ServerError> {
        self.inner.shutdown.initiate();
        self.wait().await
    }
}
