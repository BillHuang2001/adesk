//! Integration tests for [`AppRegistry`]: scan/discovery, listing, lookup and launch.
//!
//! Public API only. Fixture trees are written into `tempfile::tempdir()` and the
//! two injection seams ([`ProcessSpawner`] and [`Clock`]) come from `support`, so
//! no test spawns a process, needs a display/GPU/network or an installed
//! application. The suite covers the whole documented registry matrix: recursive
//! discovery, hidden/symlink handling, scan tolerance and counters, precedence
//! shadowing, locale-resolved names, listing/filtering, lookup, `Exec` expansion,
//! terminal wrapping, environment overrides and every launch error path.

mod support;

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;

use adesk_app_registry::{
    search_dirs, AppRegistry, Error, ExecError, LaunchEnv, LaunchRecord, RegistryOptions,
    ScanReport, SpawnCommand, TerminalSpec,
};
use adesk_core::LaunchId;

use support::{
    app as app_id, as_clock, fake_clock, write_entry, write_file, FakeClock, RecordingSpawner,
    SpawnOutcome, SPAWN_PID,
};

/// The mandatory keys of a valid entry; test bodies append `Name`/`Exec`/... to it.
const HEADER: &str = "Type=Application\n";

/// Body of a minimal `Type=Application` entry named `name`.
fn desktop(name: &str) -> String {
    format!("{HEADER}Name={name}\n")
}

/// Ids of a listing, for order-sensitive assertions.
fn listed(registry: &AppRegistry, query: Option<&str>, include_hidden: bool) -> Vec<String> {
    registry
        .list(query, include_hidden)
        .iter()
        .map(|info| info.id.0.clone())
        .collect()
}

/// Full argv of a recorded command, program first.
fn argv(command: &SpawnCommand) -> Vec<&str> {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect()
}

/// Registry plus the shared doubles it was built with.
struct Harness {
    registry: AppRegistry,
    spawner: Arc<RecordingSpawner>,
    clock: Arc<FakeClock>,
}

impl Harness {
    fn with_locale(dirs: &[PathBuf], locale: Option<&str>) -> Harness {
        let spawner = RecordingSpawner::new().shared();
        let clock = fake_clock(0);
        let options = RegistryOptions::with_search_dirs(dirs.iter().cloned())
            .with_spawner(spawner.clone())
            .with_clock(as_clock(&clock))
            .with_locale(locale.map(str::to_string));
        Harness {
            registry: AppRegistry::with_options(options),
            spawner,
            clock,
        }
    }

    fn new(dirs: &[PathBuf]) -> Harness {
        Harness::with_locale(dirs, None)
    }

    fn scanned(dirs: &[PathBuf]) -> (Harness, ScanReport) {
        let harness = Harness::new(dirs);
        let report = harness.registry.scan().expect("scan succeeds");
        (harness, report)
    }

    /// Scans a temp dir holding one entry whose body follows [`HEADER`].
    fn entry(dir: &tempfile::TempDir, relative: &str, body: &str) -> Harness {
        write_entry(dir.path(), relative, &format!("{HEADER}{body}"));
        Harness::scanned(&[dir.path().to_path_buf()]).0
    }

    fn launch(&self, app: &str) -> adesk_app_registry::Result<LaunchRecord> {
        self.registry.launch(&app_id(app), &[], &LaunchEnv::new())
    }
}

// --- options / empty registry ----------------------------------------------

#[test]
fn options_builders_replace_their_fields() {
    let spawner = RecordingSpawner::new().shared();
    let clock = fake_clock(7);
    let terminal = TerminalSpec::with_args("kitty", vec!["-e".to_string()]);
    let options = RegistryOptions::with_search_dirs(Vec::new())
        .with_spawner(spawner.clone())
        .with_clock(as_clock(&clock))
        .with_terminal(terminal.clone())
        .with_locale(Some("de_DE.UTF-8".to_string()));

    assert!(options.search_dirs.is_empty());
    assert_eq!(options.terminal, terminal);
    assert_eq!(options.locale.as_deref(), Some("de_DE.UTF-8"));
    assert_eq!(options.clock.now_ms(), 7);
    assert!(format!("{:?}", options.spawner).contains("RecordingSpawner"));
}

