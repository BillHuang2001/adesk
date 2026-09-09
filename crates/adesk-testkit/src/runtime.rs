//! In-process runtime: one compositor thread + one AGP server on private temp paths.
//!
//! [`TestRuntime`] is the entry point of every end-to-end ADesk test. It owns a
//! [`TestEnv`] (temp `XDG_RUNTIME_DIR`, temp data dirs, unique Wayland display name and
//! AGP socket), starts `adesk-server` on it and exposes the runtime's handles back to the
//! test: the compositor handle, the observer, the app registry, an AGP client, an event
//! tap and the Wayland display name.
//!
//! ## `adesk-server` contract this module is designed against
//!
//! `adesk-server` is the only crate the testkit cannot validate locally; the glue in
//! [`TestRuntime::start_with`] assumes exactly:
//!
//! ```text
//! ServerConfig::new(socket_path: impl Into<PathBuf>, compositor: CompositorConfig) -> ServerConfig
//! ServerConfig::with_app_dirs(self, dirs: Vec<PathBuf>) -> ServerConfig      // XDG_DATA_DIRS-style share roots
//! Server::start(ServerConfig) -> impl Future<Output = Result<RunningServer, _>>   // server spawns the compositor
//! RunningServer::socket_path(&self) -> &Path
//! RunningServer::compositor(&self) -> &CompositorHandle
//! RunningServer::observer(&self) -> &ObserverService
//! RunningServer::registry(&self) -> &Arc<AppRegistry>          // derefs to &AppRegistry
//! RunningServer::shutdown(self) -> impl Future<Output = Result<(), _>>
//! ```
//!
//! If the landed server differs, adapt only [`TestRuntime::start_with`] and
//! [`TestRuntime::shutdown`]; the rest of the harness is independent of it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use adesk_app_registry::AppRegistry;
use adesk_client::{CaptureRequest, Client};
use adesk_compositor::{CompositorConfig, CompositorHandle, RendererKind, RuntimeCommand};
use adesk_core::{ImageBuffer, Rect, RuntimeEvent, Size, WindowId};
use adesk_observer::ObserverService;
use adesk_server::{RunningServer, Server, ServerConfig};
use tokio::sync::broadcast;

use crate::env::{EnvScope, TestEnv};
use crate::error::{Result, TestkitError};
use crate::fixtures::FixtureDir;
use crate::wayland::WaylandTestClient;

/// Protocol default output size used by every test runtime unless overridden.
pub const DEFAULT_OUTPUT_SIZE: Size = Size { w: 1280, h: 800 };

/// Default bound for graceful shutdown ([`TestRuntime::shutdown`]).
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Default compositor event broadcast capacity (`docs/architecture.md` §1 requires ≥ 4096).
pub const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 4096;

/// The rect a mapped window is tiled to, straight from the `adesk-wm` policy.
///
/// Tests must never hard-code `1280x800`; this keeps the expected geometry single-sourced
/// with the runtime's actual policy.
pub fn expected_window_geometry(output_size: Size) -> Rect {
    adesk_wm::PolicyConfig::new(output_size).tiled_rect()
}

/// Configuration of a [`TestRuntime`].
///
/// Defaults are deterministic and isolated: `1280x800`, pixman, no app dirs beyond the
/// env's own fixture dir, 4096-slot event broadcast, 5 s shutdown bound, process env
/// scoped to the runtime.
#[derive(Debug, Clone)]
pub struct TestRuntimeConfig {
    /// Virtual output size (default [`DEFAULT_OUTPUT_SIZE`]).
    pub output_size: Size,
    /// Renderer (default [`RendererKind::Pixman`]; use [`crate::test_renderer`] for GL opt-in).
    pub renderer: RendererKind,
    /// Registry search dirs (XDG_DATA_DIRS-style share roots). Empty means "the runtime's
    /// isolated fixture dir only" — the host's `/usr/share` is never scanned.
    pub app_dirs: Vec<PathBuf>,
    /// Event broadcast capacity (default [`DEFAULT_EVENT_CHANNEL_CAPACITY`]).
    pub event_channel_capacity: usize,
    /// Bound for [`TestRuntime::shutdown`] (default [`DEFAULT_SHUTDOWN_TIMEOUT`]).
    pub shutdown_timeout: Duration,
    /// Wayland socket name override; `None` pins the env's unique display name (the
    /// runtime always names the socket explicitly so [`TestRuntime::wayland_display`]
    /// matches what the compositor bound).
    pub socket_name: Option<String>,
    /// Whether to apply [`TestEnv::apply`] for the runtime's lifetime (default `true`).
    /// Required for app-registry launch tests; see the [`crate::env`] hazard note.
    pub apply_env: bool,
}

