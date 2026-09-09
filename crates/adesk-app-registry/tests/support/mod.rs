//! Shared helpers for the `adesk-app-registry` integration suite.
//!
//! Everything here goes through the crate's **public API only**, so the suite
//! proves the documented surface is usable from outside the crate. The helpers
//! cover the three injection seams and the fixture needs of the suite:
//!
//! - [`RecordingSpawner`] — a [`ProcessSpawner`] mock that records every
//!   [`SpawnCommand`] and never starts a process;
//! - [`FakeClock`] — a manually advanced [`Clock`] so correlation deadlines are
//!   deterministic;
//! - `.desktop` fixture helpers over [`tempfile::tempdir`] (write an entry at a
//!   relative path, optionally nested, and type it through the real parser);
//! - small builders for [`AppInfo`], [`LaunchRecord`] and [`WindowCandidate`].
//!
//! Every integration-test binary compiles this module and uses a subset of it,
//! hence the module-level `dead_code` allowance.

#![allow(dead_code)] // one binary per test file; each uses a subset of these helpers

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use adesk_app_registry::{
    parse_str, Clock, Correlation, CorrelationOutcome, DesktopEntry, EntryError, LaunchRecord,
    ProcessSpawner, RawEntry, SpawnCommand, SpawnError, SpawnedProcess, WindowCandidate,
};
use adesk_core::{AppId, AppInfo, LaunchId, WindowId};

// --- process spawner mock --------------------------------------------------

/// Pid a [`RecordingSpawner`] reports by default.
pub const SPAWN_PID: i32 = 4242;

/// What a [`RecordingSpawner`] returns from [`ProcessSpawner::spawn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOutcome {
    /// The spawn succeeds and reports this pid (`None`: the OS reported none).
    Succeed(Option<i32>),
    /// The spawn fails with [`SpawnError::Other`].
    Fail(String),
}

#[derive(Debug)]
struct SpawnerState {
    commands: Vec<SpawnCommand>,
    outcome: SpawnOutcome,
}

/// [`ProcessSpawner`] mock: records every command, spawns nothing.
///
/// The default outcome succeeds with [`SPAWN_PID`]; switch to a failure with
/// [`RecordingSpawner::failing`] or [`RecordingSpawner::set_outcome`].
#[derive(Debug)]
pub struct RecordingSpawner {
    state: Mutex<SpawnerState>,
}

impl Default for RecordingSpawner {
    fn default() -> RecordingSpawner {
        RecordingSpawner::new()
    }
}

impl RecordingSpawner {
    /// Succeeds with [`SPAWN_PID`].
    pub fn new() -> RecordingSpawner {
        RecordingSpawner::with_pid(Some(SPAWN_PID))
    }

    /// Succeeds with an explicit pid (`None`: the OS reported none).
    pub fn with_pid(pid: Option<i32>) -> RecordingSpawner {
        RecordingSpawner {
            state: Mutex::new(SpawnerState {
                commands: Vec::new(),
                outcome: SpawnOutcome::Succeed(pid),
            }),
        }
    }

    /// Fails every spawn with [`SpawnError::Other`].
    pub fn failing(message: impl Into<String>) -> RecordingSpawner {
        RecordingSpawner {
            state: Mutex::new(SpawnerState {
                commands: Vec::new(),
                outcome: SpawnOutcome::Fail(message.into()),
            }),
        }
    }

    /// Wraps the mock in an `Arc` for `RegistryOptions::with_spawner`.
    pub fn shared(self) -> Arc<RecordingSpawner> {
        Arc::new(self)
    }

    /// Every recorded command, in spawn order.
    pub fn commands(&self) -> Vec<SpawnCommand> {
        self.lock().commands.clone()
    }

    /// The most recent recorded command, if any.
    pub fn last(&self) -> Option<SpawnCommand> {
        self.lock().commands.last().cloned()
    }

    /// Number of recorded spawn attempts (successful or not).
    pub fn len(&self) -> usize {
        self.lock().commands.len()
    }

    /// Whether nothing was spawned yet.
    pub fn is_empty(&self) -> bool {
        self.lock().commands.is_empty()
    }

    /// Forgets every recorded command (keeps the outcome).
    pub fn clear(&self) {
        self.lock().commands.clear();
    }

    /// Replaces the outcome returned by subsequent spawns.
    pub fn set_outcome(&self, outcome: SpawnOutcome) {
        self.lock().outcome = outcome;
    }

    /// Makes every subsequent spawn fail with `message`.
    pub fn fail_with(&self, message: impl Into<String>) {
        self.set_outcome(SpawnOutcome::Fail(message.into()));
    }

    fn lock(&self) -> MutexGuard<'_, SpawnerState> {
        // A panicking test must not turn into a poisoning failure in another test.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProcessSpawner for RecordingSpawner {
    fn spawn(&self, command: &SpawnCommand) -> std::result::Result<SpawnedProcess, SpawnError> {
        let mut state = self.lock();
        state.commands.push(command.clone());
        match state.outcome.clone() {
            SpawnOutcome::Succeed(pid) => Ok(SpawnedProcess::new(pid)),
            SpawnOutcome::Fail(message) => Err(SpawnError::Other(message)),
        }
    }
}

