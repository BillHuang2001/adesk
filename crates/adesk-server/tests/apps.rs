//! AGP §5.2 (`list_apps`, `get_app`) against a fixture `.desktop` directory.
//!
//! The runtime is started with [`TestRuntime::start_with_app_dirs`], which
//! *replaces* the XDG search directories, so the registry scans exactly the
//! entries written into a `tempfile::TempDir` and never the host's applications.
//! Four entries are written:
//!
//! - `org.example.demo` — listable, every `AppInfo` field populated,
//! - `org.example.hidden` — `Hidden=true`,
//! - `org.example.nodisplay` — `NoDisplay=true`,
//! - `org.example.broken` — invalid (no `Type`), so `scan()` records a
//!   `ScanIssue` and never lists it.
//!
//! A *missing `Exec`* is deliberately **not** the broken fixture: the registry
//! accepts an Exec-less entry (it is listed, and only `launch_app` fails with
//! `not_supported`), which
//! `list_apps_includes_a_valid_entry_without_exec` pins separately. The broken
//! entry therefore omits `Type`, the entry error `scan()` really skips.
//!
//! `launch_app` is covered by `launch.rs`; this suite only reads the registry.

mod common;

use adesk_core::{AppId, AppInfo, ErrorCode};
use common::{assert_error_code, expect_ok, write_desktop_entry, TestRuntime};

/// The listable fixture entry: every field `list_apps` exposes is populated.
const DEMO_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Demo App
Comment=A fixture application
GenericName=Demo
Exec=/bin/true --flag %u
Icon=demo-icon
Terminal=false
Categories=Utility;Network;
StartupWMClass=DemoApp
";

/// `Hidden=true` — a deleted/shadowed entry, hidden from `list_apps` unless
/// `include_hidden` is set.
const HIDDEN_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Hidden App
Exec=/bin/true
Hidden=true
";

/// `NoDisplay=true` — a real entry that menus hide; treated exactly like
/// `Hidden` by `list_apps`.
const NODISPLAY_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=No Display App
Exec=/bin/true
NoDisplay=true
";

/// Invalid entry: no `Type` key, so `DesktopEntry::from_raw` rejects it with
/// `MissingType` and `scan()` records a `ScanIssue` instead of listing it.
const BROKEN_ENTRY: &str = "\
[Desktop Entry]
Name=Broken Entry
Exec=/bin/true
";

/// Writes the four fixture entries into a fresh temp dir.
fn fixture_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(dir.path(), "org.example.demo.desktop", DEMO_ENTRY);
    write_desktop_entry(dir.path(), "org.example.hidden.desktop", HIDDEN_ENTRY);
    write_desktop_entry(dir.path(), "org.example.nodisplay.desktop", NODISPLAY_ENTRY);
    write_desktop_entry(dir.path(), "org.example.broken.desktop", BROKEN_ENTRY);
    dir
}

/// The ids of `apps`, in the order the server returned them.
fn ids(apps: &[AppInfo]) -> Vec<&str> {
    apps.iter().map(|app| app.id.as_str()).collect()
}

/// Pins every field of the `org.example.demo` entry.
///
/// Shared by `list_apps` and `get_app` so both methods are asserted to project
/// the same [`AppInfo`].
fn assert_demo_entry(app: &AppInfo, what: &str) {
    assert_eq!(app.id, AppId::from("org.example.demo"), "{what}: id");
    assert_eq!(app.name, "Demo App", "{what}: name");
    assert_eq!(app.icon.as_deref(), Some("demo-icon"), "{what}: icon");
    let exec = app.exec.as_deref().expect("demo entry has an Exec line");
    assert!(
        exec.contains("/bin/true"),
        "{what}: exec keeps the raw command line, got {exec:?}"
    );
    assert!(
        exec.contains("%u"),
        "{what}: field codes stay unexpanded in AppInfo.exec, got {exec:?}"
    );
    assert!(!app.terminal, "{what}: Terminal=false");
    assert!(
        app.categories.contains(&"Utility".to_owned()),
        "{what}: categories contain Utility, got {:?}",
        app.categories
    );
    assert!(
        app.categories.contains(&"Network".to_owned()),
        "{what}: categories contain Network, got {:?}",
        app.categories
    );
    assert_eq!(
        app.startup_wm_class.as_deref(),
        Some("DemoApp"),
        "{what}: startup_wm_class"
    );
    assert!(!app.hidden, "{what}: hidden");
    assert!(!app.no_display, "{what}: no_display");
    assert!(
        !app.dbus_activatable,
        "{what}: DBusActivatable is absent, so the entry is not DBus-activated"
    );
    assert!(app.try_exec.is_none(), "{what}: TryExec is absent");
}

