#![allow(dead_code)] // each suite uses only a subset of the harness helpers

//! Shared E2E harness for the `adesk-server` AGP suites.
//!
//! **Not a test target**: every suite declares `mod common;` and drives one
//! [`TestRuntime`] — a real `adesk-server` runtime (compositor thread, observer,
//! registry and AGP socket) on a private temp socket, plus the typed
//! [`adesk_client::Client`] and a raw NDJSON [`RawClient`] for the cases the SDK
//! deliberately cannot express (unknown methods, malformed lines, event frames).
//! No display, GPU, network or installed application is required: the runtime
//! always uses the pixman renderer and an empty app-dir list.
//!
//! ## Environment guard: a private `XDG_RUNTIME_DIR`
//!
//! The compositor binds its Wayland socket inside `$XDG_RUNTIME_DIR`, and the
//! ambient runtime dir is not always writable (read-only sandboxes, CI
//! containers), which Smithay reports as `CompositorError::Socket`. So
//! [`TestRuntime::start`] first points the process at a private `0700`
//! directory under `std::env::temp_dir()` and restores the previous value when
//! the runtime drops — the same pattern as
//! `crates/adesk-compositor/tests/compositor_smoke.rs`.
//!
//! `XDG_RUNTIME_DIR` is process-global, so the guard holds a process-wide lock
//! ([`ENV_LOCK`]) for the **whole runtime lifetime**, including the whole test
//! body: two runtimes in one test binary can never race `set_var`. Tests in
//! *different* files are separate processes and stay fully parallel. The
//! consequence is that a test must never start two [`TestRuntime`]s at once —
//! the second start would wait for the first runtime to be dropped, so the
//! harness bounds that wait and panics with an explanatory message instead of
//! hanging.
//!
//! ## Sync test bodies: the runtime owns the tokio runtime
//!
//! Suites use plain `#[test]` functions, not `#[tokio::test]`. The guard owns a
//! dedicated multi-thread tokio runtime and exposes
//! [`TestRuntime::block_on`] / [`TestRuntime::block_on_timeout`]. That buys two
//! things `#[tokio::test]` cannot give:
//!
//! 1. **Cleanup always runs.** `#[tokio::test]` builds the runtime inside the
//!    test body, so a panicking assertion unwinds *past* the runtime's own
//!    drop glue and any teardown expressed as ordinary code — the compositor
//!    thread and the socket file would leak. Here the cleanup lives in
//!    [`Drop for TestRuntime`], which runs during unwinding too, shuts the
//!    runtime down under a timeout and never fails the test a second time.
//! 2. **No `await`-holding-lock lint.** The process-wide env lock is a `std`
//!    mutex guard held across the test body; blocking the thread with
//!    `block_on` is exactly the intended shape, and no task can deadlock on it.
//!
//! `futures` is intentionally not used: [`RawClient`] reads NDJSON lines with
//! `tokio::io::AsyncBufReadExt` and never needs a `Stream` combinator.

use std::ffi::OsString;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use adesk_client::{Client, ClientError};
use adesk_compositor::{CompositorConfig, RendererKind};
use adesk_core::{ErrorCode, Size};
use adesk_server::{RunningServer, Server, ServerConfig, ServerContext, ServerError, ViewerConfig};
use adesk_viewer::{ViewerClient, ViewerTarget};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// Width of the virtual output every test runtime is started with.
pub const OUTPUT_WIDTH: u32 = 1280;

/// Height of the virtual output every test runtime is started with.
pub const OUTPUT_HEIGHT: u32 = 720;

/// Generous outer bound for any single client call.
///
/// A correct runtime answers in milliseconds; this only exists so a broken one
/// fails the test with a clear message instead of hanging it forever. It is
/// *not* an assertion about timing — protocol-level timeouts (`timeout_ms`,
/// `quiet_ms`) are asserted on the returned `Observation`, never on this bound.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Short protocol `timeout_ms` (milliseconds) for waits whose exact value is
/// immaterial: waits that only need to *expire* against an idle runtime, and
/// `observe(until = timeout)` samples whose horizon is irrelevant.
///
/// Kept strictly below the protocol default `quiet_ms` (250, §5.4) so a
/// `wait_for_quiet` carrying this timeout still expires, and short enough to
/// keep the suites fast. `observe(until = timeout)` resolves *at* this horizon
/// by definition, so a test asserting `elapsed_ms >= <horizon>` compares against
/// this constant.
pub const SHORT_TIMEOUT_MS: u64 = 100;

