//! Fixtures: `.desktop` entries, an isolated data dir and the `adesk-test-app` helper.
//!
//! ADesk launch tests cannot rely on applications installed on the host. This module writes
//! XDG `.desktop` entries into a temp directory ([`FixtureDir`]) and describes the helper
//! process that turns such an entry into a real Wayland toplevel ([`TestAppSpec`] →
//! [`TestApp`]).
//!
//! # A fixture dir is a share root, not an applications dir
//!
//! `adesk-app-registry` appends `/applications` to every `XDG_DATA_DIRS` entry, so
//! [`FixtureDir`] owns a *share root*:
//!
//! - [`FixtureDir::path`] / [`FixtureDir::search_dir`] is what
//!   [`TestRuntimeConfig::with_app_dirs`](crate::TestRuntimeConfig::with_app_dirs) and
//!   [`TestRuntimeConfig::with_fixture_dir`](crate::TestRuntimeConfig::with_fixture_dir)
//!   expect;
//! - the entries themselves live in [`FixtureDir::applications_dir`] (`<root>/applications`),
//!   where [`FixtureDir::write_entry`] puts them.
//!
//! Passing `<root>/applications` as a search dir (or the share root as the applications dir)
//! silently yields an empty registry — always go through [`FixtureDir::search_dir`].
//!
//! # Locating the helper binary
//!
//! [`TestApp::spawn`] and [`TestAppSpec::desktop_entry`] resolve `adesk-test-app` at runtime
//! with [`helper_bin_path`]. Integration tests *of this package* may instead use
//! `env!("CARGO_BIN_EXE_adesk-test-app")`: Cargo defines that variable at compile time for a
//! package's own integration tests, benches and examples. Downstream crates cannot (Cargo
//! does not define it outside the owning package), which is exactly what [`helper_bin_path`]
//! is for.
//!
//! # Fixture arguments must not need shell quoting
//!
//! A `.desktop` `Exec` value is written as space-joined arguments and the registry tokenizes
//! it on whitespace and quotes, so fixture arguments must not contain spaces, quotes or
//! backslashes. There is no shell and no quoting layer between a fixture and the helper.
//!
//! # Phase status
//!
//! This is the architecture skeleton: the public signatures are final, the trivial
//! constructors/accessors/writers are implemented, and every behavior body that needs the
//! real Wayland path is `todo!()` with its exact Phase-2 semantics documented on the item.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use adesk_core::{AppId, Size};
use tempfile::TempDir;
use tokio::process::Child;

use crate::error::{Result, TestkitError};
use crate::fill::FillPattern;
use crate::runtime::TestRuntime;

/// File name of the helper binary [`TestApp`] spawns (see [`helper_bin_path`]).
const HELPER_APP: &str = "adesk-test-app";

/// An isolated `XDG_DATA_DIRS` *share root* holding `.desktop` fixtures.
///
/// Owns a `tempfile::TempDir` with an `applications` subdirectory and removes everything when
/// dropped. Create one per test that needs launchable applications and hand
/// [`FixtureDir::search_dir`] to the runtime configuration:
///
/// ```no_run
/// # use adesk_testkit::{FixtureDir, TestRuntime, TestRuntimeConfig};
/// # fn demo() -> adesk_testkit::Result<()> {
/// let fixtures = FixtureDir::new()?;
/// let config = TestRuntimeConfig::new().with_fixture_dir(&fixtures);
/// # let _ = (fixtures, config);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct FixtureDir {
    /// Temp directory acting as the `XDG_DATA_DIRS` share root.
    root: TempDir,
}

