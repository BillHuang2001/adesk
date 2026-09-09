//! Isolated filesystem and process environment for one test runtime.
//!
//! Every [`TestRuntime`](crate::TestRuntime) owns a [`TestEnv`]: a `tempfile` directory
//! containing an `XDG_RUNTIME_DIR` (mode `0700`), an empty fixture data dir, an empty
//! `XDG_DATA_HOME`, a unique Wayland socket name and the AGP socket path. Nothing in a
//! test ever touches the host's real `$XDG_RUNTIME_DIR`, `$XDG_DATA_DIRS` or `$HOME`.
//!
//! ## Process environment hazard
//!
//! The process environment is global. Child processes launched by the runtime's app
//! registry receive `LaunchEnv::from_process()`, i.e. whatever `XDG_RUNTIME_DIR` /
//! `WAYLAND_DISPLAY` this *test process* has — so launch tests need
//! [`TestEnv::apply`], which sets those variables and restores them when the returned
//! [`EnvScope`] drops. Two runtimes applying different envs in parallel will race:
//! launch tests must not run concurrently with other runtimes in the same test binary
//! (put them in one test function or run that binary with `--test-threads=1`).
//! The Wayland test client does *not* depend on the process env: it connects to an
//! absolute socket path ([`TestEnv::wayland_socket_path`]).

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tempfile::TempDir;

use crate::error::Result;

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

    /// Absolute path of the compositor's Wayland socket.
    pub fn wayland_socket_path(&self) -> PathBuf {
        self.runtime_dir.join(&self.display)
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
    /// variable if it was unset). See the module docs for the parallel-test hazard.
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