/// The virtual output size as the `adesk-core` value type.
pub fn output_size() -> Size {
    Size::new(OUTPUT_WIDTH, OUTPUT_HEIGHT)
}

/// Serializes every [`TestRuntime`] in this test binary: they all mutate the
/// process-global `XDG_RUNTIME_DIR`, which the compositor reads when it binds
/// its Wayland socket.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Keeps temp runtime-dir names unique across tests and repeated runs of a pid.
static NEXT_RUNTIME_DIR: AtomicU64 = AtomicU64::new(1);

/// How long a start may wait for the process-wide environment lock before the
/// harness concludes that the test itself started a second runtime.
const ENV_LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// How long [`Drop for TestRuntime`] waits for a clean shutdown. Cleanup must
/// never hang a test run, so a runtime that refuses to stop is abandoned.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// A private writable `XDG_RUNTIME_DIR` plus the process-wide lock that makes
/// mutating it safe, both released when the guard drops.
///
/// Created under `std::env::temp_dir()` (which honours `$TMPDIR`) with mode
/// `0700`, because Wayland requires the runtime dir to be private to the user.
struct RuntimeDir {
    /// The temp dir; also the parent of the AGP socket by default.
    path: PathBuf,
    /// The `XDG_RUNTIME_DIR` value to restore, or `None` if it was unset.
    previous: Option<OsString>,
    /// Declared last so it is dropped last: the environment is restored and the
    /// directory removed before the next runtime may touch the environment.
    _lock: MutexGuard<'static, ()>,
}

