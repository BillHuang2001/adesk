//! Runtime lifecycle and AGP surface of the harness.
//!
//! Frozen acceptance spec for the implemented harness. Covers starting/stopping a real
//! in-process runtime, its default renderer and output size, the AGP client round trip
//! (`ping`, `list_windows`, a failing `capture_window`), non-blocking `Drop` and repeated
//! shutdown cycles.

use std::time::Duration;

use adesk_client::{CaptureRequest, ClientError, Renderer};
use adesk_compositor::RendererName;
use adesk_testkit::{
    expected_window_geometry, Result, Size, TestRuntime, TestRuntimeConfig, WindowId,
    DEFAULT_OUTPUT_SIZE,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// An id no runtime can have assigned (window ids are monotonic from `1`).
const UNKNOWN_WINDOW: WindowId = WindowId(u64::MAX);

/// A runtime that leaves the process environment alone.
///
/// The AGP client and the Wayland test client both take explicit paths, so no test in this
/// file needs [`TestRuntimeConfig::apply_env`], and parallel tests must not race on the
/// process environment (see `adesk_testkit::env`). The graceful-shutdown bound is the only
/// explicit wait this file performs, so it carries [`DEADLINE`].
fn quiet_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new()
        .with_apply_env(false)
        .with_shutdown_timeout(DEADLINE)
}

#[tokio::test]
async fn start_stop_runtime() -> Result<()> {
    let runtime = TestRuntime::start_with(quiet_config()).await?;

    assert!(
        runtime.socket_path().exists(),
        "the AGP socket must exist once the runtime is up: {}",
        runtime.socket_path().display()
    );
    assert!(runtime.env().runtime_dir().is_dir());
    assert!(!runtime.wayland_display().is_empty());
    assert!(!runtime.is_shutdown());

    let client = runtime.client().await?;
    let ping = client.ping().await?;
    assert_eq!(ping.protocol_version, 1);
    assert_eq!(
        ping.protocol_version,
        adesk_testkit::adesk_proto::PROTOCOL_VERSION
    );
    assert!(
        !ping.runtime_version.is_empty(),
        "ping reports a runtime version"
    );
    client.close().await?;

    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn default_runtime_is_pixman_1280x800() -> Result<()> {
    let runtime = TestRuntime::start_with(quiet_config()).await?;

    assert_eq!(DEFAULT_OUTPUT_SIZE, Size::new(1280, 800));
    assert_eq!(runtime.output_size(), DEFAULT_OUTPUT_SIZE);
    // Tiling is never hard-coded: the expected rect comes from the wm policy.
    assert_eq!(
        runtime.tiled_rect(),
        expected_window_geometry(DEFAULT_OUTPUT_SIZE)
    );

    // The compositor handle agrees with the configuration.
    assert_eq!(runtime.compositor().output_size(), runtime.output_size());
    assert_eq!(
        runtime.compositor().wayland_display_name().as_deref(),
        Some(runtime.wayland_display())
    );
    assert_eq!(runtime.compositor().renderer(), Some(RendererName::Pixman));

    let client = runtime.client().await?;
    let ping = client.ping().await?;
    assert_eq!(
        ping.renderer,
        Renderer::Pixman,
        "the default test renderer must be the software path (CI has no GPU)"
    );
    assert_eq!(ping.output, DEFAULT_OUTPUT_SIZE);
    assert!(
        ping.uptime_ms < 60_000,
        "uptime is monotonic ms since runtime start, got {}",
        ping.uptime_ms
    );

    runtime.shutdown().await
}

#[tokio::test]
async fn drop_does_not_hang() -> Result<()> {
    let first = TestRuntime::start_with(quiet_config()).await?;
    let first_display = first.wayland_display().to_string();
    let first_socket = first.socket_path().to_path_buf();
    drop(first); // best-effort teardown: never blocks, never panics

    // The process is still responsive and can host another runtime with fresh paths.
    let second = TestRuntime::start_with(quiet_config()).await?;
    assert_ne!(second.wayland_display(), first_display);
    assert_ne!(second.socket_path(), first_socket);
    let client = second.client().await?;
    assert_eq!(client.ping().await?.protocol_version, 1);
    client.close().await?;

    second.shutdown().await
}

#[tokio::test]
async fn client_roundtrip() -> Result<()> {
    let runtime = TestRuntime::start_with(quiet_config()).await?;
    let client = runtime.client().await?;

    assert_eq!(client.socket_path(), runtime.socket_path());
    assert!(!client.is_closed());

    let windows = client.list_windows().await?;
    assert!(
        windows.windows.is_empty(),
        "a fresh runtime has no windows, got {:?}",
        windows.windows
    );
    assert_eq!(windows.active_window_id, None);

    let error = client
        .capture_window(CaptureRequest::window(UNKNOWN_WINDOW))
        .await
        .expect_err("capturing an unknown window must fail");
    assert!(
        matches!(&error, ClientError::Server { .. }),
        "an unknown window is a server error, got {error:?}"
    );

    // A failed request must not poison the connection.
    assert_eq!(client.ping().await?.protocol_version, 1);
    client.close().await?;

    runtime.shutdown().await
}

#[tokio::test]
async fn shutdown_is_idempotent() -> Result<()> {
    // `TestRuntime::shutdown(self)` consumes the handle, so "shutting down twice" cannot be
    // expressed on one value. What is observable is that a runtime nobody used shuts down
    // cleanly and that repeated start/shutdown cycles in one process stay stable.
    let unused = TestRuntime::start_with(quiet_config()).await?;
    assert!(!unused.is_shutdown());
    unused.shutdown().await?;

    let used = TestRuntime::start_with(quiet_config()).await?;
    assert!(!used.is_shutdown());
    let client = used.client().await?;
    assert_eq!(client.ping().await?.protocol_version, 1);
    client.close().await?;
    used.shutdown().await?;

    // A third cycle proves the first two released their temp dirs and sockets.
    let last = TestRuntime::start_with(quiet_config()).await?;
    assert!(last.socket_path().exists());
    last.shutdown().await
}