impl Default for TestRuntimeConfig {
    fn default() -> Self {
        TestRuntimeConfig {
            output_size: DEFAULT_OUTPUT_SIZE,
            renderer: RendererKind::Pixman,
            app_dirs: Vec::new(),
            event_channel_capacity: DEFAULT_EVENT_CHANNEL_CAPACITY,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            socket_name: None,
            apply_env: true,
        }
    }
}

impl TestRuntimeConfig {
    /// Default configuration.
    pub fn new() -> TestRuntimeConfig {
        TestRuntimeConfig::default()
    }

    /// Overrides the virtual output size.
    pub fn with_output_size(mut self, size: Size) -> TestRuntimeConfig {
        self.output_size = size;
        self
    }

    /// Overrides the renderer.
    pub fn with_renderer(mut self, renderer: RendererKind) -> TestRuntimeConfig {
        self.renderer = renderer;
        self
    }

    /// Replaces the registry search dirs.
    pub fn with_app_dirs(mut self, dirs: Vec<PathBuf>) -> TestRuntimeConfig {
        self.app_dirs = dirs;
        self
    }

    /// Appends a [`FixtureDir`]'s share root to the registry search dirs.
    pub fn with_fixture_dir(mut self, dir: &FixtureDir) -> TestRuntimeConfig {
        self.app_dirs.push(dir.search_dir().to_path_buf());
        self
    }

    /// Overrides the event broadcast capacity.
    pub fn with_event_channel_capacity(mut self, capacity: usize) -> TestRuntimeConfig {
        self.event_channel_capacity = capacity;
        self
    }

    /// Overrides the shutdown bound.
    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> TestRuntimeConfig {
        self.shutdown_timeout = timeout;
        self
    }

    /// Overrides the Wayland socket name.
    pub fn with_socket_name(mut self, name: impl Into<String>) -> TestRuntimeConfig {
        self.socket_name = Some(name.into());
        self
    }

    /// Enables or disables process-env scoping.
    pub fn with_apply_env(mut self, apply: bool) -> TestRuntimeConfig {
        self.apply_env = apply;
        self
    }
}

/// A running in-process ADesk runtime.
///
/// Create with [`TestRuntime::start`] / [`TestRuntime::start_with`]; stop gracefully with
/// [`TestRuntime::shutdown`]. Dropping without calling `shutdown` is safe and bounded (see
/// the crate docs), but `shutdown().await` is the only way to observe shutdown errors.
#[derive(Debug)]
pub struct TestRuntime {
    config: TestRuntimeConfig,
    env: TestEnv,
    env_scope: Option<EnvScope>,
    running: Option<RunningServer>,
    socket_path: PathBuf,
    display_name: String,
}

impl TestRuntime {
    /// Starts a runtime with [`TestRuntimeConfig::default`].
    pub async fn start() -> Result<TestRuntime> {
        TestRuntime::start_with(TestRuntimeConfig::default()).await
    }

    /// Starts a runtime with the given configuration.
    ///
    /// Steps: create [`TestEnv`] → apply the process env (unless disabled) → build
    /// `CompositorConfig` → build `ServerConfig` → `Server::start` (the server spawns the
    /// compositor thread and binds the AGP socket).
    pub async fn start_with(config: TestRuntimeConfig) -> Result<TestRuntime> {
        let env = TestEnv::new()?;
        let env_scope = if config.apply_env {
            Some(env.apply())
        } else {
            None
        };

        let mut compositor = CompositorConfig::new()
            .with_output_size(config.output_size)
            .with_renderer(config.renderer)
            .with_event_channel_capacity(config.event_channel_capacity);
        // Pin the socket name to the env's display name: smithay's auto-naming would
        // otherwise bind `wayland-1..32`, and `wayland_display()` must always name the
        // socket the compositor actually bound.
        let socket_name = config
            .socket_name
            .clone()
            .unwrap_or_else(|| env.wayland_display().to_string());
        compositor = compositor.with_socket_name(socket_name);

        // An empty app_dirs list means "isolated fixture dir only", never the host's XDG dirs.
        let app_dirs = if config.app_dirs.is_empty() {
            env.app_dirs()
        } else {
            config.app_dirs.clone()
        };

        let server_config =
            ServerConfig::new(env.agp_socket().to_path_buf(), compositor).with_app_dirs(app_dirs);

        let running = Server::start(server_config)
            .await
            .map_err(|e| TestkitError::Startup(e.to_string()))?;

        let socket_path = running.socket_path().to_path_buf();
        let display_name = env.wayland_display().to_string();

        Ok(TestRuntime {
            config,
            env,
            env_scope,
            running: Some(running),
            socket_path,
            display_name,
        })
    }

    /// Absolute path of the AGP Unix socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The Wayland socket name to pass to clients (`WAYLAND_DISPLAY`).
    pub fn wayland_display(&self) -> &str {
        &self.display_name
    }

    /// Virtual output size.
    pub fn output_size(&self) -> Size {
        self.config.output_size
    }