impl RuntimeDir {
    /// Creates `<temp>/adesk-server-e2e-<pid>-<n>`, sets it as
    /// `XDG_RUNTIME_DIR` and remembers the previous value.
    fn new(lock: MutexGuard<'static, ()>) -> RuntimeDir {
        let path = std::env::temp_dir().join(format!(
            "adesk-server-e2e-{}-{}",
            std::process::id(),
            NEXT_RUNTIME_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("E2E harness: create temp runtime dir");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("E2E harness: temp runtime dir is private (0700)");
        let previous = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", &path);
        RuntimeDir {
            path,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        // Restore the ambient runtime dir first, then drop our directory. The
        // runtime has already been shut down and joined by `TestRuntime::drop`,
        // so cleanup is only best effort and must never fail a test.
        match self.previous.take() {
            Some(previous) => std::env::set_var("XDG_RUNTIME_DIR", previous),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Serializes one runtime and gives it a writable `XDG_RUNTIME_DIR`.
///
/// Poisoning is tolerated: a failing test has already reported its own
/// assertion/panic, and the remaining tests must still run instead of aborting
/// with a `PoisonError`. The wait is bounded so that starting two runtimes in
/// one test fails loudly instead of deadlocking the test binary.
fn isolated_env() -> RuntimeDir {
    let deadline = Instant::now() + ENV_LOCK_TIMEOUT;
    let lock = loop {
        match ENV_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                assert!(
                    Instant::now() < deadline,
                    "E2E harness: waited {ENV_LOCK_TIMEOUT:?} for the process-wide \
                     `XDG_RUNTIME_DIR` lock. Another `TestRuntime` in this test binary holds it \
                     for its whole lifetime — do not start two runtimes in one test."
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    RuntimeDir::new(lock)
}

/// Chainable pre-start configuration for a [`TestRuntime`]'s viewer (VAP v1)
/// endpoint.
///
/// A runtime binds its viewer at start, so the viewer must be chosen *before*
/// [`TestRuntimeBuilder::start`] runs. The builders mirror
/// [`adesk_server::ServerConfig`]'s own `with_viewer*` methods; the baseline
/// ([`ViewerConfig::default`]) enables the endpoint at the AGP socket's sibling
/// path, so a plain [`TestRuntime::start`] already serves VAP.
#[derive(Debug, Clone, Default)]
pub struct TestRuntimeBuilder {
    /// The viewer endpoint the started runtime will bind.
    viewer: ViewerConfig,
}

impl TestRuntimeBuilder {
    /// Replaces the viewer endpoint configuration wholesale.
    pub fn with_viewer(mut self, viewer: ViewerConfig) -> TestRuntimeBuilder {
        self.viewer = viewer;
        self
    }

    /// Enables the viewer endpoint on an explicit Unix socket path.
    pub fn with_viewer_socket(mut self, path: impl Into<PathBuf>) -> TestRuntimeBuilder {
        self.viewer.socket_path = Some(path.into());
        self.viewer.enabled = true;
        self
    }

    /// Disables the viewer endpoint entirely.
    pub fn without_viewer(mut self) -> TestRuntimeBuilder {
        self.viewer.enabled = false;
        self
    }

    /// Starts the baseline runtime with this viewer configuration applied.
    ///
    /// # Panics
    ///
    /// Panics exactly like [`TestRuntime::start_with`] when the runtime cannot
    /// start, including a viewer socket that cannot be bound (e.g. a live socket
    /// already occupying the explicit path).
    pub fn start(self) -> TestRuntime {
        TestRuntime::start_with(move |config| config.with_viewer(self.viewer))
    }
}

/// A live `adesk-server` runtime plus everything a test needs to drive it.
///
/// Created with [`TestRuntime::start`] (pixman, 1280x720, no app dirs),
/// [`TestRuntime::start_with`] (override any part of that baseline),
/// [`TestRuntime::start_with_app_dirs`] (fixture `.desktop` dirs) or a
/// [`TestRuntimeBuilder`] from [`TestRuntime::builder`] /
/// [`TestRuntime::without_viewer`] (configure the viewer endpoint first).
/// Dropping the guard shuts the runtime down, restores `XDG_RUNTIME_DIR` and
/// removes the temp dir — even when the test panics.
pub struct TestRuntime {
    /// Owns the async executor; every `block_on*` helper drives it.
    runtime: tokio::runtime::Runtime,
    /// The live server handle (`Debug` prints only the socket path).
    running: RunningServer,
    /// The AGP socket path this runtime bound.
    socket_path: PathBuf,
    /// Whether a clean shutdown already completed.
    shutdown_done: bool,
    /// The environment guard, declared last so it drops (and unlocks) last.
    env: RuntimeDir,
}

impl TestRuntime {
    /// Starts the baseline runtime: pixman renderer, 1280x720 output, empty
    /// app-dir list (`list_apps` sees exactly the fixture apps a test adds).
    ///
    /// Panics — never skips — if the runtime cannot start, because a suite that
    /// cannot start a runtime has nothing meaningful to assert.
    pub fn start() -> TestRuntime {
        TestRuntime::start_with(|config| config)
    }

    /// Starts the baseline runtime after applying `build` to its
    /// [`ServerConfig`] (socket path, output size, renderer, app dirs, xkb).
    ///
    /// # Panics
    ///
    /// Panics with the exact startup error, the socket path and the temp dir if
    /// the runtime fails to start.
    pub fn start_with(build: impl FnOnce(ServerConfig) -> ServerConfig) -> TestRuntime {
        let env = isolated_env();
        let socket_path = env.path.join("adesk.sock");
        let baseline = ServerConfig::new(socket_path, CompositorConfig::default())
            .with_output_size(output_size())
            .with_renderer(RendererKind::Pixman)
            .with_app_dirs(Vec::new());
        let config = build(baseline);
        let socket_path = config.socket_path.clone();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("adesk-e2e")
            .build()
            .expect("E2E harness: build the test tokio runtime");

        let running = match runtime.block_on(Server::start(config)) {
            Ok(running) => running,
            Err(error) => panic!(
                "E2E harness: Server::start failed: {error}\n  socket: {}\n  XDG_RUNTIME_DIR: {}",
                socket_path.display(),
                env.path.display()
            ),
        };

        TestRuntime {
            runtime,
            running,
            socket_path,
            shutdown_done: false,
            env,
        }
    }

    /// Starts the baseline runtime with `dirs` as the only `.desktop` search
    /// directories (used by the launch suite's fixtures).
    pub fn start_with_app_dirs(dirs: Vec<PathBuf>) -> TestRuntime {
        TestRuntime::start_with(move |config| config.with_app_dirs(dirs))
    }

    /// Chainable viewer configuration for a runtime that has not started yet:
    /// `TestRuntime::builder().with_viewer_socket(path).start()`.
    pub fn builder() -> TestRuntimeBuilder {
        TestRuntimeBuilder::default()
    }

    /// Builder shorthand for [`TestRuntimeBuilder::with_viewer`].
    pub fn with_viewer(viewer: ViewerConfig) -> TestRuntimeBuilder {
        TestRuntimeBuilder::default().with_viewer(viewer)
    }

    /// Builder shorthand for [`TestRuntimeBuilder::with_viewer_socket`].
    pub fn with_viewer_socket(path: impl Into<PathBuf>) -> TestRuntimeBuilder {
        TestRuntimeBuilder::default().with_viewer_socket(path)
    }

    /// Builder shorthand for [`TestRuntimeBuilder::without_viewer`].
    pub fn without_viewer() -> TestRuntimeBuilder {
        TestRuntimeBuilder::default().without_viewer()
    }

    /// The AGP socket the runtime listens on.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The viewer (VAP v1) Unix socket this runtime bound, or `None` when the
    /// endpoint is disabled ([`TestRuntime::without_viewer`]).
    pub fn viewer_socket_path(&self) -> Option<&Path> {
        self.running.viewer_socket_path()
    }

    /// The private `XDG_RUNTIME_DIR` this runtime owns.
    ///
    /// This is the directory the compositor's Wayland socket lives in, so it is
    /// the `runtime_dir` argument of
    /// `adesk_testkit::WaylandTestClient::connect_in`.
    pub fn runtime_dir(&self) -> &Path {
        &self.env.path
    }

    /// The compositor's Wayland display socket name (`None` before it is ready).
    ///
    /// `Server::start` awaits readiness, so this is `Some` for any started
    /// runtime; it is the `display_name` argument of
    /// `adesk_testkit::WaylandTestClient::connect_in`.
    pub fn wayland_display_name(&self) -> Option<String> {
        self.running.compositor().wayland_display_name()
    }
    /// The live server handle (compositor, observer, registry, context).
    pub fn running(&self) -> &RunningServer {
        &self.running
    }

    /// The shared [`ServerContext`]; shorthand for `running().context()`.
    pub fn context(&self) -> &ServerContext {
        self.running.context()
    }

    /// Blocks the test thread on `future` using this runtime's executor.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// [`TestRuntime::block_on`] with [`REQUEST_TIMEOUT`] as an outer bound.
    ///
    /// The timeout future is created *inside* the runtime (the timer needs a
    /// reactor context), which is why this wraps `future` in an async block
    /// rather than building the `timeout` on the test thread.
    ///
    /// # Panics
    ///
    /// Panics with the elapsed bound if the future does not resolve in time.
    pub fn block_on_timeout<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(async move {
            match tokio::time::timeout(REQUEST_TIMEOUT, future).await {
                Ok(output) => output,
                Err(_) => panic!(
                    "E2E harness: a client call did not complete within {REQUEST_TIMEOUT:?}; \
                     the runtime is not answering AGP requests"
                ),
            }
        })
    }

    /// Connects a typed [`adesk_client::Client`] to [`TestRuntime::socket_path`].
    ///
    /// # Panics
    ///
    /// Panics with the full [`ClientError`] if the connect (or its version
    /// handshake) fails.
    pub fn connect(&self) -> Client {
        let path = self.socket_path.clone();
        match self.block_on_timeout(Client::connect(&path)) {
            Ok(client) => client,
            Err(error) => panic!(
                "E2E harness: Client::connect({}) failed: {error:?}",
                path.display()
            ),
        }
    }

    /// Connects a raw NDJSON [`RawClient`] to [`TestRuntime::socket_path`].
    ///
    /// Use it for frames the typed SDK refuses to produce or consume: unknown
    /// methods, malformed lines, bare JSON, or hand-rolled event streams.
    ///
    /// # Panics
    ///
    /// Panics with the I/O error if the socket cannot be connected.
    pub fn connect_raw(&self) -> RawClient {
        let path = self.socket_path.clone();
        match self.block_on_timeout(RawClient::connect(&path)) {
            Ok(raw) => raw,
            Err(error) => panic!(
                "E2E harness: raw connect to {} failed: {error}",
                path.display()
            ),
        }
    }

    /// Connects a typed [`ViewerClient`] to the runtime's viewer (VAP v1) socket.
    ///
    /// The connect and its handshake are bounded by [`REQUEST_TIMEOUT`] through
    /// [`TestRuntime::block_on_timeout`].
    ///
    /// # Panics
    ///
    /// Panics if the viewer endpoint is disabled, or with the full
    /// `adesk_viewer::ViewerError` if the connect or handshake fails.
    pub fn connect_viewer(&self) -> ViewerClient {
        let path = self.viewer_socket_path_or_panic().to_path_buf();
        match self.block_on_timeout(ViewerClient::connect(ViewerTarget::Unix(path.clone()))) {
            Ok(client) => client,
            Err(error) => panic!(
                "E2E harness: ViewerClient::connect({}) failed: {error:?}",
                path.display()
            ),
        }
    }

    /// Connects a raw NDJSON [`RawClient`] to the viewer (VAP v1) socket.
    ///
    /// Use it to observe VAP traffic the typed client hides — an `error` frame
    /// that answers an undeliverable input, or a hand-written wire message — and
    /// to check that the connection stays open afterwards. The bytes are VAP
    /// NDJSON (`adesk_viewer_proto::encode_client` / `decode_server`), not AGP:
    /// [`RawClient`] is only a Unix-socket NDJSON pipe.
    ///
    /// # Panics
    ///
    /// Panics if the viewer endpoint is disabled or the socket cannot be
    /// connected.
    pub fn connect_viewer_raw(&self) -> RawClient {
        let path = self.viewer_socket_path_or_panic().to_path_buf();
        match self.block_on_timeout(RawClient::connect(&path)) {
            Ok(raw) => raw,
            Err(error) => panic!(
                "E2E harness: raw viewer connect to {} failed: {error}",
                path.display()
            ),
        }
    }

    /// The viewer socket path, panicking with a clear message when the endpoint
    /// is disabled.
    fn viewer_socket_path_or_panic(&self) -> &Path {
        self.viewer_socket_path().unwrap_or_else(|| {
            panic!(
                "E2E harness: the viewer (VAP v1) endpoint is disabled; start the runtime \
                 with the default viewer configuration (`TestRuntime::start`) or \
                 `TestRuntime::builder()`"
            )
        })
    }

    /// Shuts the runtime down and waits for the ordered teardown; idempotent.
    ///
    /// The handle stays usable afterwards ([`TestRuntime::running`] remains
    /// valid, and a second call returns `Ok(())` immediately), so suites can
    /// assert on the post-shutdown state (socket file gone, `wait()` resolved).
    /// On failure the runtime is left un-flagged so `Drop` retries the teardown.
    pub fn shutdown(&mut self) -> Result<(), ServerError> {
        if self.shutdown_done {
            return Ok(());
        }
        let result = self.block_on_timeout(self.running.shutdown());
        if result.is_ok() {
            self.shutdown_done = true;
        }
        result
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        // Cleanup runs during unwinding too, so a failing assertion can never
        // leak a compositor thread or a socket file. Errors and timeouts are
        // ignored: cleanup must never fail (or hang) a test a second time.
        if !self.shutdown_done {
            let runtime = &self.runtime;
            let running = &self.running;
            runtime.block_on(async move {
                let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, running.shutdown()).await;
            });
            self.shutdown_done = true;
        }
        // `env` is declared last: it restores `XDG_RUNTIME_DIR`, removes the
        // temp dir and only then releases the process-wide lock.
    }
}

/// Outcome of one read on a [`RawClient`], so callers can tell EOF from timeout.
enum ReadOutcome {
    /// A complete line arrived (terminator stripped).
    Line(String),
    /// The server closed its half (or the connection broke).
    Eof,
    /// Nothing arrived within the window.
    Timeout,
}

/// A raw NDJSON client over a `tokio::net::UnixStream`.
///
/// The typed SDK is the right tool for normal assertions, but it cannot send an
/// unknown method, a malformed line or a hand-written event frame, and its
/// event streams need `futures`. [`RawClient`] speaks the wire directly: build
/// frames with `serde_json::json!`, [`RawClient::send_json`] them and read the
/// answer with the `expect_*` helpers. Every read method records the lines it
/// saw, so a panic prints the recent traffic.
pub struct RawClient {
    /// Buffered read half; buffering keeps a read from swallowing the next line.
    reader: BufReader<OwnedReadHalf>,
    /// Write half; every `send_*` flushes.
    writer: OwnedWriteHalf,
    /// Lines received so far, for panic diagnostics.
    history: Vec<String>,
}

impl RawClient {
    /// Connect to `path` (private: use [`TestRuntime::connect_raw`]).
    async fn connect(path: &Path) -> std::io::Result<RawClient> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = stream.into_split();
        Ok(RawClient {
            reader: BufReader::new(reader),
            writer,
            history: Vec::new(),
        })
    }

    /// Writes `line` followed by `\n` and flushes.
    ///
    /// # Panics
    ///
    /// Panics if the write fails: a test that cannot send its frame cannot
    /// assert anything meaningful.
    pub async fn send_line(&mut self, line: &str) {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        if let Err(error) = self.writer.write_all(&bytes).await {
            panic!("raw client: write failed: {error}");
        }
        if let Err(error) = self.writer.flush().await {
            panic!("raw client: flush failed: {error}");
        }
    }

    /// Serializes `value` as one NDJSON line and sends it.
    pub async fn send_json(&mut self, value: &Value) {
        self.send_line(&value.to_string()).await;
    }

    /// Reads one line, returning `None` on EOF or timeout (never panics).
    pub async fn read_line(&mut self, within: Duration) -> Option<String> {
        match self.read_outcome(within).await {
            ReadOutcome::Line(line) => Some(line),
            ReadOutcome::Eof | ReadOutcome::Timeout => None,
        }
    }

    /// Reads one line and decodes it as JSON.
    ///
    /// Returns `None` on EOF, timeout, or a line that is not valid JSON (the
    /// line is still consumed and recorded for diagnostics).
    pub async fn read_json(&mut self, within: Duration) -> Option<Value> {
        let line = self.read_line(within).await?;
        serde_json::from_str(&line).ok()
    }

    /// Reads one line, panicking on EOF or timeout.
    ///
    /// # Panics
    ///
    /// Panics with the recent traffic if no line arrives in `within`.
    pub async fn expect_line(&mut self, within: Duration) -> String {
        match self.read_outcome(within).await {
            ReadOutcome::Line(line) => line,
            ReadOutcome::Eof => panic!(
                "raw client: connection closed before a line arrived\n{}",
                self.recent()
            ),
            ReadOutcome::Timeout => {
                panic!("raw client: no line within {within:?}\n{}", self.recent())
            }
        }
    }

    /// Reads one line and decodes it as JSON, panicking on EOF, timeout or a
    /// non-JSON line.
    pub async fn expect_json(&mut self, within: Duration) -> Value {
        let line = self.expect_line(within).await;
        match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => panic!("raw client: line is not JSON ({error}): {line}"),
        }
    }

    /// Reads lines until one is valid JSON for which `pred` returns `true`.
    ///
    /// Non-JSON lines and JSON lines the predicate rejects are skipped (all are
    /// recorded for diagnostics) — this is how a suite waits for its own
    /// response while events stream on the same connection.
    ///
    /// # Panics
    ///
    /// Panics with the recent traffic if the connection closes or `within`
    /// expires before a matching line arrives.
    pub async fn expect_json_matching(
        &mut self,
        within: Duration,
        mut pred: impl FnMut(&Value) -> bool,
    ) -> Value {
        let deadline = Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!(
                    "raw client: no matching line within {within:?}\n{}",
                    self.recent()
                );
            }
            match self.read_outcome(remaining).await {
                ReadOutcome::Line(line) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if pred(&value) {
                            return value;
                        }
                    }
                }
                ReadOutcome::Eof => panic!(
                    "raw client: connection closed before a matching line arrived\n{}",
                    self.recent()
                ),
                ReadOutcome::Timeout => panic!(
                    "raw client: no matching line within {within:?}\n{}",
                    self.recent()
                ),
            }
        }
    }