impl FixtureDir {
    /// Creates an empty share root with an `applications` subdirectory.
    ///
    /// The temp dir is private to this process and is deleted on drop.
    pub fn new() -> Result<FixtureDir> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("applications"))?;
        Ok(FixtureDir { root })
    }

    /// The share root (what the registry sees as one `XDG_DATA_DIRS` entry).
    pub fn path(&self) -> &Path {
        self.root.path()
    }

    /// The share root to pass to [`TestRuntimeConfig::with_app_dirs`].
    ///
    /// Identical to [`FixtureDir::path`]; named `search_dir` because that is the runtime's
    /// terminology for an `XDG_DATA_DIRS` entry.
    ///
    /// [`TestRuntimeConfig::with_app_dirs`]: crate::TestRuntimeConfig::with_app_dirs
    pub fn search_dir(&self) -> &Path {
        self.path()
    }

    /// `<share root>/applications`: where the registry looks for `.desktop` files.
    pub fn applications_dir(&self) -> PathBuf {
        self.path().join("applications")
    }

    /// Writes `entry` to `<share root>/applications/<file_stem>.desktop`.
    ///
    /// Returns the [`AppId`] the registry will report for that file. `file_stem` is the file
    /// name without the `.desktop` suffix and may contain `/` to create subdirectories; the
    /// id then joins the components with `.` — the rule of
    /// `adesk_app_registry::desktop_file_id` (the path relative to the applications dir, with
    /// `.desktop` stripped and `/` replaced by `.`), replicated here because that function's
    /// body is still a stub. The id is derived *before* the file is written, so an empty stem
    /// fails with [`TestkitError::Fixture`] without touching the filesystem.
    ///
    /// The contents come from [`DesktopEntryFixture::to_desktop_file`].
    pub fn write_entry(&self, file_stem: &str, entry: &DesktopEntryFixture) -> Result<AppId> {
        let file_name = format!("{file_stem}.desktop");
        let app_id = desktop_id_from_rel(&file_name).ok_or_else(|| {
            TestkitError::Fixture(format!(
                "desktop file stem {file_stem:?} does not yield a non-empty app id"
            ))
        })?;
        self.write_raw(
            Path::new("applications").join(&file_name),
            &entry.to_desktop_file(),
        )?;
        Ok(app_id)
    }

    /// Writes `contents` to `rel` under the share root and returns the absolute path.
    ///
    /// `rel` must be relative (an absolute path is rejected with [`TestkitError::Fixture`]);
    /// missing parent directories are created. Use this for fixture files that are not
    /// `.desktop` entries (icons, helper scripts); use [`FixtureDir::write_entry`] for entries
    /// so the returned [`AppId`] stays consistent with the registry.
    pub fn write_raw(&self, rel: impl AsRef<Path>, contents: &str) -> Result<PathBuf> {
        let rel = rel.as_ref();
        if rel.is_absolute() {
            return Err(TestkitError::Fixture(format!(
                "fixture path {} must be relative to the share root {}",
                rel.display(),
                self.path().display()
            )));
        }
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_at(parent, e))?;
        }
        std::fs::write(&path, contents).map_err(|e| io_at(&path, e))?;
        Ok(path)
    }

    /// Removes `<share root>/applications/<file_stem>.desktop`.
    ///
    /// Used to prove that a running registry stops offering an app after its entry is gone.
    /// Removing a file that does not exist is an error ([`TestkitError::Io`], kind
    /// `NotFound`) rather than a silent no-op.
    pub fn remove_entry(&self, file_stem: &str) -> Result<()> {
        let path = self.applications_dir().join(format!("{file_stem}.desktop"));
        std::fs::remove_file(&path).map_err(|e| io_at(&path, e))?;
        Ok(())
    }

    /// Convenience: writes the `.desktop` entry described by `spec`.
    ///
    /// Equivalent to `write_entry(spec.app_id().as_str(), &spec.desktop_entry()?)`: the entry
    /// id is the spec's app id, so a launch of the returned [`AppId`] runs the spec's helper
    /// process.
    pub fn write_app(&self, spec: &TestAppSpec) -> Result<AppId> {
        self.write_entry(spec.app_id().as_str(), &spec.desktop_entry()?)
    }
}

/// A `.desktop` entry to write into a [`FixtureDir`].
///
/// A plain data builder: [`DesktopEntryFixture::to_desktop_file`] is the only place the
/// serialization is defined, so tests can also assert on the exact file text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntryFixture {
    /// `Name` (the displayed name).
    pub name: String,
    /// `Exec` argument vector, written space-joined.
    pub exec: Vec<String>,
    /// `Icon` name or path.
    pub icon: Option<String>,
    /// `Terminal=true` when set.
    pub terminal: bool,
    /// `NoDisplay=true` when set.
    pub no_display: bool,
    /// `Hidden=true` when set.
    pub hidden: bool,
    /// `Categories`, written `;`-joined.
    pub categories: Vec<String>,
    /// `StartupWMClass`, used by the registry's window correlator.
    pub startup_wm_class: Option<String>,
    /// `TryExec` program.
    pub try_exec: Option<String>,
    /// `Comment`.
    pub comment: Option<String>,
    /// Verbatim `Key=Value` pairs written after the known keys, in order.
    pub extra: Vec<(String, String)>,
}