    /// The rect windows are tiled to ([`expected_window_geometry`]).
    pub fn tiled_rect(&self) -> Rect {
        expected_window_geometry(self.config.output_size)
    }

    /// The compositor handle.
    ///
    /// Panics if the runtime has been shut down (a harness invariant: tests never touch a
    /// runtime after `shutdown()`).
    pub fn compositor(&self) -> &CompositorHandle {
        self.running().compositor()
    }

    /// The observer service.
    ///
    /// Panics if the runtime has been shut down.
    pub fn observer(&self) -> &ObserverService {
        self.running().observer()
    }

    /// The app registry the runtime scanned at startup.
    ///
    /// Panics if the runtime has been shut down.
    pub fn registry(&self) -> &AppRegistry {
        self.running().registry()
    }

    /// The private environment this runtime owns.
    pub fn env(&self) -> &TestEnv {
        &self.env
    }

    /// The configuration this runtime was started with.
    pub fn config(&self) -> &TestRuntimeConfig {
        &self.config
    }

    /// Whether [`TestRuntime::shutdown`] has already run.
    pub fn is_shutdown(&self) -> bool {
        self.running.is_none()
    }

    /// Opens a fresh AGP client connected to this runtime.
    pub async fn client(&self) -> Result<Client> {
        Ok(Client::connect(&self.socket_path).await?)
    }

    /// Subscribes to the compositor's `RuntimeEvent` broadcast.
    ///
    /// Subscribe *before* triggering the action under test; events are not replayed.
    pub fn event_tap(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.running().compositor().subscribe()
    }

    /// Connects a [`WaylandTestClient`] to this runtime's Wayland socket.
    pub fn wayland_client(&self) -> Result<WaylandTestClient> {
        WaylandTestClient::connect_in(self.env.runtime_dir(), &self.display_name)
    }

    /// Renders `window_id` at its natural size and returns the pixels.
    ///
    /// Convenience over `capture_window` (RGBA8, no scaling) for tests that only need an
    /// [`ImageBuffer`]; opens a short-lived AGP connection.
    pub async fn capture(&self, window_id: WindowId) -> Result<ImageBuffer> {
        let client = self.client().await?;
        let result = client
            .capture_window(CaptureRequest::window(window_id))
            .await?;
        Ok(result.decode_image()?)
    }

    /// Waits for the next `WindowCreated` event and returns its id.
    pub async fn wait_for_window(&self, timeout: Duration) -> Result<WindowId> {
        let _ = timeout;
        todo!("stub: implementation phase — EventAssert over self.event_tap()")
    }

    /// Waits for the next `WindowCreated` event whose `app_id` matches.
    pub async fn wait_for_window_app(
        &self,
        app_id: &adesk_core::AppId,
        timeout: Duration,
    ) -> Result<WindowId> {
        let _ = (app_id, timeout);
        todo!("stub: implementation phase — EventAssert over self.event_tap()")
    }

    /// Gracefully stops the runtime within the configured shutdown bound.
    ///
    /// Returns [`TestkitError::ShutdownTimeout`] if the server does not stop in time, and
    /// [`TestkitError::Shutdown`] if it reports an error. Idempotent: shutting down twice
    /// returns `Ok(())`.
    pub async fn shutdown(mut self) -> Result<()> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        let timeout = self.config.shutdown_timeout;
        let outcome = tokio::time::timeout(timeout, running.shutdown()).await;
        // Restore the process env as soon as the runtime is gone.
        self.env_scope.take();
        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(TestkitError::Shutdown(e.to_string())),
            Err(_) => Err(TestkitError::ShutdownTimeout { timeout }),
        }
    }

    fn running(&self) -> &RunningServer {
        self.running
            .as_ref()
            .expect("TestRuntime used after shutdown()")
    }
}

impl Drop for TestRuntime {
    /// Best-effort, non-blocking teardown; never panics and never hangs.
    ///
    /// Order matters:
    /// 1. Ask the compositor to stop through its command channel (synchronous,
    ///    non-blocking) so its thread exits and unlinks the Wayland socket even when the
    ///    async server teardown never gets to run.
    /// 2. If a tokio runtime is currently driving the test, spawn the graceful server
    ///    shutdown as a detached task with the configured bound. If not (test already
    ///    returned), the process teardown reclaims the rest.
    /// 3. Restore the process environment.
    ///
    /// `Drop` deliberately does not block: blocking inside a `#[tokio::test]` body would
    /// deadlock the current-thread executor. Use [`TestRuntime::shutdown`] when the test
    /// must observe a clean, awaited shutdown.
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let (reply, _rx) = tokio::sync::oneshot::channel();
            let _ = running.compositor().send(RuntimeCommand::Shutdown { reply });
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let timeout = self.config.shutdown_timeout;
                handle.spawn(async move {
                    let _ = tokio::time::timeout(timeout, running.shutdown()).await;
                });
            }
        }
        self.env_scope.take();
    }
}