#[test]
fn new_registry_is_empty_and_exposes_its_options() {
    let registry = AppRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.list(None, true).is_empty());
    assert_eq!(registry.search_dirs(), search_dirs().as_slice());
    assert_eq!(registry.options().terminal, TerminalSpec::from_env());
    assert!(AppRegistry::default().is_empty());

    let dirs = vec![PathBuf::from("/tmp/adesk-only")];
    let explicit = AppRegistry::with_options(RegistryOptions::with_search_dirs(dirs.clone()));
    assert_eq!(explicit.search_dirs(), dirs.as_slice());
    assert_eq!(explicit.options().search_dirs, dirs);
}

// --- scan -------------------------------------------------------------------

#[test]
fn scan_walks_recursively_and_ignores_hidden_and_non_desktop_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(dir.path(), "alpha.desktop", &desktop("Alpha"));
    write_entry(dir.path(), "sub/beta.desktop", &desktop("Beta"));
    write_entry(dir.path(), "sub/deeper/gamma.desktop", &desktop("Gamma"));
    write_entry(dir.path(), "notes.txt", "not a desktop file");
    write_entry(dir.path(), ".hidden.desktop", &desktop("Hidden"));
    write_entry(dir.path(), ".hidden/delta.desktop", &desktop("Delta"));

    let (harness, report) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(report.dirs_scanned, 1);
    assert_eq!(report.files_scanned, 3);
    assert_eq!(report.apps, 3);
    assert_eq!(report.skipped, 0);
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
    assert_eq!(harness.registry.len(), 3);
    assert_eq!(
        listed(&harness.registry, None, true),
        ["alpha", "sub.beta", "sub.deeper.gamma"]
    );
}

#[test]
fn scan_skips_directory_symlinks_but_reads_file_symlinks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside dir");
    write_entry(dir.path(), "real/entry.desktop", &desktop("Real"));
    symlink(dir.path().join("real"), dir.path().join("link")).expect("dir symlink");
    let target = write_entry(outside.path(), "linked.desktop", &desktop("Linked"));
    symlink(target, dir.path().join("linked.desktop")).expect("file symlink");

    let (harness, report) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!((report.apps, report.files_scanned), (2, 2));
    // `link.entry` would appear here if the directory symlink were followed.
    assert_eq!(
        listed(&harness.registry, None, true),
        ["linked", "real.entry"]
    );
}

#[test]
fn scan_counts_non_application_entries_as_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(dir.path(), "app.desktop", &desktop("App"));
    write_entry(dir.path(), "link.desktop", "Type=Link\nName=Link\n");

    let (harness, report) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(
        (report.files_scanned, report.skipped, report.apps),
        (2, 1, 1)
    );
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
    assert_eq!(listed(&harness.registry, None, true), ["app"]);
}

#[test]
fn scan_records_issues_without_aborting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let garbage = write_file(dir.path(), "garbage.desktop", "no group header\n");
    let nameless = write_entry(dir.path(), "nameless.desktop", HEADER);
    let typeless = write_entry(dir.path(), "typeless.desktop", "Name=Typeless\n");
    write_entry(dir.path(), "fine.desktop", &desktop("Fine"));
    write_entry(dir.path(), "good.desktop", &desktop("Good"));
    let dangling = dir.path().join("dangling.desktop");
    symlink(dir.path().join("missing-target"), &dangling).expect("broken symlink");

    let (harness, report) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(
        (report.apps, report.files_scanned, report.skipped),
        (2, 5, 0)
    );
    let mut paths: Vec<PathBuf> = report
        .issues
        .iter()
        .map(|issue| issue.path.clone())
        .collect();
    paths.sort();
    let mut expected = vec![garbage, nameless, typeless, dangling];
    expected.sort();
    assert_eq!(paths, expected);

    let reasons: Vec<&str> = report
        .issues
        .iter()
        .map(|issue| issue.reason.as_str())
        .collect();
    for fragment in [
        "missing [Desktop Entry] group",
        "missing Name key",
        "missing Type key",
    ] {
        assert!(
            reasons.iter().any(|reason| reason.contains(fragment)),
            "no issue mentions {fragment}: {reasons:?}"
        );
    }
    assert!(harness.registry.contains(&app_id("good")));
}

