//! The application registry: discovery, listing, description and launch.
//!
//! [`AppRegistry`] is the only stateful public type of this crate. It is
//! `Send + Sync`, cheap to share as `Arc<AppRegistry>`, and its methods take
//! `&self`: [`AppRegistry::scan`] rebuilds the entry table behind an internal
//! `RwLock` so the server can rescan without dropping the shared handle.
//!
//! `adesk-server` translates the AGP registry methods onto this type:
//! `list_apps` → [`AppRegistry::list`], `get_app` → [`AppRegistry::get`],
//! `launch_app` → [`AppRegistry::launch`]. Event emission and window stamping stay
//! in the server (see `CONTEXT.md` → "What adesk-server must know").

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use adesk_core::{AppId, AppInfo, LaunchId};

use crate::app_id::{desktop_file_id, is_desktop_file};
use crate::clock::{Clock, MonotonicClock};
use crate::error::{Error, Result};
use crate::exec::{ExecContext, ExecExpander};
use crate::launch::{CommandSpawner, LaunchEnv, LaunchRecord, ProcessSpawner, SpawnCommand};
use crate::parser::{parse_str, DesktopEntry, EntryError};
use crate::terminal::TerminalSpec;
use crate::try_exec::resolve_try_exec;

/// Environment variables consulted for the locale, in precedence order.
const LOCALE_VARS: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];

/// First non-empty value of [`LOCALE_VARS`], if any.
fn locale_from_env() -> Option<String> {
    LOCALE_VARS
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
}

/// Configuration for an [`AppRegistry`].
#[derive(Debug, Clone)]
pub struct RegistryOptions {
    /// Search directories in precedence order; the first occurrence of a
    /// desktop-file id wins.
    pub search_dirs: Vec<PathBuf>,
    /// Terminal wrapper for `Terminal=true` entries.
    pub terminal: TerminalSpec,
    /// Process spawner; tests inject a mock.
    pub spawner: Arc<dyn ProcessSpawner>,
    /// Monotonic clock; share one instance with the [`crate::Correlator`].
    pub clock: Arc<dyn Clock>,
    /// Locale used to resolve `Name[locale]` keys (e.g. `de_DE.UTF-8`).
    pub locale: Option<String>,
}

impl RegistryOptions {
    /// Production defaults: XDG search directories ([`crate::search_dirs`]),
    /// `$TERMINAL`-aware terminal spec, [`crate::CommandSpawner`], a fresh
    /// [`crate::MonotonicClock`] and the locale from `LC_ALL`/`LC_MESSAGES`/`LANG`.
    pub fn xdg() -> RegistryOptions {
        RegistryOptions {
            search_dirs: crate::search_dirs(),
            terminal: TerminalSpec::from_env(),
            spawner: Arc::new(CommandSpawner::new()),
            clock: Arc::new(MonotonicClock::new()),
            locale: locale_from_env(),
        }
    }

    /// Explicit search directories with the other production defaults.
    pub fn with_search_dirs<I>(dirs: I) -> RegistryOptions
    where
        I: IntoIterator<Item = PathBuf>,
    {
        RegistryOptions {
            search_dirs: dirs.into_iter().collect(),
            ..RegistryOptions::xdg()
        }
    }

    /// Replaces the process spawner.
    pub fn with_spawner(mut self, spawner: Arc<dyn ProcessSpawner>) -> RegistryOptions {
        self.spawner = spawner;
        self
    }

    /// Replaces the clock.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> RegistryOptions {
        self.clock = clock;
        self
    }

    /// Replaces the terminal wrapper.
    pub fn with_terminal(mut self, terminal: TerminalSpec) -> RegistryOptions {
        self.terminal = terminal;
        self
    }

    /// Replaces the locale used for localized names.
    pub fn with_locale(mut self, locale: Option<String>) -> RegistryOptions {
        self.locale = locale;
        self
    }
}

/// One non-fatal problem found while scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    /// The file (or directory) that could not be used.
    pub path: PathBuf,
    /// Human-readable reason.
    pub reason: String,
}

/// Outcome of [`AppRegistry::scan`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Directories that existed and were walked.
    pub dirs_scanned: usize,
    /// `.desktop` files that were read.
    pub files_scanned: usize,
    /// Entries kept (including `Hidden`/`NoDisplay` ones).
    pub apps: usize,
    /// Files skipped because `Type != Application`.
    pub skipped: usize,
    /// Non-fatal parse/read problems; a scan never fails because one file is bad.
    pub issues: Vec<ScanIssue>,
}