impl DesktopEntryFixture {
    /// A `Type=Application` entry with `name` and `exec` and everything else unset.
    ///
    /// `exec` is the argument vector; the first element is the program (relative names are
    /// resolved through `PATH` by the registry, so tests pass an absolute helper path).
    pub fn new(name: impl Into<String>, exec: impl IntoIterator<Item = impl Into<String>>) -> Self {
        DesktopEntryFixture {
            name: name.into(),
            exec: exec.into_iter().map(Into::into).collect(),
            icon: None,
            terminal: false,
            no_display: false,
            hidden: false,
            categories: Vec::new(),
            startup_wm_class: None,
            try_exec: None,
            comment: None,
            extra: Vec::new(),
        }
    }

    /// Sets `Icon`.
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Sets `Terminal` (written only when `true`).
    pub fn with_terminal(mut self, terminal: bool) -> Self {
        self.terminal = terminal;
        self
    }

    /// Sets `NoDisplay` (written only when `true`).
    pub fn with_no_display(mut self, no_display: bool) -> Self {
        self.no_display = no_display;
        self
    }

    /// Sets `Hidden` (written only when `true`).
    pub fn with_hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Appends one `Categories` value.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.categories.push(category.into());
        self
    }

    /// Sets `StartupWMClass`.
    pub fn with_startup_wm_class(mut self, class: impl Into<String>) -> Self {
        self.startup_wm_class = Some(class.into());
        self
    }

    /// Sets `TryExec`.
    pub fn with_try_exec(mut self, program: impl Into<String>) -> Self {
        self.try_exec = Some(program.into());
        self
    }

    /// Sets `Comment`.
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Appends a verbatim `key=value` line (written last, so it can also override a known
    /// key for tests that exercise duplicate-key handling).
    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }

    /// Serializes the entry.
    ///
    /// Exact layout (Phase 2 produces exactly this; the registry's parser must accept it):
    ///
    /// ```text
    /// [Desktop Entry]
    /// Type=Application
    /// Name=<name>
    /// Exec=<exec joined by single spaces>
    /// Icon=<icon>                     # only when Some
    /// Terminal=true                   # only when true
    /// NoDisplay=true                  # only when true
    /// Hidden=true                     # only when true
    /// Categories=<values joined by ;> # only when non-empty
    /// StartupWMClass=<class>          # only when Some
    /// TryExec=<program>               # only when Some
    /// Comment=<comment>               # only when Some
    /// <extra key>=<extra value>       # in insertion order, verbatim
    /// ```
    ///
    /// The text ends with a trailing newline and contains no blank lines. Values are written
    /// verbatim: `Exec` arguments must not contain whitespace or quoting characters (see the
    /// module docs), and `Categories` items must not contain `;`.
    pub fn to_desktop_file(&self) -> String {
        todo!("stub: implementation phase — layout documented above")
    }
}

/// Description of the helper process [`TestApp`] spawns for one fixture app.
///
/// Defaults: title = app id, `640x480`, [`FillPattern::default`], no exit deadline, no extra
/// arguments. [`TestAppSpec::cli_args`] is the single source of truth for the
/// `adesk-test-app` command line, and [`TestAppSpec::desktop_entry`] embeds exactly that
/// command line in an `Exec` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestAppSpec {
    /// The app id the toplevel reports and the `.desktop` file is named after.
    app_id: AppId,
    /// Window title (`--title`).
    title: String,
    /// Committed buffer size (`--size`).
    size: Size,
    /// Buffer fill (`--fill`).
    fill: FillPattern,
    /// Self-exit deadline (`--exit-after`), `None` to wait for the `exit` command.
    exit_after: Option<Duration>,
    /// Extra arguments appended verbatim after the standard flags.
    extra_args: Vec<String>,
}

impl TestAppSpec {
    /// A spec for `app_id` with the defaults documented on the struct.
    ///
    /// `app_id` is both the Wayland app id and the default title, so a launch test can assert
    /// on the window without further setup.
    pub fn new(app_id: impl Into<String>) -> Self {
        let app_id = AppId::from(app_id.into());
        TestAppSpec {
            title: app_id.as_str().to_string(),
            app_id,
            size: Size::new(640, 480),
            fill: FillPattern::default(),
            exit_after: None,
            extra_args: Vec::new(),
        }
    }

    /// Overrides the window title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Overrides the committed buffer size.
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Overrides the buffer fill pattern (must be opaque, see [`FillPattern::require_opaque`]).
    pub fn with_fill(mut self, fill: FillPattern) -> Self {
        self.fill = fill;
        self
    }