#[test]
fn scan_dedupes_by_precedence_and_a_hidden_entry_shadows() {
    let high = tempfile::tempdir().expect("high dir");
    let low = tempfile::tempdir().expect("low dir");
    write_entry(
        high.path(),
        "dup.desktop",
        &format!("{HEADER}Name=Shadowing\nHidden=true\n"),
    );
    write_entry(high.path(), "only-high.desktop", &desktop("OnlyHigh"));
    write_entry(low.path(), "dup.desktop", &desktop("Shadowed"));
    write_entry(low.path(), "only-low.desktop", &desktop("OnlyLow"));

    let dirs = [high.path().to_path_buf(), low.path().to_path_buf()];
    let (harness, report) = Harness::scanned(&dirs);

    assert_eq!(
        (report.dirs_scanned, report.files_scanned, report.apps),
        (2, 4, 3)
    );
    let dup = harness.registry.get(&app_id("dup")).expect("dup is kept");
    assert_eq!(dup.name, "Shadowing");
    assert!(dup.hidden);
    assert_eq!(
        listed(&harness.registry, None, false),
        ["only-high", "only-low"]
    );
    assert_eq!(
        listed(&harness.registry, None, true),
        ["dup", "only-high", "only-low"]
    );
}

#[test]
fn scan_replaces_the_table_atomically() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(dir.path(), "first.desktop", &desktop("First"));
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);
    assert_eq!(harness.registry.len(), 1);

    std::fs::remove_file(dir.path().join("first.desktop")).expect("remove fixture");
    write_entry(dir.path(), "second.desktop", &desktop("Second"));
    assert_eq!(harness.registry.scan().expect("second scan").apps, 1);

    assert!(harness.registry.contains(&app_id("second")));
    assert!(!harness.registry.contains(&app_id("first")));
    assert_eq!(listed(&harness.registry, None, true), ["second"]);
}

#[test]
fn scan_ignores_missing_search_directories() {
    let (harness, report) = Harness::scanned(&[PathBuf::from("/nonexistent/adesk-apps")]);

    assert_eq!(report, ScanReport::default());
    assert!(harness.registry.is_empty());
}

#[test]
fn scan_uses_the_configured_locale_for_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(
        dir.path(),
        "app.desktop",
        &format!("{HEADER}Name=Base\nName[fr]=Francais\n"),
    );

    let harness = Harness::with_locale(&[dir.path().to_path_buf()], Some("fr_FR.UTF-8"));
    harness.registry.scan().expect("scan");

    let info = harness.registry.get(&app_id("app")).expect("app");
    assert_eq!(info.name, "Francais");
    assert_eq!(listed(&harness.registry, Some("franc"), false), ["app"]);
    assert!(
        listed(&harness.registry, Some("base"), false).is_empty(),
        "the localized name replaces the base Name for queries"
    );
}

// --- list / get / contains --------------------------------------------------

#[test]
fn list_filters_hidden_and_no_display_unless_requested() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(dir.path(), "visible.desktop", &desktop("Visible"));
    write_entry(
        dir.path(),
        "hidden.desktop",
        &format!("{HEADER}Name=Hidden\nHidden=true\n"),
    );
    write_entry(
        dir.path(),
        "nodisplay.desktop",
        &format!("{HEADER}Name=NoDisplay\nNoDisplay=true\n"),
    );
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(listed(&harness.registry, None, false), ["visible"]);
    assert_eq!(
        listed(&harness.registry, None, true),
        ["hidden", "nodisplay", "visible"]
    );
}