/// The registry: discovered applications, listing and launch.
///
/// Entries are deduplicated by desktop-file id with the first search directory
/// winning, so a `Hidden=true` entry in a higher-precedence directory shadows the
/// same id in lower-precedence ones (it stays in the table and is filtered by
/// [`AppRegistry::list`] unless `include_hidden` is set).
#[derive(Debug)]
pub struct AppRegistry {
    options: RegistryOptions,
    /// Entry table, keyed by desktop-file id; swapped wholesale by
    /// [`AppRegistry::scan`] so readers never observe a half-built table.
    entries: RwLock<BTreeMap<AppId, DesktopEntry>>,
    /// Next [`LaunchId`] to hand out; monotonic from 1, never reused.
    next_launch_id: AtomicU64,
}

impl AppRegistry {
    /// Empty registry with production defaults ([`RegistryOptions::xdg`]).
    ///
    /// Call [`AppRegistry::scan`] before serving requests.
    pub fn new() -> AppRegistry {
        AppRegistry::with_options(RegistryOptions::xdg())
    }

    /// Empty registry with explicit options.
    pub fn with_options(options: RegistryOptions) -> AppRegistry {
        AppRegistry {
            options,
            entries: RwLock::new(BTreeMap::new()),
            next_launch_id: AtomicU64::new(1),
        }
    }

    /// The options this registry was built with.
    pub fn options(&self) -> &RegistryOptions {
        &self.options
    }