#[test]
fn list_apps_returns_only_the_listable_fixture_entries() {
    let dir = fixture_dir();
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    let apps = expect_ok(
        t.block_on_timeout(client.list_apps(None, false)),
        "list_apps(None, false)",
    );

    assert_eq!(
        ids(&apps),
        vec!["org.example.demo"],
        "with include_hidden=false only the listable fixture entry may be returned"
    );
    assert!(
        apps.iter()
            .all(|app| app.id.as_str() != "org.example.broken"),
        "the invalid fixture entry (no Type) must be skipped by scan(), got {:?}",
        ids(&apps)
    );
    assert_demo_entry(&apps[0], "list_apps(None, false)[0]");
}

#[test]
fn list_apps_query_matches_id_and_name_case_insensitively() {
    let dir = fixture_dir();
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    let by_name = expect_ok(
        t.block_on_timeout(client.list_apps(Some("DEMO"), false)),
        "list_apps(Some(\"DEMO\"), false)",
    );
    assert_eq!(
        ids(&by_name),
        vec!["org.example.demo"],
        "the query is matched case-insensitively against Name"
    );

    let by_id = expect_ok(
        t.block_on_timeout(client.list_apps(Some("example.demo"), false)),
        "list_apps(Some(\"example.demo\"), false)",
    );
    assert_eq!(
        ids(&by_id),
        vec!["org.example.demo"],
        "the query is matched as a substring of the desktop-file id"
    );

    let unmatched = expect_ok(
        t.block_on_timeout(client.list_apps(Some("zzz-no-match"), false)),
        "list_apps(Some(\"zzz-no-match\"), false)",
    );
    assert!(
        unmatched.is_empty(),
        "a query matching no entry returns an empty list, got {:?}",
        ids(&unmatched)
    );

    let nodisplay_hidden = expect_ok(
        t.block_on_timeout(client.list_apps(Some("nodisplay"), false)),
        "list_apps(Some(\"nodisplay\"), false)",
    );
    assert!(
        nodisplay_hidden.is_empty(),
        "NoDisplay=true entries are excluded when include_hidden=false, got {:?}",
        ids(&nodisplay_hidden)
    );

    let nodisplay_shown = expect_ok(
        t.block_on_timeout(client.list_apps(Some("nodisplay"), true)),
        "list_apps(Some(\"nodisplay\"), true)",
    );
    assert_eq!(
        ids(&nodisplay_shown),
        vec!["org.example.nodisplay"],
        "the same query with include_hidden=true proves the entry exists and was filtered out"
    );
}

#[test]
fn list_apps_include_hidden_includes_hidden_and_nodisplay() {
    let dir = fixture_dir();
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    let apps = expect_ok(
        t.block_on_timeout(client.list_apps(None, true)),
        "list_apps(None, true)",
    );

    assert_eq!(
        ids(&apps),
        vec![
            "org.example.demo",
            "org.example.hidden",
            "org.example.nodisplay",
        ],
        "include_hidden=true adds Hidden and NoDisplay entries; the invalid one stays absent"
    );

    let hidden = apps
        .iter()
        .find(|app| app.id == AppId::from("org.example.hidden"))
        .expect("the hidden fixture entry is listed with include_hidden=true");
    assert!(hidden.hidden, "Hidden=true is projected");
    assert!(!hidden.no_display, "hidden entry has no NoDisplay");

    let nodisplay = apps
        .iter()
        .find(|app| app.id == AppId::from("org.example.nodisplay"))
        .expect("the NoDisplay fixture entry is listed with include_hidden=true");
    assert!(nodisplay.no_display, "NoDisplay=true is projected");
    assert!(!nodisplay.hidden, "NoDisplay entry has no Hidden");
}

#[test]
fn get_app_returns_the_entry() {
    let dir = fixture_dir();
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    let app = expect_ok(
        t.block_on_timeout(client.get_app(&AppId::from("org.example.demo"))),
        "get_app(org.example.demo)",
    );

    assert_demo_entry(&app, "get_app(org.example.demo)");
}

#[test]
fn get_app_unknown_is_unknown_app() {
    let dir = fixture_dir();
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    assert_error_code(
        t.block_on_timeout(client.get_app(&AppId::from("org.example.absent"))),
        ErrorCode::UnknownApp,
        "get_app(org.example.absent)",
    );

    // An error response must not close the connection (§6).
    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after unknown_app must still succeed",
    );
}

/// An entry without `Exec` is *valid*: the registry lists it and only
/// `launch_app` fails (with `not_supported`). This is why the "broken" fixture
/// above omits `Type` instead.
#[test]
fn list_apps_includes_a_valid_entry_without_exec() {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(
        dir.path(),
        "org.example.noexec.desktop",
        "[Desktop Entry]\nType=Application\nName=No Exec App\n",
    );
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    let apps = expect_ok(
        t.block_on_timeout(client.list_apps(None, false)),
        "list_apps(None, false) for an Exec-less entry",
    );

    assert_eq!(
        ids(&apps),
        vec!["org.example.noexec"],
        "a missing Exec is a launch-time error, not a scan-time one"
    );
    assert!(
        apps[0].exec.is_none(),
        "the absent Exec line projects as None"
    );
}
