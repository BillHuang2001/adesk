//! Compile-shape guards for the harness's public API.
//!
//! Frozen acceptance spec for the implemented harness. Binds the main entry points to their
//! exact signatures, so a breaking change to the testkit's public surface fails to compile
//! here instead of silently changing the tests' meaning, and checks the runtime-free
//! constants and builder semantics.

use std::path::{Path, PathBuf};
use std::time::Duration;

use adesk_client::Client;
use adesk_compositor::RendererKind;
use adesk_testkit::{
    expected_window_geometry, gl_enabled, helper_bin_path, require_gl, test_renderer, wait_until,
    AppId, EnvScope, EventAssert, FillPattern, FixtureDir, ImageAssert, ImageBuffer, Rect, Result,
    RuntimeEvent, Size, TestApp, TestAppSpec, TestEnv, TestRuntime, TestRuntimeConfig,
    ToplevelSpec, WaylandTestClient, WindowId, DEFAULT_EVENT_CHANNEL_CAPACITY, DEFAULT_FILL,
    DEFAULT_OUTPUT_SIZE, DEFAULT_POLL_INTERVAL, DEFAULT_SHUTDOWN_TIMEOUT, GL_ENV_VAR,
};
use tokio::sync::broadcast;

// --- async entry points: `async fn` items cannot coerce to `fn` pointers, so each one is
// wrapped in a function with an explicit argument and return shape. ---

fn start() -> impl std::future::Future<Output = Result<TestRuntime>> {
    TestRuntime::start()
}

fn start_with(config: TestRuntimeConfig) -> impl std::future::Future<Output = Result<TestRuntime>> {
    TestRuntime::start_with(config)
}

fn shutdown(runtime: TestRuntime) -> impl std::future::Future<Output = Result<()>> {
    runtime.shutdown()
}

fn client(runtime: &TestRuntime) -> impl std::future::Future<Output = Result<Client>> + '_ {
    runtime.client()
}

fn capture(
    runtime: &TestRuntime,
    window_id: WindowId,
) -> impl std::future::Future<Output = Result<ImageBuffer>> + '_ {
    runtime.capture(window_id)
}

fn wait_for_window(
    runtime: &TestRuntime,
    timeout: Duration,
) -> impl std::future::Future<Output = Result<WindowId>> + '_ {
    runtime.wait_for_window(timeout)
}

fn wait_for_window_app<'a>(
    runtime: &'a TestRuntime,
    app_id: &'a AppId,
    timeout: Duration,
) -> impl std::future::Future<Output = Result<WindowId>> + 'a {
    runtime.wait_for_window_app(app_id, timeout)
}

fn spawn_test_app<'a>(
    runtime: &'a TestRuntime,
    spec: &'a TestAppSpec,
) -> impl std::future::Future<Output = Result<TestApp>> + 'a {
    TestApp::spawn(runtime, spec)
}

fn wait(timeout: Duration) -> impl std::future::Future<Output = Result<()>> {
    wait_until(timeout, "api surface", || true)
}

fn image_assert(image: &ImageBuffer) -> ImageAssert<'_> {
    ImageAssert::new(image)
}

