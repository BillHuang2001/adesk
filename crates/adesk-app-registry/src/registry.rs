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
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use adesk_core::{AppId, AppInfo};

use crate::clock::Clock;
use crate::error::{Error, Result};
use crate::launch::{LaunchEnv, LaunchRecord, ProcessSpawner};
use crate::parser::DesktopEntry;
use crate::terminal::TerminalSpec;

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

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl RegistryOptions {
    /// Production defaults: XDG search directories ([`crate::search_dirs`]),
    /// `$TERMINAL`-aware terminal spec, [`crate::CommandSpawner`], a fresh
    /// [`crate::MonotonicClock`] and the locale from `LC_ALL`/`LC_MESSAGES`/`LANG`.
    pub fn xdg() -> RegistryOptions {
        todo!("stub: implementation phase")
    }

    /// Explicit search directories with the other production defaults.
    pub fn with_search_dirs<I>(dirs: I) -> RegistryOptions
    where
        I: IntoIterator<Item = PathBuf>,
    {
        todo!("stub: implementation phase")
    }

    /// Replaces the process spawner.
    pub fn with_spawner(self, spawner: Arc<dyn ProcessSpawner>) -> RegistryOptions {
        todo!("stub: implementation phase")
    }

    /// Replaces the clock.
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> RegistryOptions {
        todo!("stub: implementation phase")
    }

    /// Replaces the terminal wrapper.
    pub fn with_terminal(self, terminal: TerminalSpec) -> RegistryOptions {
        todo!("stub: implementation phase")
    }

    /// Replaces the locale used for localized names.
    pub fn with_locale(self, locale: Option<String>) -> RegistryOptions {
        todo!("stub: implementation phase")
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
    // stub: populated by the implementation.
    #[allow(dead_code)]
    options: RegistryOptions,
    #[allow(dead_code)]
    entries: RwLock<BTreeMap<AppId, DesktopEntry>>,
    #[allow(dead_code)]
    next_launch_id: AtomicU64,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl AppRegistry {
    /// Empty registry with production defaults ([`RegistryOptions::xdg`]).
    ///
    /// Call [`AppRegistry::scan`] before serving requests.
    pub fn new() -> AppRegistry {
        todo!("stub: implementation phase")
    }

    /// Empty registry with explicit options.
    pub fn with_options(options: RegistryOptions) -> AppRegistry {
        todo!("stub: implementation phase")
    }

    /// The options this registry was built with.
    pub fn options(&self) -> &RegistryOptions {
        todo!("stub: implementation phase")
    }

    /// The search directories in precedence order.
    pub fn search_dirs(&self) -> &[PathBuf] {
        todo!("stub: implementation phase")
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
        todo!("stub: implementation phase")
    }

    /// Lists applications, sorted by id (BTreeMap order).
    ///
    /// `query` matches case-insensitively as a substring of the desktop-file id or
    /// the localized name. `include_hidden` controls whether entries with
    /// `Hidden=true` or `NoDisplay=true` are included (`docs/architecture.md` §7).
    pub fn list(&self, query: Option<&str>, include_hidden: bool) -> Vec<AppInfo> {
        todo!("stub: implementation phase")
    }

    /// Looks up one application, including hidden/`NoDisplay` entries — filtering
    /// is a listing concern, so an explicitly requested id is always returned.
    pub fn get(&self, id: &AppId) -> Option<AppInfo> {
        todo!("stub: implementation phase")
    }

    /// `true` when `id` is known to the registry (hidden entries included).
    pub fn contains(&self, id: &AppId) -> bool {
        todo!("stub: implementation phase")
    }

    /// Launches an application and returns its [`LaunchRecord`].
    ///
    /// Pipeline (AGP §5.2, `docs/architecture.md` §7):
    /// 1. unknown id → [`Error::UnknownApp`];
    /// 2. `TryExec` set and unresolvable → [`Error::TryExecNotFound`];
    /// 3. `Exec` expanded with [`crate::ExecExpander`] (empty `files` context);
    ///    a missing/empty result → [`Error::NoExec`] — this is also the
    ///    `DBusActivatable` fallback rule: v1 attempts `Exec` and only reports
    ///    `not_supported` when there is no usable `Exec` line;
    /// 4. `args` are appended after expansion;
    /// 5. `Terminal=true` wraps the command with [`TerminalSpec`];
    /// 6. [`LaunchEnv::overrides`] is applied to the command's environment;
    /// 7. the launch id is allocated (monotonic from 1, consumed even on failure)
    ///    and `started_at_ms` is read from the clock;
    /// 8. [`ProcessSpawner::spawn`] runs the command; OS failures →
    ///    [`Error::Spawn`] (AGP `launch_failed`).
    ///
    /// This method does **not** emit events or register with the
    /// [`crate::Correlator`]; the server does both with the returned record.
    pub fn launch(&self, id: &AppId, args: &[String], env: &LaunchEnv) -> Result<LaunchRecord> {
        todo!("stub: implementation phase")
    }

    /// Number of known entries.
    pub fn len(&self) -> usize {
        todo!("stub: implementation phase")
    }

    /// `true` when no entries are known (e.g. before [`AppRegistry::scan`]).
    pub fn is_empty(&self) -> bool {
        todo!("stub: implementation phase")
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}