    /// The search directories in precedence order.
    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.options.search_dirs
    }

    /// Rebuilds the entry table by scanning the search directories recursively.
    ///
    /// Walks each directory in precedence order, reading every `*.desktop` file
    /// (skipping hidden files and directories, and not following directory
    /// symlinks). Files that fail to read or parse, or that lack `Name`/`Type`,
    /// become [`ScanIssue`]s; `Type != Application` files are counted in
    /// [`ScanReport::skipped`]. The table is replaced atomically when the scan
    /// finishes, so a failed or partial scan never leaves the registry in a
    /// half-built state. Only catastrophic I/O on the registry itself returns
    /// `Err`.
    pub fn scan(&self) -> Result<ScanReport> {
        let mut report = ScanReport::default();
        let mut table: BTreeMap<AppId, DesktopEntry> = BTreeMap::new();

        for dir in &self.options.search_dirs {
            let Some(files) = collect_desktop_files(dir, &mut report.issues) else {
                // Missing or unreadable search directories are normal for XDG
                // locations; unreadable ones already produced an issue.
                continue;
            };
            report.dirs_scanned += 1;

            for path in files {
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(source) => {
                        report.issues.push(ScanIssue {
                            path,
                            reason: format!("cannot read file: {source}"),
                        });
                        continue;
                    }
                };
                report.files_scanned += 1;

                let raw = match parse_str(&text) {
                    Ok(raw) => raw,
                    Err(error) => {
                        report.issues.push(ScanIssue {
                            path,
                            reason: error.to_string(),
                        });
                        continue;
                    }
                };
                let Some(id) = desktop_file_id(dir, &path) else {
                    report.issues.push(ScanIssue {
                        path,
                        reason: "cannot derive a desktop-file id".to_string(),
                    });
                    continue;
                };

                match DesktopEntry::from_raw(
                    id.clone(),
                    path.clone(),
                    &raw,
                    self.options.locale.as_deref(),
                ) {
                    // First search directory wins: an already-known id shadows
                    // this file instead of replacing the higher-precedence entry.
                    Ok(entry) => {
                        table.entry(id).or_insert(entry);
                    }
                    Err(EntryError::NotApplication(_)) => report.skipped += 1,
                    Err(error) => report.issues.push(ScanIssue {
                        path,
                        reason: error.to_string(),
                    }),
                }
            }
        }

        report.apps = table.len();
        // The whole table is built outside the lock and swapped in once, so
        // concurrent readers see either the old or the new registry, never a
        // partially populated one.
        *self
            .entries
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = table;

        for issue in &report.issues {
            tracing::warn!(
                path = %issue.path.display(),
                reason = %issue.reason,
                "desktop entry skipped during scan"
            );
        }
        tracing::debug!(
            dirs = report.dirs_scanned,
            files = report.files_scanned,
            apps = report.apps,
            skipped = report.skipped,
            issues = report.issues.len(),
            "application registry scanned"
        );

        Ok(report)
    }

    /// Lists applications, sorted by id (BTreeMap order).
    ///
    /// `query` matches case-insensitively as a substring of the desktop-file id or
    /// the localized name. `include_hidden` controls whether entries with
    /// `Hidden=true` or `NoDisplay=true` are included (`docs/architecture.md` §7).
    pub fn list(&self, query: Option<&str>, include_hidden: bool) -> Vec<AppInfo> {
        let query = query.map(str::to_lowercase);
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner());

        entries
            .values()
            .filter(|entry| entry.is_listable(include_hidden))
            .filter(|entry| match &query {
                None => true,
                Some(query) => {
                    entry.id.as_str().to_lowercase().contains(query)
                        || entry.name.to_lowercase().contains(query)
                }
            })
            .map(DesktopEntry::to_app_info)
            .collect()
    }

    /// Looks up one application, including hidden/`NoDisplay` entries — filtering
    /// is a listing concern, so an explicitly requested id is always returned.
    pub fn get(&self, id: &AppId) -> Option<AppInfo> {
        self.entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .map(DesktopEntry::to_app_info)
    }

    /// `true` when `id` is known to the registry (hidden entries included).
    pub fn contains(&self, id: &AppId) -> bool {
        self.entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains_key(id)
    }

    /// Launches an application and returns its [`LaunchRecord`].
    ///
    /// Pipeline (AGP §5.2, `docs/architecture.md` §7):
    /// 1. unknown id → [`crate::Error::UnknownApp`];
    /// 2. `TryExec` set and unresolvable → [`crate::Error::TryExecNotFound`];
    /// 3. `Exec` expanded with [`crate::ExecExpander`] (empty `files` context);
    ///    a missing/empty result → [`crate::Error::NoExec`] — this is also the
    ///    `DBusActivatable` fallback rule: v1 attempts `Exec` and only reports
    ///    `not_supported` when there is no usable `Exec` line;
    /// 4. `args` are appended after expansion;
    /// 5. `Terminal=true` wraps the command with [`TerminalSpec`];
    /// 6. [`LaunchEnv::overrides`] is applied to the command's environment;
    /// 7. the launch id is allocated (monotonic from 1, consumed even on failure)
    ///    and `started_at_ms` is read from the clock;
    /// 8. [`ProcessSpawner::spawn`] runs the command; OS failures →
    ///    [`crate::Error::Spawn`] (AGP `launch_failed`).
    ///
    /// This method does **not** emit events or register with the
    /// [`crate::Correlator`]; the server does both with the returned record.
    pub fn launch(&self, id: &AppId, args: &[String], env: &LaunchEnv) -> Result<LaunchRecord> {
        let entry = self
            .entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| Error::UnknownApp(id.clone()))?;

        // 2. Availability check: an unresolvable TryExec is reported at launch
        //    time (the entry stays listed, see `docs/architecture.md` §7).
        if let Some(program) = entry.try_exec.as_deref() {
            if resolve_try_exec(program).is_none() {
                return Err(Error::TryExecNotFound {
                    program: program.to_string(),
                });
            }
        }

        // 3. Field-code expansion. AGP carries no file arguments, so `files` is
        //    empty and `%f/%F/%u/%U` expand to nothing.
        let context = ExecContext {
            name: &entry.name,
            icon: entry.icon.as_deref(),
            desktop_file: Some(entry.path.as_path()),
            files: &[],
        };
        let mut argv = match entry.exec.as_deref() {
            Some(exec) => ExecExpander::new()
                .expand(exec, &context)
                .map_err(|source| Error::InvalidExec {
                    app: entry.id.clone(),
                    source,
                })?,
            None => Vec::new(),
        };
        if argv.is_empty() {
            // Also the DBus-only fallback rule: no usable Exec means not_supported.
            return Err(Error::NoExec {
                app: entry.id.clone(),
            });
        }

        // 4. Caller-supplied arguments come after the expanded Exec line.
        argv.extend(args.iter().cloned());

        // 5. Terminal entries run the whole command inside the terminal.
        let mut command = if entry.terminal {
            self.options
                .terminal
                .wrap(&argv)
                .ok_or_else(|| Error::NoExec {
                    app: entry.id.clone(),
                })?
        } else {
            // `argv` is non-empty here, so the program is always available; the
            // `None` arm keeps this path panic-free.
            let Some((program, rest)) = argv.split_first() else {
                return Err(Error::NoExec {
                    app: entry.id.clone(),
                });
            };
            SpawnCommand {
                program: program.clone(),
                args: rest.to_vec(),
                env: Vec::new(),
            }
        };

        // 6. Environment overrides are layered on the inherited environment.
        command.env = env.overrides();

        // 7. The id is allocated even when the spawn fails, so ids are never
        //    reused; the timestamp comes from the injected clock.
        let launch_id = LaunchId::from(self.next_launch_id.fetch_add(1, Ordering::SeqCst));
        let started_at_ms = self.options.clock.now_ms();

        // 8. Spawn. Reaping the child is the server's responsibility.
        let spawned = self.options.spawner.spawn(&command)?;
        let record = LaunchRecord {
            launch_id,
            app_id: entry.id,
            pid: spawned.pid,
            started_at_ms,
        };

        tracing::debug!(
            app = %record.app_id,
            launch_id = %record.launch_id,
            pid = ?record.pid,
            "application launched"
        );

        Ok(record)
    }

    /// Number of known entries.
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    /// `true` when no entries are known (e.g. before [`AppRegistry::scan`]).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Recursively collects `*.desktop` files below `dir`.
///
/// Hidden names (leading `.`) are skipped, directory symlinks are **not**
/// followed (cycle safety) while file symlinks are read normally, and
/// non-`.desktop` files are ignored. Problems reading a directory or entry are
/// pushed onto `issues`; the walk never aborts on one bad entry.
///
/// Returns `None` when the root directory itself does not exist or cannot be
/// read (a missing XDG directory is not an issue), and `Some(files)` otherwise —
/// `files` is empty when the directory holds no usable entry.
fn collect_desktop_files(dir: &Path, issues: &mut Vec<ScanIssue>) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut root_walked = false;

    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            // A search directory that does not exist is normal, not an issue.
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                issues.push(ScanIssue {
                    path: current,
                    reason: format!("cannot read directory: {source}"),
                });
                continue;
            }
        };
        if current.as_path() == dir {
            root_walked = true;
        }

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    issues.push(ScanIssue {
                        path: current.clone(),
                        reason: format!("cannot read directory entry: {source}"),
                    });
                    continue;
                }
            };
            if is_hidden(&entry.file_name()) {
                continue;
            }

            let path = entry.path();
            match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => stack.push(path),
                Ok(file_type) if file_type.is_file() => {
                    if is_desktop_file(&path) {
                        files.push(path);
                    }
                }
                // A symlink (or other special file): resolve it by hand so a
                // directory symlink is skipped instead of walked.
                Ok(_) => {
                    if !is_desktop_file(&path) {
                        continue;
                    }
                    match std::fs::metadata(&path) {
                        Ok(metadata) if metadata.is_file() => files.push(path),
                        Ok(_) => {}
                        Err(source) => issues.push(ScanIssue {
                            path,
                            reason: format!("cannot read file: {source}"),
                        }),
                    }
                }
                Err(source) => issues.push(ScanIssue {
                    path,
                    reason: format!("cannot stat directory entry: {source}"),
                }),
            }
        }
    }

    root_walked.then_some(files)
}