#[test]
fn public_signatures_are_stable() {
    // Synchronous entry points coerce to plain function pointers.
    let _: fn(&Path, &str) -> Result<WaylandTestClient> = WaylandTestClient::connect_in;
    let _: fn(&str) -> Result<WaylandTestClient> = WaylandTestClient::connect;
    let _: fn(Size) -> Rect = expected_window_geometry;
    let _: fn(&str) -> Result<PathBuf> = helper_bin_path;
    let _: fn() -> Result<FixtureDir> = FixtureDir::new;
    let _: fn(&TestRuntime) -> &str = TestRuntime::wayland_display;
    let _: fn(&TestRuntime) -> &Path = TestRuntime::socket_path;
    let _: fn(&TestRuntime) -> Size = TestRuntime::output_size;
    let _: fn(&TestRuntime) -> Rect = TestRuntime::tiled_rect;
    let _: fn(&TestRuntime) -> bool = TestRuntime::is_shutdown;
    let _: fn(&TestRuntime) -> &TestRuntimeConfig = TestRuntime::config;
    let _: fn(&TestRuntime) -> &TestEnv = TestRuntime::env;
    let _: fn(&TestRuntime) -> Result<WaylandTestClient> = TestRuntime::wayland_client;
    let _: fn(&TestRuntime) -> broadcast::Receiver<RuntimeEvent> = TestRuntime::event_tap;
    let _: fn(&TestRuntime) -> EventAssert = EventAssert::tap;
    let _: fn(broadcast::Receiver<RuntimeEvent>) -> EventAssert = EventAssert::from_receiver;
    let _: fn(&TestEnv) -> EnvScope = TestEnv::apply;

    // Async entry points, pinned through the wrappers above.
    let _ = (
        start,
        start_with,
        shutdown,
        client,
        capture,
        wait_for_window,
        wait_for_window_app,
        spawn_test_app,
        wait,
        image_assert,
    );

    // Protocol and harness constants are the documented values.
    assert_eq!(adesk_testkit::adesk_proto::PROTOCOL_VERSION, 1);
    assert_eq!(DEFAULT_OUTPUT_SIZE, Size::new(1280, 800));
    assert_eq!(DEFAULT_SHUTDOWN_TIMEOUT, Duration::from_secs(5));
    assert_eq!(DEFAULT_EVENT_CHANNEL_CAPACITY, 4096);
    assert_eq!(DEFAULT_POLL_INTERVAL, Duration::from_millis(10));
    assert_eq!(DEFAULT_FILL, [32, 64, 96, 255]);
    assert_eq!(FillPattern::default(), FillPattern::solid_rgb(32, 64, 96));

    // Tiling is single-sourced from the wm policy, never hard-coded in a test.
    assert_eq!(
        expected_window_geometry(DEFAULT_OUTPUT_SIZE),
        Rect::new(0, 0, 1280, 800)
    );
    assert_eq!(
        expected_window_geometry(Size::new(640, 480)),
        Rect::new(0, 0, 640, 480)
    );

    // GL gating is opt-in and self-consistent.
    let gl_env = std::env::var(GL_ENV_VAR);
    assert_eq!(gl_enabled(), matches!(gl_env.as_deref(), Ok("1")));
    assert_eq!(require_gl().is_ok(), gl_enabled());
    assert_eq!(
        test_renderer(),
        if gl_enabled() {
            RendererKind::Gl
        } else {
            RendererKind::Pixman
        }
    );

    // Value builders stay chainable and expose their fields.
    let spec = ToplevelSpec::new("org.example.api", "API", Size::new(10, 20))
        .with_fill(FillPattern::solid_rgb(1, 2, 3))
        .with_title("API 2")
        .with_app_id("org.example.api2");
    assert_eq!(spec.app_id, "org.example.api2");
    assert_eq!(spec.title, "API 2");
    assert_eq!(spec.size, Size::new(10, 20));
    assert_eq!(spec.fill, FillPattern::solid_rgb(1, 2, 3));

    let fixtures = FixtureDir::new().expect("temp fixture dir");
    let config = TestRuntimeConfig::new()
        .with_output_size(Size::new(800, 600))
        .with_renderer(RendererKind::Pixman)
        .with_app_dirs(vec![PathBuf::from("/tmp/adesk-api-surface")])
        .with_fixture_dir(&fixtures)
        .with_event_channel_capacity(64)
        .with_shutdown_timeout(Duration::from_secs(3))
        .with_socket_name("wayland-adesk-api")
        .with_apply_env(false);
    assert_eq!(config.output_size, Size::new(800, 600));
    assert_eq!(config.renderer, RendererKind::Pixman);
    assert_eq!(
        config.app_dirs,
        vec![
            PathBuf::from("/tmp/adesk-api-surface"),
            fixtures.search_dir().to_path_buf(),
        ]
    );
    assert_eq!(config.event_channel_capacity, 64);
    assert_eq!(config.shutdown_timeout, Duration::from_secs(3));
    assert_eq!(config.socket_name.as_deref(), Some("wayland-adesk-api"));
    assert!(!config.apply_env);
}
