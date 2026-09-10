//! Isolated filesystem and process environment for one test runtime.
//!
//! Every [`TestRuntime`](crate::TestRuntime) owns a [`TestEnv`]: a `tempfile` directory
//! containing an `XDG_RUNTIME_DIR` (mode `0700`), an empty fixture data dir, an empty
//! `XDG_DATA_HOME`, a unique Wayland socket name and the AGP socket path. Nothing in a
//! test ever touches the host's real `$XDG_RUNTIME_DIR`, `$XDG_DATA_DIRS` or `$HOME`.
//!
//! ## Process environment: one scoped runtime at a time
//!
//! The process environment is global, and two parts of a runtime read it directly:
//! `adesk-compositor` binds its Wayland listening socket under the process
//! `XDG_RUNTIME_DIR`, and `launch_app` builds the child's `LaunchEnv` from the *server
//! process's* environment (`XDG_RUNTIME_DIR` from the process env, plus the
//! compositor's `WAYLAND_DISPLAY`), so a child inherits whatever env this process has
//! at launch time.
//!
//! [`TestRuntime::start_with`](crate::TestRuntime::start_with) therefore serializes every
//! env mutation on the process-wide `lock_process_env` guard and always scopes the
//! process env to the runtime's own [`TestEnv`] across `Server::start`, so a compositor can
//! never bind into another runtime's — or the ambient, possibly read-only — runtime dir.
//! When [`TestRuntimeConfig::apply_env`](crate::TestRuntimeConfig::apply_env) is `true` the
//! [`EnvScope`] *and* the lock stay held for the runtime's lifetime, because registry
//! children inherit the process env at launch time; that also serializes env-scoped
//! runtimes in one test binary, so plain `cargo test` needs no `--test-threads=1`.
//! With `apply_env == false` the env is restored as soon as the server is up, and the lock
//! is released with it.
//!
//! An env-scoped runtime holds the lock for its whole lifetime, so a second env-scoped
//! runtime started *inside the same test* cannot make progress; the bounded acquire fails
//! with [`TestkitError::Timeout`] instead of hanging the test
//! binary. Code that mutates the process env itself should hold the same lock (or run in
//! its own test binary).
//!
//! The Wayland test client does *not* depend on the process env: it connects to an
//! absolute socket path under the runtime directory.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};

use crate::error::{Result, TestkitError};

/// Monotonic counter making Wayland display names unique within a process.
static NEXT_ENV: AtomicU64 = AtomicU64::new(1);

/// A private runtime directory, data dirs and socket paths for one test runtime.
#[derive(Debug)]
pub struct TestEnv {
    root: TempDir,
    runtime_dir: PathBuf,
    data_dir: PathBuf,
    data_home: PathBuf,
    display: String,
    agp_socket: PathBuf,
}

impl TestEnv {
    /// Creates the directory tree and picks a unique Wayland display name.
    ///
    /// Layout: `<root>/runtime` (0700), `<root>/data` (fixture share root, passed as
    /// `XDG_DATA_DIRS`), `<root>/data_home` (empty `XDG_DATA_HOME`), AGP socket at
    /// `<root>/runtime/adesk.sock`, Wayland display `wayland-adesk-<pid>-<n>`.
    pub fn new() -> Result<TestEnv> {
        let root = tempfile::tempdir()?;
        let runtime_dir = root.path().join("runtime");
        let data_dir = root.path().join("data");
        let data_home = root.path().join("data_home");
        for dir in [&runtime_dir, &data_dir, &data_home] {
            std::fs::create_dir_all(dir)?;
        }
        // XDG_RUNTIME_DIR must be private to the user; some clients check it.
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))?;
        let display = format!(
            "wayland-adesk-{}-{}",
            std::process::id(),
            NEXT_ENV.fetch_add(1, Ordering::Relaxed)
        );
        let agp_socket = runtime_dir.join("adesk.sock");
        Ok(TestEnv {
            root,
            runtime_dir,
            data_dir,
            data_home,
            display,
            agp_socket,
        })
    }

    /// The temp directory containing everything this env owns.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// `XDG_RUNTIME_DIR` for this runtime (mode `0700`).
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Fixture share root; pass to [`TestRuntimeConfig::with_app_dirs`] or
    /// [`FixtureDir`](crate::FixtureDir).
    ///
    /// The registry appends `/applications` to each `XDG_DATA_DIRS` entry, so this is a
    /// *share* root, not the applications directory itself.
    ///
    /// [`TestRuntimeConfig::with_app_dirs`]: crate::TestRuntimeConfig::with_app_dirs
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Empty `XDG_DATA_HOME` used to isolate the registry from the host.
    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    /// The Wayland socket name to set as `WAYLAND_DISPLAY`.
    pub fn wayland_display(&self) -> &str {
        &self.display
    }

    /// Absolute path of the AGP Unix socket.
    pub fn agp_socket(&self) -> &Path {
        &self.agp_socket
    }

    /// The search dirs a runtime started with this env should scan: `[data_dir]`.
    pub fn app_dirs(&self) -> Vec<PathBuf> {
        vec![self.data_dir.clone()]
    }

    /// Sets the process environment for this env and returns a restoring guard.
    ///
    /// Sets `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, `XDG_DATA_DIRS`, `XDG_DATA_HOME` and
    /// `ADESK_SOCKET`; dropping the guard restores every previous value (or removes the
    /// variable if it was unset). The process env is global: see the module docs and hold
    /// the process-wide environment lock while a runtime starts.
    pub fn apply(&self) -> EnvScope {
        EnvScope::set(&[
            ("XDG_RUNTIME_DIR", Some(self.runtime_dir.clone().into())),
            ("WAYLAND_DISPLAY", Some(OsString::from(&self.display))),
            ("XDG_DATA_DIRS", Some(self.data_dir.clone().into())),
            ("XDG_DATA_HOME", Some(self.data_home.clone().into())),
            ("ADESK_SOCKET", Some(self.agp_socket.clone().into())),
        ])
    }
}