    /// Makes the helper exit on its own after `after`.
    ///
    /// Encoded as whole milliseconds (truncated) in `--exit-after`.
    pub fn with_exit_after(mut self, after: Duration) -> Self {
        self.exit_after = Some(after);
        self
    }

    /// Appends an extra helper argument, verbatim, after the standard flags.
    ///
    /// The stock `adesk-test-app` rejects unknown arguments with exit code 64, so this only
    /// makes sense for a helper that understands the flag.
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.extra_args.push(arg.into());
        self
    }

    /// The app id this spec's toplevel reports.
    pub fn app_id(&self) -> &AppId {
        &self.app_id
    }

    /// The helper's argument vector (without `argv[0]`).
    ///
    /// Exactly the CLI grammar of `adesk-test-app`:
    ///
    /// ```text
    /// --app-id <ID> --title <TITLE> --size <W>x<H> --fill <PATTERN> [--exit-after <MS>] [extra...]
    /// ```
    ///
    /// `--app-id`, `--title`, `--size` and `--fill` are always present (the helper's defaults
    /// are never relied upon); `--exit-after` carries the whole milliseconds of
    /// [`TestAppSpec::with_exit_after`]; `--fill` uses the [`FillPattern::to_cli_arg`] grammar.
    pub fn cli_args(&self) -> Vec<String> {
        let mut args = vec![
            "--app-id".to_string(),
            self.app_id.as_str().to_string(),
            "--title".to_string(),
            self.title.clone(),
            "--size".to_string(),
            format!("{}x{}", self.size.w, self.size.h),
            "--fill".to_string(),
            self.fill.to_cli_arg(),
        ];
        if let Some(after) = self.exit_after {
            args.push("--exit-after".to_string());
            args.push(after.as_millis().to_string());
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }

    /// The `.desktop` entry that launches this spec.
    ///
    /// `Exec` is `[helper_bin_path("adesk-test-app")?, ...cli_args()]`, `StartupWMClass` is
    /// the app id (so pid-less launches still correlate) and `Name` is the title. The helper
    /// path must be UTF-8, otherwise [`TestkitError::Fixture`] is returned — a `.desktop`
    /// file cannot carry a non-UTF-8 path.
    pub fn desktop_entry(&self) -> Result<DesktopEntryFixture> {
        let helper = helper_bin_path(HELPER_APP)?;
        let helper = helper.to_str().ok_or_else(|| {
            TestkitError::Fixture(format!(
                "helper binary path {} is not valid UTF-8",
                helper.display()
            ))
        })?;
        let mut exec = vec![helper.to_string()];
        exec.extend(self.cli_args());
        let mut entry = DesktopEntryFixture::new(self.title.clone(), exec);
        entry.startup_wm_class = Some(self.app_id.as_str().to_string());
        Ok(entry)
    }
}

/// A spawned `adesk-test-app` helper process.
///
/// Created by [`TestApp::spawn`]; stop it with [`TestApp::exit`] (graceful) or
/// [`TestApp::kill`]. The child is killed when the handle is dropped, so a panicking test
/// never leaves a helper running.
#[derive(Debug)]
pub struct TestApp {
    /// The helper process.
    child: Child,
    /// App id the helper was started with.
    app_id: AppId,
}

impl TestApp {
    /// Spawns `adesk-test-app` for `spec` against `runtime`'s Wayland socket.
    ///
    /// Phase-2 semantics (exactly this):
    ///
    /// 1. program = [`helper_bin_path`]`("adesk-test-app")`, args = [`TestAppSpec::cli_args`];
    /// 2. environment: `WAYLAND_DISPLAY` = [`TestRuntime::wayland_display`],
    ///    `XDG_RUNTIME_DIR` = [`TestRuntime::env`]`().runtime_dir()` (the helper must never
    ///    inherit the host's values);
    /// 3. `stdin` piped (the `exit` command channel), `stdout`/`stderr` inherited so helper
    ///    diagnostics show up in the test output;
    /// 4. `kill_on_drop(true)`, then spawn; the returned handle owns the child and the app id
    ///    from `spec`.
    ///
    /// Spawning itself does not wait for the toplevel to appear — use
    /// [`TestRuntime::wait_for_window_app`] for that.
    pub async fn spawn(runtime: &TestRuntime, spec: &TestAppSpec) -> Result<TestApp> {
        let _ = (runtime, spec);
        todo!("stub: implementation phase — semantics documented above")
    }

