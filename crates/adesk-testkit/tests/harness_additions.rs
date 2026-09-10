//! Additive harness API: exec-path override, `title()`, custom AGP socket and viewer toggle.
//!
//! New-capability spec (the frozen acceptance specs are not edited). Every wait is
//! deadline-bounded and no test needs a display, GPU, network or installed application.

use std::path::Path;
use std::time::Duration;

use adesk_testkit::{
    helper_bin_path, Result, TestApp, TestAppSpec, TestRuntime, TestRuntimeConfig,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// A runtime that leaves the process environment alone (no test here launches through the
/// registry, so the env need not stay scoped; see `adesk_testkit::env`).
fn quiet_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new()
        .with_apply_env(false)
        .with_shutdown_timeout(DEADLINE)
}

/// The `Exec=` value of a serialized `.desktop` entry.
fn exec_line(desktop_file: &str) -> &str {
    desktop_file
        .lines()
        .find_map(|line| line.strip_prefix("Exec="))
        .expect("the entry has an Exec= line")
}

#[test]
fn with_exec_points_the_desktop_entry_at_the_override() -> Result<()> {
    let program = Path::new("/custom/fixture-app");
    let spec = TestAppSpec::new("org.example.override").with_exec(program);

    assert_eq!(spec.exec(), Some(program), "the override is reported back");

    // argv[0] of the fixture command is the override, not the packaged helper.
    let entry = spec.desktop_entry()?;
    assert_eq!(
        entry.exec.first().map(String::as_str),
        Some("/custom/fixture-app")
    );

    // The serialized Exec line starts with the override and keeps the standard arguments.
    let desktop_file = entry.to_desktop_file();
    let line = exec_line(&desktop_file);
    let rest = line
        .strip_prefix("/custom/fixture-app ")
        .unwrap_or_else(|| panic!("Exec line starts with the override: {line:?}"));
    assert_eq!(rest, spec.cli_args().join(" "), "arguments are unchanged");
    Ok(())
}

#[test]
fn without_exec_the_desktop_entry_uses_the_packaged_helper() -> Result<()> {
    let spec = TestAppSpec::new("org.example.default");
    assert_eq!(spec.exec(), None, "the packaged helper is the default");

    let helper = helper_bin_path("adesk-test-app")?;
    let expected = helper.to_str().expect("the helper path is UTF-8");

    // Default behaviour is unchanged: the helper binary is Exec's program.
    let entry = spec.desktop_entry()?;
    assert_eq!(entry.exec.first().map(String::as_str), Some(expected));

    let desktop_file = entry.to_desktop_file();
    let line = exec_line(&desktop_file);
    assert!(
        line.starts_with(expected),
        "Exec line uses the helper path {expected:?}: {line:?}"
    );
    Ok(())
}

#[tokio::test]
async fn spawn_uses_the_exec_override_as_the_program() -> Result<()> {
    let runtime = TestRuntime::start_with(quiet_config()).await?;

    // A program that does not exist: spawning must fail with exactly that path in the error,
    // which proves `spawn` ran the override instead of the packaged helper.
    let missing = runtime.env().root().join("no-such-fixture-program");
    let spec = TestAppSpec::new("org.example.spawn").with_exec(&missing);
    let error = TestApp::spawn(&runtime, &spec)
        .await
        .expect_err("spawning a missing program must fail");
    assert!(
        error.to_string().contains(&missing.display().to_string()),
        "the spawn error names the overridden program: {error}"
    );

    runtime.shutdown().await
}

#[test]
fn title_defaults_to_the_app_id_and_with_title_overrides() -> Result<()> {
    // The default title is the app id; `with_title` replaces it.
    assert_eq!(TestAppSpec::new("org.example.t").title(), "org.example.t");
    assert_eq!(
        TestAppSpec::new("org.example.t").with_title("T").title(),
        "T"
    );

    // The title is what the desktop entry carries as Name.
    let entry = TestAppSpec::new("org.example.t")
        .with_title("T")
        .desktop_entry()?;
    assert!(
        entry.to_desktop_file().contains("Name=T\n"),
        "{:?}",
        entry.to_desktop_file()
    );
    Ok(())
}

#[tokio::test]
async fn custom_agp_socket_is_bound_and_reachable() -> Result<()> {
    // A socket path under this test's own temp dir, not the runtime's private env dir.
    let socket_dir = tempfile::tempdir()?;
    let socket = socket_dir.path().join("custom-agp.sock");

    let runtime = TestRuntime::start_with(quiet_config().with_agp_socket(&socket)).await?;

    assert_eq!(runtime.socket_path(), socket);
    assert!(
        socket.exists(),
        "the configured AGP socket must be bound: {}",
        socket.display()
    );

    // The client connects to the configured path and round-trips.
    let client = runtime.client().await?;
    let ping = client.ping().await?;
    assert_eq!(
        ping.protocol_version,
        adesk_testkit::adesk_proto::PROTOCOL_VERSION
    );
    client.close().await?;

    runtime.shutdown().await
}

#[tokio::test]
async fn with_viewer_false_binds_no_vap_socket() -> Result<()> {
    // Disabled: no viewer socket is bound beside the AGP socket.
    let disabled = TestRuntime::start_with(quiet_config().with_viewer(false)).await?;
    let disabled_vap = adesk_server::config::viewer_socket_sibling(disabled.socket_path());
    assert!(
        !disabled_vap.exists(),
        "viewer disabled, yet {} exists",
        disabled_vap.display()
    );
    disabled.shutdown().await?;

    // The default runtime (viewer enabled) binds that sibling socket, so the toggle matters.
    let enabled = TestRuntime::start_with(quiet_config()).await?;
    let enabled_vap = adesk_server::config::viewer_socket_sibling(enabled.socket_path());
    assert!(
        enabled_vap.exists(),
        "viewer enabled by default, yet {} does not exist",
        enabled_vap.display()
    );
    enabled.shutdown().await
}