#[test]
fn list_matches_query_against_id_and_name_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(
        dir.path(),
        "org.example.editor.desktop",
        &desktop("Example Editor"),
    );
    write_entry(dir.path(), "terminal.desktop", &desktop("Shell"));
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(
        listed(&harness.registry, Some("EDITOR"), false),
        ["org.example.editor"]
    );
    assert_eq!(
        listed(&harness.registry, Some("editor"), false),
        ["org.example.editor"]
    );
    assert_eq!(
        listed(&harness.registry, Some("shell"), false),
        ["terminal"]
    );
    assert_eq!(
        listed(&harness.registry, Some(""), false),
        ["org.example.editor", "terminal"]
    );
    assert!(listed(&harness.registry, Some("nothing"), false).is_empty());
}

#[test]
fn list_is_sorted_by_id_regardless_of_scan_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(dir.path(), "zeta.desktop", &desktop("Zeta"));
    write_entry(dir.path(), "alpha.desktop", &desktop("Alpha"));
    write_entry(dir.path(), "nested/middle.desktop", &desktop("Middle"));
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);

    assert_eq!(
        listed(&harness.registry, None, true),
        ["alpha", "nested.middle", "zeta"]
    );
}

#[test]
fn get_and_contains_include_hidden_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(
        dir.path(),
        "hidden.desktop",
        &format!("{HEADER}Name=Hidden\nHidden=true\nTryExec=/usr/bin/true\n"),
    );
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);

    let info = harness
        .registry
        .get(&app_id("hidden"))
        .expect("hidden entry");
    assert_eq!(info.name, "Hidden");
    assert!(info.hidden);
    assert_eq!(info.try_exec.as_deref(), Some("/usr/bin/true"));
    assert!(harness.registry.contains(&app_id("hidden")));
    assert!(!harness.registry.contains(&app_id("missing")));
    assert_eq!(harness.registry.get(&app_id("missing")), None);
}

// --- launch -----------------------------------------------------------------

#[test]
fn launch_expands_field_codes_and_appends_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::entry(
        &dir,
        "app.desktop",
        "Name=Example\nExec=/usr/bin/example --flag %U %c %k %i\nIcon=example-icon\n",
    );
    let args = vec!["--extra".to_string(), "value".to_string()];

    let record = harness
        .registry
        .launch(&app_id("app"), &args, &LaunchEnv::new())
        .expect("launch");

    let command = harness.spawner.last().expect("a command was recorded");
    let desktop_path = dir.path().join("app.desktop");
    assert_eq!(
        argv(&command),
        [
            "/usr/bin/example",
            "--flag",
            "Example",
            desktop_path.to_str().expect("utf-8 fixture path"),
            "--icon",
            "example-icon",
            "--extra",
            "value",
        ]
    );
    assert!(command.env.is_empty());
    assert_eq!(
        (record.app_id, record.pid),
        (app_id("app"), Some(SPAWN_PID))
    );

    // The same command carries the LaunchEnv overrides when one is given.
    let env = LaunchEnv::new()
        .with_wayland_display("wayland-9")
        .with_xdg_runtime_dir("/run/user/1000")
        .with_var("GDK_BACKEND", "wayland");
    harness
        .registry
        .launch(&app_id("app"), &[], &env)
        .expect("launch with env");
    assert_eq!(
        harness.spawner.last().expect("a command was recorded").env,
        env.overrides()
    );
}

#[test]
fn launch_applies_launch_env_removals_to_the_spawn_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::entry(&dir, "app.desktop", "Name=Example\nExec=/usr/bin/example\n");
    let env = LaunchEnv::new()
        .with_wayland_display("wayland-5")
        .without("DISPLAY")
        .without("XAUTHORITY");

    harness
        .registry
        .launch(&app_id("app"), &[], &env)
        .expect("launch");

    let command = harness.spawner.last().expect("a command was recorded");
    assert_eq!(command.env, env.overrides());
    assert_eq!(command.env_remove, env.removals());
    // A plain launch records no removals.
    harness.launch("app").expect("launch without env");
    assert!(harness
        .spawner
        .last()
        .expect("a command was recorded")
        .env_remove
        .is_empty());
}