// --- clock mock ------------------------------------------------------------

/// [`Clock`] whose value only changes when the test changes it.
#[derive(Debug, Default)]
pub struct FakeClock {
    now_ms: AtomicU64,
}

impl FakeClock {
    /// A clock reading `now_ms`.
    pub fn new(now_ms: u64) -> FakeClock {
        FakeClock {
            now_ms: AtomicU64::new(now_ms),
        }
    }

    /// Adds `ms` and returns the new value.
    pub fn advance(&self, ms: u64) -> u64 {
        self.now_ms.fetch_add(ms, Ordering::SeqCst) + ms
    }

    /// Jumps to an absolute value (may go backwards on purpose).
    pub fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    /// Current value; identical to [`Clock::now_ms`].
    pub fn now(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// A shared [`FakeClock`] starting at `now_ms`.
pub fn fake_clock(now_ms: u64) -> Arc<FakeClock> {
    Arc::new(FakeClock::new(now_ms))
}

/// Erases the concrete clock to the `Arc<dyn Clock>` the API expects.
pub fn as_clock(clock: &Arc<FakeClock>) -> Arc<dyn Clock> {
    clock.clone()
}

// --- `.desktop` fixtures ---------------------------------------------------

/// Writes `contents` to `root/relative`, creating parent directories.
pub fn write_file(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
    }
    std::fs::write(&path, contents).expect("write fixture file");
    path
}

/// Wraps `body` in the mandatory `[Desktop Entry]` group header.
pub fn entry_text(body: &str) -> String {
    format!("[Desktop Entry]\n{body}")
}

/// Writes a `[Desktop Entry]` fixture at `root/relative`.
pub fn write_entry(root: &Path, relative: &str, body: &str) -> PathBuf {
    write_file(root, relative, &entry_text(body))
}

/// Parses `text` through [`parse_str`], panicking on malformed fixtures.
pub fn parse(text: &str) -> RawEntry {
    parse_str(text).expect("fixture parses")
}

/// Parses a fixture body (with the group header added) through [`parse_str`].
pub fn parse_body(body: &str) -> RawEntry {
    parse(&entry_text(body))
}

/// Writes a `[Desktop Entry]` fixture, reads it back and types it.
///
/// Uses the real pipeline: file on disk → [`parse_str`] → [`DesktopEntry::from_raw`]
/// with the id derived from the path. Panics when the fixture is not a valid
/// `Type=Application` entry — use [`try_entry_at`] for the rejection cases.
pub fn entry_at(root: &Path, relative: &str, body: &str, locale: Option<&str>) -> DesktopEntry {
    try_entry_at(root, relative, body, locale).expect("fixture validates")
}

/// Like [`entry_at`], but returns the typing error instead of panicking.
pub fn try_entry_at(
    root: &Path,
    relative: &str,
    body: &str,
    locale: Option<&str>,
) -> std::result::Result<DesktopEntry, EntryError> {
    let path = write_entry(root, relative, body);
    let id = adesk_app_registry::desktop_file_id(root, &path).expect("fixture path yields an id");
    let text = std::fs::read_to_string(&path).expect("read fixture");
    DesktopEntry::from_raw(id, path, &parse(&text), locale)
}

// --- domain builders -------------------------------------------------------

/// `AppId` from a string.
pub fn app(id: &str) -> AppId {
    AppId::from(id)
}

/// An [`AppInfo`] with only the fields correlation reads filled in.
pub fn app_info(id: &str, name: &str, startup_wm_class: Option<&str>) -> AppInfo {
    AppInfo {
        id: app(id),
        name: name.to_owned(),
        icon: None,
        exec: Some("/bin/true".to_owned()),
        terminal: false,
        categories: Vec::new(),
        startup_wm_class: startup_wm_class.map(str::to_owned),
        dbus_activatable: false,
        hidden: false,
        no_display: false,
        try_exec: None,
    }
}

/// A [`LaunchRecord`] with explicit id, pid and start time.
pub fn launch_record(
    launch_id: u64,
    app_id: &str,
    pid: Option<i32>,
    started_at_ms: u64,
) -> LaunchRecord {
    LaunchRecord {
        launch_id: LaunchId(launch_id),
        app_id: app(app_id),
        pid,
        started_at_ms,
    }
}

/// A mapped toplevel candidate.
pub fn window<'a>(
    window_id: u64,
    pid: Option<i32>,
    app_id: Option<&'a str>,
    title: Option<&'a str>,
) -> WindowCandidate<'a> {
    WindowCandidate {
        window_id: WindowId(window_id),
        pid,
        app_id,
        title,
    }
}

/// Unwraps a correlated outcome, panicking (with a useful message) otherwise.
pub fn correlation(outcome: &CorrelationOutcome) -> &Correlation {
    match outcome {
        CorrelationOutcome::Correlated(correlation) => correlation,
        _ => panic!("expected CorrelationOutcome::Correlated, got {outcome:?}"),
    }
}
