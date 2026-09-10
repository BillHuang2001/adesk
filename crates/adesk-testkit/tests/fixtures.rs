//! `.desktop` fixtures, the isolated app registry and the `adesk-test-app` helper.
//!
//! Frozen acceptance spec for the implemented harness. Covers writing entries into a
//! [`FixtureDir`] (share root, id rule, exact serialization), listing them through the
//! runtime's registry, launching a fixture app over AGP and driving the helper process
//! directly.
//!
//! # Process-environment hazard
//!
//! Two tests in this binary apply the process environment (`with_apply_env(true)`):
//! [`launch_app_starts_helper_window`] (the registry launches children with
//! `LaunchEnv::from_process()`, so the helper needs this process's `WAYLAND_DISPLAY` and
//! `XDG_RUNTIME_DIR`) and [`test_app_spawn_and_exit`] (the compositor binds its Wayland
//! socket under the process `XDG_RUNTIME_DIR`, so the helper can only reach it when the
//! env is scoped to the runtime). Every other runtime here disables it; the harness
//! serializes env-scoped runtimes within one test binary itself, so no `--test-threads=1`
//! is required.

use std::path::Path;
use std::time::Duration;

use adesk_testkit::{
    helper_bin_path, wait_until, AppId, DesktopEntryFixture, EventAssert, Expected, FillPattern,
    FixtureDir, ImageAssert, Result, RuntimeEvent, Size, TestApp, TestAppSpec, TestRuntime,
    TestRuntimeConfig,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

#[tokio::test]
async fn desktop_entry_is_written_and_listed() -> Result<()> {
    let fixtures = FixtureDir::new()?;
    let entry = DesktopEntryFixture::new("Fixture Demo", ["/bin/true", "--fixture"])
        .with_comment("written by adesk-testkit")
        .with_category("Utility");
    let app_id = fixtures.write_entry("org.example.fixture", &entry)?;
    assert_eq!(app_id, AppId::from("org.example.fixture"));

    // The id rule is the registry's: path relative to `applications/`, `.desktop` stripped,
    // `/` replaced by `.`.
    let nested = fixtures.write_entry(
        "nested/kate",
        &DesktopEntryFixture::new("Kate", ["/bin/true"]),
    )?;
    assert_eq!(nested, AppId::from("nested.kate"));

    let path = fixtures
        .applications_dir()
        .join("org.example.fixture.desktop");
    assert!(
        path.is_file(),
        "write_entry writes into <share root>/applications: {}",
        path.display()
    );
    let text = std::fs::read_to_string(&path)?;
    assert_eq!(
        text,
        entry.to_desktop_file(),
        "the file is exactly the serialized entry"
    );
    assert!(text.starts_with("[Desktop Entry]\n"), "{text:?}");
    assert!(text.contains("Type=Application\n"), "{text:?}");
    assert!(text.contains("Name=Fixture Demo\n"), "{text:?}");
    assert!(text.contains("Exec=/bin/true --fixture\n"), "{text:?}");
    assert!(text.contains("Categories=Utility\n"), "{text:?}");

    // The runtime scans the fixture share root, so the AGP registry lists the entry.
    let runtime = TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_fixture_dir(&fixtures)
            .with_apply_env(false),
    )
    .await?;
    let registry = runtime.registry();
    assert!(
        registry.contains(&app_id),
        "the registry scanned {}",
        fixtures.search_dir().display()
    );
    let info = registry.get(&app_id).expect("the entry is known");
    assert_eq!(info.name, "Fixture Demo");
    assert_eq!(info.exec.as_deref(), Some("/bin/true --fixture"));
    assert_eq!(info.categories, vec!["Utility".to_string()]);
    let listed = registry.list(Some("org.example.fixture"), false);
    assert_eq!(
        listed.iter().map(|app| app.id.clone()).collect::<Vec<_>>(),
        vec![app_id.clone()]
    );

    runtime.shutdown().await
}

