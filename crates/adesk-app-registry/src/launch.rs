//! Process launch: launch records, environment overrides, and the spawner seam.
//!
//! [`ProcessSpawner`] is the injection point that keeps tests hermetic: unit and
//! integration tests install a mock and assert the exact argv/env without spawning
//! anything. Production uses [`CommandSpawner`] (`std::process::Command`).

use adesk_core::{AppId, LaunchId};

/// One successful `launch_app` call.
///
/// The server returns `launch_id`/`app_id`/`pid` to the client (AGP §5.2), emits
/// the `AppLaunched` event, and registers the record with the
/// [`crate::Correlator`] so a later `WindowCreated` can be attributed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRecord {
    /// Monotonic launch id, starting at 1; consumed even when the spawn fails.
    pub launch_id: LaunchId,
    /// The launched application.
    pub app_id: AppId,
    /// Child pid when the OS reported one.
    pub pid: Option<i32>,
    /// Monotonic milliseconds since runtime start, from the registry's
    /// [`crate::Clock`].
    pub started_at_ms: u64,
}

/// Environment overrides applied to a spawned application.
///
/// `None` fields mean "inherit the runtime's environment" (the server passes what
/// the compositor actually uses). Applications must see the runtime's Wayland
/// socket, so `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` are the first overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchEnv {
    /// Value for `WAYLAND_DISPLAY`; `None` inherits.
    pub wayland_display: Option<String>,
    /// Value for `XDG_RUNTIME_DIR`; `None` inherits.
    pub xdg_runtime_dir: Option<String>,
    /// Additional overrides, applied after the Wayland variables (later wins).
    pub extra: Vec<(String, String)>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl LaunchEnv {
    /// Empty override set (inherit everything).
    pub fn new() -> LaunchEnv {
        Self::default()
    }

    /// Reads `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` from the process environment.
    pub fn from_process() -> LaunchEnv {
        todo!("stub: implementation phase")
    }

    /// Sets the `WAYLAND_DISPLAY` override.
    pub fn with_wayland_display(self, value: impl Into<String>) -> LaunchEnv {
        todo!("stub: implementation phase")
    }

    /// Sets the `XDG_RUNTIME_DIR` override.
    pub fn with_xdg_runtime_dir(self, value: impl Into<String>) -> LaunchEnv {
        todo!("stub: implementation phase")
    }

    /// Adds an extra environment override.
    pub fn with_var(self, key: impl Into<String>, value: impl Into<String>) -> LaunchEnv {
        todo!("stub: implementation phase")
    }

    /// Flattens the overrides in application order: `WAYLAND_DISPLAY`,
    /// `XDG_RUNTIME_DIR`, then `extra` in insertion order.
    pub fn overrides(&self) -> Vec<(String, String)> {
        todo!("stub: implementation phase")
    }
}

/// A fully resolved process to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    /// Program to execute (from `Exec` expansion, or the terminal program).
    pub program: String,
    /// Arguments in order.
    pub args: Vec<String>,
    /// Environment overrides layered on top of the inherited environment.
    pub env: Vec<(String, String)>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl SpawnCommand {
    /// A command with no arguments and no environment overrides.
    pub fn new(program: impl Into<String>) -> SpawnCommand {
        todo!("stub: implementation phase")
    }

    /// Appends one argument.
    pub fn with_arg(self, arg: impl Into<String>) -> SpawnCommand {
        todo!("stub: implementation phase")
    }

    /// Appends one environment override.
    pub fn with_env(self, key: impl Into<String>, value: impl Into<String>) -> SpawnCommand {
        todo!("stub: implementation phase")
    }
}

/// Result of a successful spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedProcess {
    /// Child pid when the OS reported one.
    pub pid: Option<i32>,
}

impl SpawnedProcess {
    /// A spawn result with the given pid.
    pub fn new(pid: Option<i32>) -> SpawnedProcess {
        SpawnedProcess { pid }
    }
}

/// Why a spawn failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnError {
    /// The OS rejected the spawn (missing binary, permissions, fd limits, ...).
    #[error("failed to spawn {program}: {source}")]
    Io {
        /// Program that could not be executed.
        program: String,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// Injection seam for mock spawners that fail without an OS error.
    #[error("{0}")]
    Other(String),
}

/// Spawns processes. The seam that keeps tests hermetic.
///
/// Implementations must be `Send + Sync` (the registry is shared across server
/// tasks) and must not block the compositor thread. Reaping children is the
/// caller's responsibility — see [`CommandSpawner`].
pub trait ProcessSpawner: Send + Sync + std::fmt::Debug {
    /// Spawns `command` and returns the child pid when available.
    fn spawn(&self, command: &SpawnCommand) -> std::result::Result<SpawnedProcess, SpawnError>;
}

/// Production spawner backed by [`std::process::Command`].
///
/// Inherits the runtime's environment and applies [`SpawnCommand::env`] on top.
/// It does **not** wait for or reap the child: the returned process is expected to
/// outlive the launch call. `adesk-server` owns child reaping (see `CONTEXT.md`).
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandSpawner;

impl CommandSpawner {
    /// Creates the production spawner.
    pub fn new() -> CommandSpawner {
        Self
    }
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl ProcessSpawner for CommandSpawner {
    fn spawn(&self, command: &SpawnCommand) -> std::result::Result<SpawnedProcess, SpawnError> {
        todo!("stub: implementation phase")
    }
}