    /// Asserts that the server closes this connection within `within`.
    ///
    /// # Panics
    ///
    /// Panics if a line arrives instead of an EOF, or if the window expires.
    pub async fn expect_closed(&mut self, within: Duration) {
        // No loop: an arriving line is a failure, so the single read is final.
        match self.read_outcome(within).await {
            ReadOutcome::Eof => {}
            ReadOutcome::Line(line) => panic!(
                "raw client: expected the server to close the connection, \
                 but it sent: {line}\n{}",
                self.recent()
            ),
            ReadOutcome::Timeout => panic!(
                "raw client: connection stayed open for {within:?}; \
                 expected the server to close it\n{}",
                self.recent()
            ),
        }
    }

    /// Reads one line into `history` and classifies the outcome.
    ///
    /// A read error other than a clean EOF is reported as [`ReadOutcome::Eof`]:
    /// on a Unix socket that means the peer is gone, which is exactly what
    /// [`RawClient::expect_closed`] must accept. The error text is recorded for
    /// diagnostics instead of panicking (reads never panic).
    async fn read_outcome(&mut self, within: Duration) -> ReadOutcome {
        let mut buffer = Vec::new();
        match tokio::time::timeout(within, self.reader.read_until(b'\n', &mut buffer)).await {
            Err(_) => ReadOutcome::Timeout,
            Ok(Err(error)) => {
                self.history.push(format!("<read error: {error}>"));
                ReadOutcome::Eof
            }
            Ok(Ok(0)) => ReadOutcome::Eof,
            Ok(Ok(_)) => {
                let mut line = String::from_utf8_lossy(&buffer).into_owned();
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                self.history.push(line.clone());
                ReadOutcome::Line(line)
            }
        }
    }