    /// The app id the helper was started with.
    pub fn app_id(&self) -> &AppId {
        &self.app_id
    }

    /// The helper's process id, or `None` once it has been reaped.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Whether the helper is still running.
    ///
    /// Phase-2 semantics: `tokio::process::Child::try_wait` — `Ok(None)` means running,
    /// `Ok(Some(_))` means it exited and the status was reaped (tokio caches it, so a later
    /// [`TestApp::wait_for_exit`] still returns it).
    pub fn is_running(&mut self) -> Result<bool> {
        todo!("stub: implementation phase — try_wait semantics documented above")
    }

    /// Waits up to `timeout` for the helper to exit.
    ///
    /// Phase-2 semantics: bounded `Child::wait`; on expiry the child is killed and
    /// [`TestkitError::Timeout`] is returned with `what = "adesk-test-app exit"` (a killed
    /// helper reports no status here — that is what makes a stuck helper a test failure).
    pub async fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let _ = timeout;
        todo!("stub: implementation phase — bounded wait semantics documented above")
    }

    /// Asks the helper to exit gracefully and waits (bounded) for it.
    ///
    /// Phase-2 semantics: write `"exit\n"` to the child's stdin and close the pipe (the
    /// helper's graceful command), then wait up to 5 s. If the deadline expires the child is
    /// killed and the resulting signal status is returned, so a helper that ignored `exit`
    /// fails the caller's `status.success()` assertion instead of hanging the test.
    pub async fn exit(mut self) -> Result<ExitStatus> {
        let _ = &mut self;
        todo!("stub: implementation phase — graceful-exit semantics documented above")
    }

    /// Kills the helper without waiting for it.
    ///
    /// Phase-2 semantics: `tokio::process::Child::start_kill` (SIGKILL on Unix). The OS error
    /// is surfaced as [`TestkitError::Io`] — including the error reported when the child has
    /// already exited.
    pub fn kill(&mut self) -> Result<()> {
        todo!("stub: implementation phase — start_kill semantics documented above")
    }
}

/// Resolves a helper binary shipped by this package (e.g. `adesk-test-app`).
///
/// Phase-2 semantics — the first existing regular file wins, in this order:
///
/// 1. `<dir>/<name>`, where `<dir>` is the directory of `std::env::current_exe()`;
/// 2. `<dir>/../<name>` — a test binary lives in `target/debug/deps/`, the helper in
///    `target/debug/` one level up;
/// 3. `<name>` joined onto every `$PATH` entry (empty entries are skipped, so the current
///    directory is never searched implicitly); `$PATH` is not searched when unset.
///
/// Failure: [`TestkitError::HelperNotFound`] with `searched_from` = `<dir>`;
/// [`TestkitError::Io`] when `current_exe()` itself fails.
///
/// Downstream crates need this because Cargo's `CARGO_BIN_EXE_<name>` is defined only for the
/// package that declares the `[[bin]]`; this package's own integration tests may use
/// `env!("CARGO_BIN_EXE_adesk-test-app")` at compile time instead.
pub fn helper_bin_path(name: &str) -> Result<PathBuf> {
    let _ = name;
    todo!("stub: implementation phase — search order documented above")
}

/// Derives an app id from a path relative to the applications dir.
///
/// Mirrors `adesk_app_registry::desktop_file_id`: drop the `.desktop` suffix and replace `/`
/// with `.`. Returns `None` when the remainder is empty or the suffix is missing. Replicated
/// (not called) because the registry function's body is still `todo!()`; keep the two in sync
/// and switch to it once it lands.
fn desktop_id_from_rel(rel: &str) -> Option<AppId> {
    let stem = rel.strip_suffix(".desktop")?;
    if stem.is_empty() {
        return None;
    }
    Some(AppId::from(stem.replace('/', ".")))
}

/// Wraps an IO error with the path it happened on, preserving the error kind.
fn io_at(path: &Path, error: std::io::Error) -> TestkitError {
    TestkitError::Io(std::io::Error::new(
        error.kind(),
        format!("{}: {error}", path.display()),
    ))
}