#[test]
fn launch_wraps_terminal_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(
        dir.path(),
        "term.desktop",
        &format!("{HEADER}Name=Term\nExec=/usr/bin/htop\nTerminal=true\n"),
    );
    let spawner = RecordingSpawner::new().shared();
    let registry = AppRegistry::with_options(
        RegistryOptions::with_search_dirs(vec![dir.path().to_path_buf()])
            .with_spawner(spawner.clone())
            .with_terminal(TerminalSpec::with_args(
                "kitty",
                vec!["--single-instance".to_string(), "-e".to_string()],
            )),
    );
    registry.scan().expect("scan");
    registry
        .launch(&app_id("term"), &[], &LaunchEnv::new())
        .expect("launch");

    assert_eq!(
        argv(&spawner.last().expect("a command was recorded")),
        ["kitty", "--single-instance", "-e", "/usr/bin/htop"]
    );
}

#[test]
fn launch_error_paths_never_spawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_entry(
        dir.path(),
        "tryexec.desktop",
        &format!("{HEADER}Name=Try\nExec=/usr/bin/app\nTryExec=/nonexistent/adesk-missing\n"),
    );
    write_entry(
        dir.path(),
        "noexec.desktop",
        &format!("{HEADER}Name=NoExec\n"),
    );
    write_entry(
        dir.path(),
        "dbus.desktop",
        &format!("{HEADER}Name=DBus\nDBusActivatable=true\n"),
    );
    write_entry(
        dir.path(),
        "empty.desktop",
        &format!("{HEADER}Name=Empty\nExec=%U\n"),
    );
    write_entry(
        dir.path(),
        "bad.desktop",
        &format!("{HEADER}Name=Bad\nExec=/usr/bin/app \"oops\n"),
    );
    let (harness, _) = Harness::scanned(&[dir.path().to_path_buf()]);

    let unknown = harness.launch("ghost").expect_err("unknown app");
    assert!(matches!(unknown, Error::UnknownApp(ref app) if app == &app_id("ghost")));

    let try_exec = harness.launch("tryexec").expect_err("try exec missing");
    assert!(matches!(
        try_exec,
        Error::TryExecNotFound { ref program } if program == "/nonexistent/adesk-missing"
    ));

    // No Exec, DBus-only, and an Exec that expands to nothing.
    for app in ["noexec", "dbus", "empty"] {
        let error = harness.launch(app).expect_err(app);
        assert!(matches!(error, Error::NoExec { .. }), "{app}: {error:?}");
    }

    let invalid = harness.launch("bad").expect_err("invalid exec");
    assert!(matches!(
        invalid,
        Error::InvalidExec {
            source: ExecError::UnterminatedQuote { .. },
            ..
        }
    ));

    assert!(harness.spawner.is_empty(), "error paths must not spawn");
}

#[test]
fn launch_allocates_monotonic_ids_and_reads_the_clock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::entry(&dir, "app.desktop", "Name=App\nExec=/usr/bin/app\n");

    harness.clock.set(1_234);
    let first = harness.launch("app").expect("first launch");
    harness
        .spawner
        .set_outcome(SpawnOutcome::Fail("mock spawn failure".to_string()));
    let failed = harness.launch("app").expect_err("spawn fails");
    assert!(matches!(failed, Error::Spawn(_)));
    harness
        .spawner
        .set_outcome(SpawnOutcome::Succeed(Some(SPAWN_PID)));
    harness.clock.set(987_654);
    let third = harness.launch("app").expect("third launch");

    assert_eq!(
        (first.launch_id, first.started_at_ms, first.pid),
        (LaunchId::from(1), 1_234, Some(SPAWN_PID))
    );
    // The failed spawn consumed id 2, so the next launch is 3.
    assert_eq!(
        (third.launch_id, third.started_at_ms),
        (LaunchId::from(3), 987_654)
    );
    assert_eq!(harness.spawner.len(), 3);

    // A spawner that reports no pid is passed through as `None`.
    let registry = AppRegistry::with_options(
        RegistryOptions::with_search_dirs(vec![dir.path().to_path_buf()])
            .with_spawner(RecordingSpawner::with_pid(None).shared()),
    );
    registry.scan().expect("scan");
    let record = registry
        .launch(&app_id("app"), &[], &LaunchEnv::new())
        .expect("launch");
    assert_eq!(record.pid, None);
}