/// RAII guard that sets process environment variables and restores them on drop.
///
/// Created by [`TestEnv::apply`]; also usable directly for a single variable.
#[derive(Debug)]
pub struct EnvScope {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvScope {
    /// Sets each variable (removing it when the value is `None`) and remembers the
    /// previous state.
    pub fn set(vars: &[(&'static str, Option<OsString>)]) -> EnvScope {
        let mut saved = Vec::with_capacity(vars.len());
        for (name, value) in vars {
            saved.push((*name, std::env::var_os(name)));
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        EnvScope { saved }
    }

    /// The variable names this guard owns.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.saved.iter().map(|(name, _)| *name)
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        // Restore in reverse order so nested guards unwind correctly.
        for (name, previous) in self.saved.drain(..).rev() {
            match previous {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

// --- process-wide environment lock ---

/// The lock that serializes every change to the process environment.
///
/// A `tokio` mutex rather than `std`'s: the guard is held across
/// `Server::start(..).await` (and, for env-scoped runtimes, for the runtime's whole
/// lifetime), so it must never block an executor thread and must be `Send`.
static PROCESS_ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// How long [`lock_process_env`] waits for [`PROCESS_ENV_LOCK`].
///
/// Generous on purpose: an env-scoped runtime holds the lock for its whole lifetime, so
/// parallel tests in one binary queue behind each other. The bound exists so that a harness
/// mistake — a second env-scoped runtime inside one test — fails with a
/// [`TestkitError::Timeout`] instead of hanging the test
/// binary forever.
pub(crate) const PROCESS_ENV_LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// Proof that this task holds [`PROCESS_ENV_LOCK`]; the lock is released on drop.
///
/// Droppable from synchronous code (so [`crate::TestRuntime`]'s `Drop` stays non-blocking)
/// and `Send` (so the runtime handle and the startup future stay `Send`).
pub(crate) struct ProcessEnvLock {
    _guard: MutexGuard<'static, ()>,
}

impl std::fmt::Debug for ProcessEnvLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessEnvLock")
    }
}

/// Acquires the process-wide environment lock, bounded by [`PROCESS_ENV_LOCK_TIMEOUT`].
///
/// # Errors
///
/// Returns [`TestkitError::Timeout`] if another runtime in
/// this process holds the lock for longer than the bound.
pub(crate) async fn lock_process_env() -> Result<ProcessEnvLock> {
    match tokio::time::timeout(PROCESS_ENV_LOCK_TIMEOUT, PROCESS_ENV_LOCK.lock()).await {
        Ok(guard) => Ok(ProcessEnvLock { _guard: guard }),
        Err(_elapsed) => Err(TestkitError::Timeout {
            what: "process environment lock",
            timeout: PROCESS_ENV_LOCK_TIMEOUT,
        }),
    }
}

// The guard is stored in `TestRuntime` and inside the `start_with` future, so it must stay
// `Send`; this fails to compile if the guard ever loses that property.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ProcessEnvLock>();
};
