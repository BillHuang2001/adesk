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
    /// 6. [`LaunchEnv::removals`] and [`LaunchEnv::overrides`] are applied to the
    ///    command's environment;
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
                env_remove: Vec::new(),
            }
        };

        // 6. Environment removals and overrides are applied to the inherited
        //    environment (overrides win for a key that is both set and removed).
        command.env = env.overrides();
        command.env_remove = env.removals().to_vec();

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
    //! Pure option-constructor tests only. The registry behavior matrix
    //! (scan/list/get/launch, including both injection seams) lives in
    //! `tests/registry.rs`, which uses the shared doubles from `tests/support`.

    use super::*;

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
    fn registry_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppRegistry>();
        assert_send_sync::<RegistryOptions>();
    }
}