#[tokio::test]
async fn launch_app_starts_helper_window() -> Result<()> {
    let fixtures = FixtureDir::new()?;
    let fill = FillPattern::solid_rgb(10, 120, 200);
    let spec = TestAppSpec::new("org.example.launched")
        .with_title("Launched Fixture")
        .with_size(Size::new(320, 200))
        .with_fill(fill);
    let app_id = fixtures.write_app(&spec)?;
    assert_eq!(app_id, *spec.app_id());

    // One of the two env-applying runtimes in this binary (see the module docs).
    let runtime = TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_fixture_dir(&fixtures)
            .with_apply_env(true),
    )
    .await?;
    let client = runtime.client().await?;

    // Subscribe before launching: a broadcast channel does not replay.
    let mut events = EventAssert::tap(&runtime);
    let launched = client.launch_app(&app_id, &[]).await?;
    assert_eq!(launched.app_id, app_id);
    assert!(launched.pid.is_some(), "the spawned helper reports its pid");
    events
        .wait_for_expected(&Expected::AppLaunched, DEADLINE)
        .await?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(app_id.clone()), DEADLINE)
        .await?;
    let RuntimeEvent::WindowCreated {
        window_id,
        app_id: reported,
        pid,
        launch_id,
        title,
        ..
    } = &created
    else {
        panic!("wait_for_expected(WindowCreatedFor) returned {created:?}");
    };
    assert_eq!(reported.as_ref(), Some(&app_id));
    assert_eq!(
        pid, &launched.pid,
        "the window correlates with the launched process"
    );
    assert_eq!(launch_id, &Some(launched.launch_id));

    let info = client.get_window(*window_id).await?;
    assert_eq!(info.app_id, Some(app_id.clone()));
    assert_eq!(
        info.geometry,
        runtime.tiled_rect(),
        "launched windows are tiled like any other toplevel"
    );
    assert_eq!(info.title.as_deref(), title.as_deref());
    let listed = client.list_windows().await?;
    assert!(listed.windows.iter().any(|w| w.id == *window_id));

    // The helper paints its own fill, so the launch produced real pixels.
    let image = runtime.capture(*window_id).await?;
    ImageAssert::new(&image).matches_pattern(fill);

    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn test_app_spawn_and_exit() -> Result<()> {
    // The helper connects to the runtime's Wayland socket, which the compositor binds
    // under the process `XDG_RUNTIME_DIR`, so this runtime must scope the env too (see the
    // module docs; `TestApp::spawn` then passes the runtime's own values to the child).
    let runtime = TestRuntime::start_with(TestRuntimeConfig::new().with_apply_env(true)).await?;
    let spec = TestAppSpec::new("org.example.helper").with_size(Size::new(160, 120));

    let mut events = EventAssert::tap(&runtime);
    let mut app = TestApp::spawn(&runtime, &spec).await?;
    assert_eq!(app.app_id(), spec.app_id());
    let pid = app.pid().expect("a spawned helper reports its pid");
    assert!(
        app.is_running()?,
        "the helper runs until it is told to exit"
    );
    events
        .wait_for_expected(&Expected::WindowCreatedFor(spec.app_id().clone()), DEADLINE)
        .await?;

    let status = app.exit().await?;
    assert!(
        status.success(),
        "graceful exit must succeed, got {status:?}"
    );
    // `exit` consumed the handle and reaped the child; on Linux its /proc entry is gone.
    if Path::new("/proc").is_dir() {
        wait_until(DEADLINE, "helper process to disappear", || {
            !Path::new(&format!("/proc/{pid}")).exists()
        })
        .await?;
    }

    // The kill path reports a signal status and is observed by `is_running`.
    let mut second = TestApp::spawn(&runtime, &spec).await?;
    assert!(second.is_running()?);
    second.kill()?;
    let status = second.wait_for_exit(DEADLINE).await?;
    assert!(
        !status.success(),
        "a killed helper must not report success: {status:?}"
    );
    assert!(!second.is_running()?, "after reaping, the helper is gone");

    runtime.shutdown().await
}

#[test]
fn helper_bin_path_resolves() {
    // Cargo defines CARGO_BIN_EXE_<name> at compile time for this package's own
    // integration tests; downstream crates must use `helper_bin_path` instead.
    let cargo_helper = Path::new(env!("CARGO_BIN_EXE_adesk-test-app"));
    assert!(
        cargo_helper.is_file(),
        "cargo built the helper at {}",
        cargo_helper.display()
    );

    let resolved = helper_bin_path("adesk-test-app").expect("helper_bin_path finds the helper");
    assert!(resolved.is_file(), "{}", resolved.display());
    assert_eq!(
        resolved.file_name().and_then(|name| name.to_str()),
        Some("adesk-test-app")
    );

    // The spec embeds exactly that path as argv[0] of the fixture Exec line.
    let spec = TestAppSpec::new("org.example.helper");
    let entry = spec.desktop_entry().expect("the helper path is UTF-8");
    assert_eq!(
        entry.exec.first().map(String::as_str),
        Some(resolved.to_string_lossy().as_ref())
    );
    assert_eq!(
        entry.startup_wm_class.as_deref(),
        Some("org.example.helper")
    );
    assert_eq!(
        spec.cli_args().first().map(String::as_str),
        Some("--app-id")
    );
    assert_eq!(spec.cli_args().len(), 8, "{:?}", spec.cli_args());
}