/// Unit tests for the parts of this module that are implemented in Phase 1.
///
/// Everything that needs the real Wayland path (`to_desktop_file`, `helper_bin_path`,
/// `TestApp`) stays untested until its body lands; `TestAppSpec::cli_args` also depends on
/// `FillPattern::to_cli_arg` and therefore on Phase 2.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_dir_layout_is_a_share_root() {
        let dir = FixtureDir::new().expect("temp dir");
        assert_eq!(dir.path(), dir.search_dir());
        assert_eq!(dir.applications_dir(), dir.path().join("applications"));
        assert!(dir.applications_dir().is_dir());
    }

    #[test]
    fn write_raw_creates_parents_and_rejects_absolute_paths() {
        let dir = FixtureDir::new().expect("temp dir");
        let path = dir
            .write_raw("icons/hicolor/48x48/app.png", "png")
            .expect("write");
        assert_eq!(path, dir.path().join("icons/hicolor/48x48/app.png"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "png");

        let error = dir
            .write_raw("/tmp/adesk-escape", "x")
            .expect_err("absolute");
        assert!(matches!(error, TestkitError::Fixture(_)), "{error}");
        assert!(!dir.path().join("tmp").exists());
    }

    #[test]
    fn desktop_ids_follow_the_registry_rule() {
        let id = |rel: &str| desktop_id_from_rel(rel).map(|id| id.as_str().to_string());
        assert_eq!(
            id("org.mozilla.firefox.desktop").as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(id("code.desktop").as_deref(), Some("code"));
        assert_eq!(id("kde/kate.desktop").as_deref(), Some("kde.kate"));
        assert_eq!(id("foo/bar.baz.desktop").as_deref(), Some("foo.bar.baz"));
        assert_eq!(id("no-extension"), None);
        assert_eq!(id(".desktop"), None);
    }

    #[test]
    fn remove_entry_targets_the_applications_dir() {
        let dir = FixtureDir::new().expect("temp dir");
        let path = dir
            .write_raw("applications/demo.desktop", "[Desktop Entry]\n")
            .expect("write");
        dir.remove_entry("demo").expect("remove");
        assert!(!path.exists());
        let error = dir.remove_entry("demo").expect_err("removed twice");
        assert!(matches!(error, TestkitError::Io(_)), "{error}");
    }

    #[test]
    fn desktop_entry_fixture_builders_set_fields() {
        let entry = DesktopEntryFixture::new("Demo", ["adesk-test-app", "--app-id", "demo"])
            .with_icon("demo")
            .with_terminal(true)
            .with_no_display(true)
            .with_hidden(true)
            .with_category("Utility")
            .with_category("Test")
            .with_startup_wm_class("demo")
            .with_try_exec("adesk-test-app")
            .with_comment("fixture")
            .with_extra("X-Testkit", "1");
        assert_eq!(entry.name, "Demo");
        assert_eq!(entry.exec, ["adesk-test-app", "--app-id", "demo"]);
        assert_eq!(entry.icon.as_deref(), Some("demo"));
        assert!(entry.terminal && entry.no_display && entry.hidden);
        assert_eq!(entry.categories, ["Utility", "Test"]);
        assert_eq!(entry.startup_wm_class.as_deref(), Some("demo"));
        assert_eq!(entry.try_exec.as_deref(), Some("adesk-test-app"));
        assert_eq!(entry.comment.as_deref(), Some("fixture"));
        assert_eq!(entry.extra, [("X-Testkit".to_string(), "1".to_string())]);

        let defaults = DesktopEntryFixture::new("Demo", ["adesk-test-app"]);
        assert!(defaults.icon.is_none() && defaults.categories.is_empty());
        assert!(!defaults.terminal && !defaults.no_display && !defaults.hidden);
    }

    #[test]
    fn test_app_spec_defaults_and_builders() {
        let spec = TestAppSpec::new("org.example.demo");
        assert_eq!(spec.app_id().as_str(), "org.example.demo");
        assert_eq!(
            spec,
            TestAppSpec {
                app_id: AppId::from("org.example.demo"),
                title: "org.example.demo".to_string(),
                size: Size::new(640, 480),
                fill: FillPattern::default(),
                exit_after: None,
                extra_args: Vec::new(),
            }
        );

        let built = TestAppSpec::new("org.example.other")
            .with_title("Other")
            .with_size(Size::new(320, 200))
            .with_fill(FillPattern::solid_rgb(1, 2, 3))
            .with_exit_after(Duration::from_millis(250))
            .with_arg("--verbose");
        assert_eq!(
            built,
            TestAppSpec {
                app_id: AppId::from("org.example.other"),
                title: "Other".to_string(),
                size: Size::new(320, 200),
                fill: FillPattern::solid_rgb(1, 2, 3),
                exit_after: Some(Duration::from_millis(250)),
                extra_args: vec!["--verbose".to_string()],
            }
        );
    }
}