    /// The last few lines received, for panic messages.
    fn recent(&self) -> String {
        if self.history.is_empty() {
            return "raw client: <no lines received>".to_owned();
        }
        let start = self.history.len().saturating_sub(5);
        format!(
            "raw client: last lines:\n  {}",
            self.history[start..].join("\n  ")
        )
    }
}

/// Asserts that `result` is the AGP error `expected` (protocol §6).
///
/// # Panics
///
/// Panics with `what`, the expected code and the full actual result on any
/// other outcome (including a success).
pub fn assert_error_code<T: std::fmt::Debug>(
    result: Result<T, ClientError>,
    expected: ErrorCode,
    what: &str,
) {
    match result {
        Err(ClientError::Server { code, .. }) if code == expected => {}
        other => panic!("{what}: expected AGP error `{expected:?}`, got {other:?}"),
    }
}

/// Unwraps a successful AGP call.
///
/// # Panics
///
/// Panics with `what` and the full error if `result` is an error.
pub fn expect_ok<T: std::fmt::Debug>(result: Result<T, ClientError>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: expected Ok, got {error:?}"),
    }
}

/// Polls `check` every 10 ms until it returns `true` or `within` expires.
///
/// Returns the final verdict, so callers write
/// `assert!(eventually(..), "..")` and keep their own message. Used for
/// runtime-side effects (a window appears, a file is written, a child exits)
/// that are not AGP requests; never for AGP timeouts, which the protocol
/// expresses as `Observation::timed_out`.
pub fn eventually(within: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Writes a `.desktop` entry verbatim into `dir` and returns its path.
///
/// Creates `dir` (and parents) if needed. Contents are written exactly as
/// given, so the launch suite can pin both valid and malformed entries.
///
/// # Panics
///
/// Panics if the directory or file cannot be created: a missing fixture would
/// otherwise turn into a confusing assertion failure later.
pub fn write_desktop_entry(dir: &Path, file_name: &str, contents: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap_or_else(|error| {
        panic!(
            "E2E harness: create fixture dir {} failed: {error}",
            dir.display()
        )
    });
    let path = dir.join(file_name);
    std::fs::write(&path, contents).unwrap_or_else(|error| {
        panic!(
            "E2E harness: write fixture {} failed: {error}",
            path.display()
        )
    });
    path
}