/// `true` when the file name starts with `.` (hidden per the XDG spec).
fn is_hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::exec::ExecError;
    use crate::launch::{SpawnError, SpawnedProcess};

    const HEADER: &str = "[Desktop Entry]\nType=Application\n";

    fn desktop(name: &str) -> String {
        format!("{HEADER}Name={name}\n")
    }

    fn id(value: &str) -> AppId {
        AppId::from(value)
    }

    /// Writes `<dir>/<relative>` (creating parents) and returns the path.
    fn write(dir: &Path, relative: &str, contents: &str) -> PathBuf {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directories");
        }
        std::fs::write(&path, contents).expect("write fixture file");
        path
    }

    /// Ids of a listing, for order-sensitive assertions.
    fn listed(fixture: &Fixture, query: Option<&str>, include_hidden: bool) -> Vec<String> {
        fixture
            .registry
            .list(query, include_hidden)
            .iter()
            .map(|app| app.id.0.clone())
            .collect()
    }

    fn argv(command: &SpawnCommand) -> Vec<&str> {
        std::iter::once(command.program.as_str())
            .chain(command.args.iter().map(String::as_str))
            .collect()
    }

    /// Spawner double: records every command and can be switched to failing.
    #[derive(Debug)]
    struct RecordingSpawner {
        commands: Mutex<Vec<SpawnCommand>>,
        fail: AtomicBool,
        pid: Option<i32>,
    }

    impl RecordingSpawner {
        fn spawner() -> Arc<RecordingSpawner> {
            RecordingSpawner::with_pid(Some(4242))
        }

        fn with_pid(pid: Option<i32>) -> Arc<RecordingSpawner> {
            Arc::new(RecordingSpawner {
                commands: Mutex::new(Vec::new()),
                fail: AtomicBool::new(false),
                pid,
            })
        }

        fn set_fail(&self, fail: bool) {
            self.fail.store(fail, Ordering::SeqCst);
        }

        fn count(&self) -> usize {
            self.commands.lock().expect("spawner lock").len()
        }

        fn last(&self) -> SpawnCommand {
            self.commands().pop().expect("a command was recorded")
        }

        fn commands(&self) -> Vec<SpawnCommand> {
            self.commands.lock().expect("spawner lock").clone()
        }
    }

    impl ProcessSpawner for RecordingSpawner {
        fn spawn(&self, command: &SpawnCommand) -> std::result::Result<SpawnedProcess, SpawnError> {
            self.commands
                .lock()
                .expect("spawner lock")
                .push(command.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(SpawnError::Other("mock spawn failure".to_string()));
            }
            Ok(SpawnedProcess::new(self.pid))
        }
    }

    /// Clock double with a manually controlled timestamp.
    #[derive(Debug)]
    struct FakeClock(AtomicU64);

    impl FakeClock {
        fn at(ms: u64) -> Arc<FakeClock> {
            Arc::new(FakeClock(AtomicU64::new(ms)))
        }

        fn set(&self, ms: u64) {
            self.0.store(ms, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Registry plus its injected doubles.
    struct Fixture {
        registry: AppRegistry,
        spawner: Arc<RecordingSpawner>,
        clock: Arc<FakeClock>,
    }

    impl Fixture {
        fn with_locale(dirs: &[PathBuf], locale: Option<&str>) -> Fixture {
            let spawner = RecordingSpawner::spawner();
            let clock = FakeClock::at(0);
            let options = RegistryOptions::with_search_dirs(dirs.iter().cloned())
                .with_spawner(spawner.clone())
                .with_clock(clock.clone())
                .with_locale(locale.map(str::to_string));
            Fixture {
                registry: AppRegistry::with_options(options),
                spawner,
                clock,
            }
        }

        fn new(dirs: &[PathBuf]) -> Fixture {
            Fixture::with_locale(dirs, None)
        }

        fn scanned(dirs: &[PathBuf]) -> (Fixture, ScanReport) {
            let fixture = Fixture::new(dirs);
            let report = fixture.registry.scan().expect("scan succeeds");
            (fixture, report)
        }

        /// Scans a temp dir holding one entry whose body follows [`HEADER`].
        fn entry(dir: &TempDir, relative: &str, body: &str) -> Fixture {
            write(dir.path(), relative, &format!("{HEADER}{body}"));
            Fixture::scanned(&[dir.path().to_path_buf()]).0
        }

        fn launch(&self, app: &str) -> Result<LaunchRecord> {
            self.registry.launch(&id(app), &[], &LaunchEnv::new())
        }
    }

    // --- options -----------------------------------------------------------

    #[test]
    fn xdg_options_use_production_defaults() {
        let options = RegistryOptions::xdg();
        let expected_locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.is_empty());

        assert_eq!(options.search_dirs, crate::search_dirs());
        assert_eq!(options.terminal, TerminalSpec::from_env());
        assert_eq!(options.locale, expected_locale);
        assert!(format!("{:?}", options.spawner).contains("CommandSpawner"));
        assert!(format!("{:?}", options.clock).contains("MonotonicClock"));

        // Explicit directories keep every other production default.
        let dirs = vec![
            PathBuf::from("/tmp/adesk-high"),
            PathBuf::from("/tmp/adesk-low"),
        ];
        let explicit = RegistryOptions::with_search_dirs(dirs.clone());
        assert_eq!(explicit.search_dirs, dirs);
        assert_eq!(explicit.terminal, options.terminal);
        assert_eq!(explicit.locale, options.locale);
        assert!(format!("{:?}", explicit.spawner).contains("CommandSpawner"));
    }

    #[test]
    fn builders_replace_their_field() {
        let spawner = RecordingSpawner::spawner();
        let clock = FakeClock::at(7);
        let terminal = TerminalSpec::with_args("kitty", vec!["-e".to_string()]);
        let options = RegistryOptions::with_search_dirs(Vec::new())
            .with_spawner(spawner.clone())
            .with_clock(clock.clone())
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
        assert_eq!(registry.search_dirs(), crate::search_dirs().as_slice());
        assert_eq!(registry.options().terminal, TerminalSpec::from_env());
        assert!(AppRegistry::default().is_empty());

        let dirs = vec![PathBuf::from("/tmp/adesk-only")];
        let explicit = AppRegistry::with_options(RegistryOptions::with_search_dirs(dirs.clone()));
        assert_eq!(explicit.search_dirs(), dirs.as_slice());
        assert_eq!(explicit.options().search_dirs, dirs);
    }

    // --- scan --------------------------------------------------------------

    #[test]
    fn scan_walks_recursively_and_ignores_hidden_and_non_desktop_files() {
        let dir = TempDir::new().expect("temp dir");
        write(dir.path(), "alpha.desktop", &desktop("Alpha"));
        write(dir.path(), "sub/beta.desktop", &desktop("Beta"));
        write(dir.path(), "sub/deeper/gamma.desktop", &desktop("Gamma"));
        write(dir.path(), "notes.txt", "not a desktop file");
        write(dir.path(), ".hidden.desktop", &desktop("Hidden"));
        write(dir.path(), ".hidden/delta.desktop", &desktop("Delta"));

        let (fixture, report) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!(report.dirs_scanned, 1);
        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.apps, 3);
        assert_eq!(report.skipped, 0);
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
        assert_eq!(fixture.registry.len(), 3);
        assert_eq!(
            listed(&fixture, None, true),
            ["alpha", "sub.beta", "sub.deeper.gamma"]
        );
    }

    #[test]
    fn scan_skips_directory_symlinks_but_reads_file_symlinks() {
        let dir = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        write(dir.path(), "real/entry.desktop", &desktop("Real"));
        symlink(dir.path().join("real"), dir.path().join("link")).expect("dir symlink");
        let target = write(outside.path(), "linked.desktop", &desktop("Linked"));
        symlink(target, dir.path().join("linked.desktop")).expect("file symlink");

        let (fixture, report) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!((report.apps, report.files_scanned), (2, 2));
        // `link.entry` would appear here if the directory symlink were followed.
        assert_eq!(listed(&fixture, None, true), ["linked", "real.entry"]);
    }

    #[test]
    fn scan_counts_non_application_entries_as_skipped() {
        let dir = TempDir::new().expect("temp dir");
        write(dir.path(), "app.desktop", &desktop("App"));
        write(
            dir.path(),
            "link.desktop",
            "[Desktop Entry]\nType=Link\nName=Link\n",
        );

        let (fixture, report) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!(
            (report.files_scanned, report.skipped, report.apps),
            (2, 1, 1)
        );
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
        assert_eq!(listed(&fixture, None, true), ["app"]);
    }

    #[test]
    fn scan_records_issues_without_aborting() {
        let dir = TempDir::new().expect("temp dir");
        let garbage = write(dir.path(), "garbage.desktop", "no group header\n");
        let nameless = write(
            dir.path(),
            "nameless.desktop",
            "[Desktop Entry]\nType=Application\n",
        );
        let typeless = write(
            dir.path(),
            "typeless.desktop",
            "[Desktop Entry]\nName=Typeless\n",
        );
        write(dir.path(), "fine.desktop", &desktop("Fine"));
        write(dir.path(), "good.desktop", &desktop("Good"));
        let dangling = dir.path().join("dangling.desktop");
        symlink(dir.path().join("missing-target"), &dangling).expect("broken symlink");

        let (fixture, report) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!(
            (report.apps, report.files_scanned, report.skipped),
            (2, 5, 0)
        );
        let mut paths: Vec<PathBuf> = report.issues.iter().map(|i| i.path.clone()).collect();
        paths.sort();
        let mut expected = vec![garbage, nameless, typeless, dangling];
        expected.sort();
        assert_eq!(paths, expected);

        let reasons: Vec<&str> = report.issues.iter().map(|i| i.reason.as_str()).collect();
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
        assert!(fixture.registry.contains(&id("good")));
    }

    #[test]
    fn scan_dedupes_by_precedence_and_a_hidden_entry_shadows() {
        let high = TempDir::new().expect("high dir");
        let low = TempDir::new().expect("low dir");
        write(
            high.path(),
            "dup.desktop",
            &format!("{HEADER}Name=Shadowing\nHidden=true\n"),
        );
        write(high.path(), "only-high.desktop", &desktop("OnlyHigh"));
        write(low.path(), "dup.desktop", &desktop("Shadowed"));
        write(low.path(), "only-low.desktop", &desktop("OnlyLow"));

        let dirs = [high.path().to_path_buf(), low.path().to_path_buf()];
        let (fixture, report) = Fixture::scanned(&dirs);

        assert_eq!(
            (report.dirs_scanned, report.files_scanned, report.apps),
            (2, 4, 3)
        );
        let dup = fixture.registry.get(&id("dup")).expect("dup is kept");
        assert_eq!(dup.name, "Shadowing");
        assert!(dup.hidden);
        assert_eq!(listed(&fixture, None, false), ["only-high", "only-low"]);
        assert_eq!(
            listed(&fixture, None, true),
            ["dup", "only-high", "only-low"]
        );
    }

    #[test]
    fn scan_replaces_the_table_atomically() {
        let dir = TempDir::new().expect("temp dir");
        write(dir.path(), "first.desktop", &desktop("First"));
        let (fixture, _) = Fixture::scanned(&[dir.path().to_path_buf()]);
        assert_eq!(fixture.registry.len(), 1);

        std::fs::remove_file(dir.path().join("first.desktop")).expect("remove fixture");
        write(dir.path(), "second.desktop", &desktop("Second"));
        assert_eq!(fixture.registry.scan().expect("second scan").apps, 1);

        assert!(fixture.registry.contains(&id("second")));
        assert!(!fixture.registry.contains(&id("first")));
        assert_eq!(listed(&fixture, None, true), ["second"]);
    }

    #[test]
    fn scan_ignores_missing_search_directories() {
        let (fixture, report) = Fixture::scanned(&[PathBuf::from("/nonexistent/adesk-apps")]);

        assert_eq!(report, ScanReport::default());
        assert!(fixture.registry.is_empty());
    }

    #[test]
    fn scan_uses_the_configured_locale_for_names() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "app.desktop",
            &format!("{HEADER}Name=Base\nName[fr]=Francais\n"),
        );

        let fixture = Fixture::with_locale(&[dir.path().to_path_buf()], Some("fr_FR.UTF-8"));
        fixture.registry.scan().expect("scan");

        let app = fixture.registry.get(&id("app")).expect("app");
        assert_eq!(app.name, "Francais");
        assert_eq!(listed(&fixture, Some("franc"), false), ["app"]);
    }

    // --- list / get / contains ---------------------------------------------

    #[test]
    fn list_filters_hidden_and_no_display_unless_requested() {
        let dir = TempDir::new().expect("temp dir");
        write(dir.path(), "visible.desktop", &desktop("Visible"));
        write(
            dir.path(),
            "hidden.desktop",
            &format!("{HEADER}Name=Hidden\nHidden=true\n"),
        );
        write(
            dir.path(),
            "nodisplay.desktop",
            &format!("{HEADER}Name=NoDisplay\nNoDisplay=true\n"),
        );
        let (fixture, _) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!(listed(&fixture, None, false), ["visible"]);
        assert_eq!(
            listed(&fixture, None, true),
            ["hidden", "nodisplay", "visible"]
        );
    }

    #[test]
    fn list_matches_query_against_id_and_name_case_insensitively() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "org.example.editor.desktop",
            &desktop("Example Editor"),
        );
        write(dir.path(), "terminal.desktop", &desktop("Shell"));
        let (fixture, _) = Fixture::scanned(&[dir.path().to_path_buf()]);

        assert_eq!(
            listed(&fixture, Some("EDITOR"), false),
            ["org.example.editor"]
        );
        assert_eq!(
            listed(&fixture, Some("editor"), false),
            ["org.example.editor"]
        );
        assert_eq!(listed(&fixture, Some("shell"), false), ["terminal"]);
        assert_eq!(
            listed(&fixture, Some(""), false),
            ["org.example.editor", "terminal"]
        );
        assert!(listed(&fixture, Some("nothing"), false).is_empty());
    }

    #[test]
    fn get_and_contains_include_hidden_entries() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "hidden.desktop",
            &format!("{HEADER}Name=Hidden\nHidden=true\nTryExec=/usr/bin/true\n"),
        );
        let (fixture, _) = Fixture::scanned(&[dir.path().to_path_buf()]);

        let info = fixture.registry.get(&id("hidden")).expect("hidden entry");
        assert_eq!(info.name, "Hidden");
        assert!(info.hidden);
        assert_eq!(info.try_exec.as_deref(), Some("/usr/bin/true"));
        assert!(fixture.registry.contains(&id("hidden")));
        assert!(!fixture.registry.contains(&id("missing")));
        assert_eq!(fixture.registry.get(&id("missing")), None);
    }

    // --- launch ------------------------------------------------------------

    #[test]
    fn launch_expands_field_codes_and_appends_args() {
        let dir = TempDir::new().expect("temp dir");
        let fixture = Fixture::entry(
            &dir,
            "app.desktop",
            "Name=Example\nExec=/usr/bin/example --flag %U %c %k %i\nIcon=example-icon\n",
        );
        let args = vec!["--extra".to_string(), "value".to_string()];

        let record = fixture
            .registry
            .launch(&id("app"), &args, &LaunchEnv::new())
            .expect("launch");

        let command = fixture.spawner.last();
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
        assert_eq!((record.app_id, record.pid), (id("app"), Some(4242)));

        // The same command carries the LaunchEnv overrides when one is given.
        let env = LaunchEnv::new()
            .with_wayland_display("wayland-9")
            .with_var("GDK_BACKEND", "wayland");
        fixture
            .registry
            .launch(&id("app"), &[], &env)
            .expect("launch with env");
        assert_eq!(fixture.spawner.last().env, env.overrides());
    }

    #[test]
    fn launch_wraps_terminal_entries() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "term.desktop",
            &format!("{HEADER}Name=Term\nExec=/usr/bin/htop\nTerminal=true\n"),
        );
        let spawner = RecordingSpawner::spawner();
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
            .launch(&id("term"), &[], &LaunchEnv::new())
            .expect("launch");

        assert_eq!(
            argv(&spawner.last()),
            ["kitty", "--single-instance", "-e", "/usr/bin/htop"]
        );
    }

    #[test]
    fn launch_error_paths_never_spawn() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "tryexec.desktop",
            &format!("{HEADER}Name=Try\nExec=/usr/bin/app\nTryExec=/nonexistent/adesk-missing\n"),
        );
        write(
            dir.path(),
            "noexec.desktop",
            &format!("{HEADER}Name=NoExec\n"),
        );
        write(
            dir.path(),
            "dbus.desktop",
            &format!("{HEADER}Name=DBus\nDBusActivatable=true\n"),
        );
        write(
            dir.path(),
            "empty.desktop",
            &format!("{HEADER}Name=Empty\nExec=%U\n"),
        );
        write(
            dir.path(),
            "bad.desktop",
            &format!("{HEADER}Name=Bad\nExec=/usr/bin/app \"oops\n"),
        );
        let (fixture, _) = Fixture::scanned(&[dir.path().to_path_buf()]);

        let unknown = fixture.launch("ghost").expect_err("unknown app");
        assert!(matches!(unknown, Error::UnknownApp(ref app) if app == &id("ghost")));

        let try_exec = fixture.launch("tryexec").expect_err("try exec missing");
        assert!(matches!(
            try_exec,
            Error::TryExecNotFound { ref program } if program == "/nonexistent/adesk-missing"
        ));

        // No Exec, DBus-only, and an Exec that expands to nothing.
        for app in ["noexec", "dbus", "empty"] {
            let error = fixture.launch(app).expect_err(app);
            assert!(matches!(error, Error::NoExec { .. }), "{app}: {error:?}");
        }

        let invalid = fixture.launch("bad").expect_err("invalid exec");
        assert!(matches!(
            invalid,
            Error::InvalidExec {
                source: ExecError::UnterminatedQuote { .. },
                ..
            }
        ));

        assert_eq!(fixture.spawner.count(), 0);
    }

    #[test]
    fn launch_allocates_monotonic_ids_and_reads_the_clock() {
        let dir = TempDir::new().expect("temp dir");
        let fixture = Fixture::entry(&dir, "app.desktop", "Name=App\nExec=/usr/bin/app\n");

        fixture.clock.set(1_234);
        let first = fixture.launch("app").expect("first launch");
        fixture.spawner.set_fail(true);
        let failed = fixture.launch("app").expect_err("spawn fails");
        assert!(matches!(failed, Error::Spawn(_)));
        fixture.spawner.set_fail(false);
        fixture.clock.set(987_654);
        let third = fixture.launch("app").expect("third launch");

        assert_eq!(
            (first.launch_id, first.started_at_ms, first.pid),
            (LaunchId::from(1), 1_234, Some(4242))
        );
        // The failed spawn consumed id 2, so the next launch is 3.
        assert_eq!(
            (third.launch_id, third.started_at_ms),
            (LaunchId::from(3), 987_654)
        );
        assert_eq!(fixture.spawner.count(), 3);

        // A spawner that reports no pid is passed through as `None`.
        let registry = AppRegistry::with_options(
            RegistryOptions::with_search_dirs(vec![dir.path().to_path_buf()])
                .with_spawner(RecordingSpawner::with_pid(None)),
        );
        registry.scan().expect("scan");
        let record = registry
            .launch(&id("app"), &[], &LaunchEnv::new())
            .expect("launch");
        assert_eq!(record.pid, None);
    }
}
