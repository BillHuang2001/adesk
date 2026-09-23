//! Process launch: launch records, environment overrides, and the spawner seam.
//!
//! [`ProcessSpawner`] is the injection point that keeps tests hermetic: unit and
//! integration tests install a mock and assert the exact argv/env without spawning
//! anything. Production uses [`CommandSpawner`] (`std::process::Command`).

use adesk_core::{AppId, LaunchId};

/// Environment variable carrying the Wayland socket name the compositor listens on.
const WAYLAND_DISPLAY_VAR: &str = "WAYLAND_DISPLAY";

/// Environment variable carrying the per-user runtime directory holding the socket.
const XDG_RUNTIME_DIR_VAR: &str = "XDG_RUNTIME_DIR";
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
///
/// Besides overriding values, the set can also [`without`](LaunchEnv::without)
/// variables: a removal deletes the variable from the child's inherited
/// environment, which is the only way to neutralize a parent value the launcher
/// does not want to leak (e.g. a host `DISPLAY` that would make a toolkit prefer
/// X11). An explicit override for the same key wins over its removal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchEnv {
    /// Value for `WAYLAND_DISPLAY`; `None` inherits.
    pub wayland_display: Option<String>,
    /// Value for `XDG_RUNTIME_DIR`; `None` inherits.
    pub xdg_runtime_dir: Option<String>,
    /// Additional overrides, applied after the Wayland variables (later wins).
    pub extra: Vec<(String, String)>,
    /// Keys to delete from the child's inherited environment, in insertion order.
    pub removals: Vec<String>,
}

impl LaunchEnv {
    /// Empty override set (inherit everything).
    pub fn new() -> LaunchEnv {
        Self::default()
    }

    /// Sets the `WAYLAND_DISPLAY` override.
    pub fn with_wayland_display(mut self, value: impl Into<String>) -> LaunchEnv {
        self.wayland_display = Some(value.into());
        self
    }

    /// Sets the `XDG_RUNTIME_DIR` override.
    pub fn with_xdg_runtime_dir(mut self, value: impl Into<String>) -> LaunchEnv {
        self.xdg_runtime_dir = Some(value.into());
        self
    }

    /// Adds an extra environment override.
    pub fn with_var(mut self, key: impl Into<String>, value: impl Into<String>) -> LaunchEnv {
        self.extra.push((key.into(), value.into()));
        self
    }

    /// Marks `key` for removal from the child's inherited environment.
    ///
    /// Removal is applied before the overrides, so an explicit `with_var` for the
    /// same key still wins.
    pub fn without(mut self, key: impl Into<String>) -> LaunchEnv {
        self.removals.push(key.into());
        self
    }

    /// Flattens the overrides in application order: `WAYLAND_DISPLAY`,
    /// `XDG_RUNTIME_DIR`, then `extra` in insertion order.
    pub fn overrides(&self) -> Vec<(String, String)> {
        let mut overrides = Vec::with_capacity(self.extra.len() + 2);
        if let Some(value) = &self.wayland_display {
            overrides.push((WAYLAND_DISPLAY_VAR.to_string(), value.clone()));
        }
        if let Some(value) = &self.xdg_runtime_dir {
            overrides.push((XDG_RUNTIME_DIR_VAR.to_string(), value.clone()));
        }
        overrides.extend(self.extra.iter().cloned());
        overrides
    }

    /// The keys marked for removal, in insertion order.
    pub fn removals(&self) -> &[String] {
        &self.removals
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
    /// Keys deleted from the inherited environment (see [`CommandSpawner`]).
    pub env_remove: Vec<String>,
}

impl SpawnCommand {
    /// A command with no arguments and no environment changes.
    pub fn new(program: impl Into<String>) -> SpawnCommand {
        SpawnCommand {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    /// Appends one argument.
    pub fn with_arg(mut self, arg: impl Into<String>) -> SpawnCommand {
        self.args.push(arg.into());
        self
    }

    /// Appends one environment override.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> SpawnCommand {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Marks one inherited environment variable for removal.
    pub fn with_env_remove(mut self, key: impl Into<String>) -> SpawnCommand {
        self.env_remove.push(key.into());
        self
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
/// Inherits the runtime's environment, deletes [`SpawnCommand::env_remove`], then
/// applies [`SpawnCommand::env`] on top. It does **not** wait for or reap the
/// child: the returned process is expected to outlive the launch call.
/// `adesk-server` owns child reaping (see `CONTEXT.md`).
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandSpawner;

impl CommandSpawner {
    /// Creates the production spawner.
    pub fn new() -> CommandSpawner {
        Self
    }
}

impl ProcessSpawner for CommandSpawner {
    fn spawn(&self, command: &SpawnCommand) -> std::result::Result<SpawnedProcess, SpawnError> {
        let child = build_command(command)
            .spawn()
            .map_err(|source| SpawnError::Io {
                program: command.program.clone(),
                source,
            })?;
        // `Child` is dropped without waiting: the launched application is expected
        // to outlive this call, and `adesk-server` owns reaping (see `CONTEXT.md`).
        Ok(SpawnedProcess::new(Some(child.id() as i32)))
    }
}

/// Builds the `std::process::Command` for `command` without spawning it.
///
/// The child inherits the runtime's environment (no `env_clear`), with
/// [`SpawnCommand::env_remove`] deleted first and [`SpawnCommand::env`] then
/// layered on top in order — a repeated key therefore keeps the last value, and an
/// override of a removed key wins. The program is executed directly, never through
/// a shell, and the arguments are passed verbatim (no word splitting or globbing).
pub(crate) fn build_command(command: &SpawnCommand) -> std::process::Command {
    let mut process = std::process::Command::new(&command.program);
    process.args(&command.args);
    for key in &command.env_remove {
        process.env_remove(key);
    }
    for (key, value) in &command.env {
        process.env(key, value);
    }
    process
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args_of(command: &std::process::Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of(command: &std::process::Command) -> Vec<(String, Option<String>)> {
        command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    /// `Command::get_envs` yields the explicitly set variables in an unspecified
    /// order, so assertions compare them as a map.
    fn env_map(
        command: &std::process::Command,
    ) -> std::collections::BTreeMap<String, Option<String>> {
        env_of(command).into_iter().collect()
    }
    #[test]
    fn launch_env_new_is_empty() {
        let env = LaunchEnv::new();
        assert_eq!(env, LaunchEnv::default());
        assert_eq!(env.wayland_display, None);
        assert_eq!(env.xdg_runtime_dir, None);
        assert!(env.extra.is_empty());
        assert!(env.overrides().is_empty());
    }

    #[test]
    fn launch_env_builders_chain_in_application_order() {
        let env = LaunchEnv::new()
            .with_wayland_display("wayland-1")
            .with_xdg_runtime_dir("/run/user/1000")
            .with_var("GDK_BACKEND", "wayland")
            .with_var("MOZ_ENABLE_WAYLAND", "1");

        assert_eq!(
            env.overrides(),
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
                ("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string()),
                ("GDK_BACKEND".to_string(), "wayland".to_string()),
                ("MOZ_ENABLE_WAYLAND".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn launch_env_omits_none_fields() {
        let only_extra = LaunchEnv::new().with_var("A", "1").with_var("B", "2");
        assert_eq!(
            only_extra.overrides(),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string())
            ]
        );

        let only_runtime = LaunchEnv::new().with_xdg_runtime_dir("/tmp/rt");
        assert_eq!(
            only_runtime.overrides(),
            vec![("XDG_RUNTIME_DIR".to_string(), "/tmp/rt".to_string())]
        );

        let only_wayland = LaunchEnv::new().with_wayland_display("wayland-0");
        assert_eq!(
            only_wayland.overrides(),
            vec![("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string())]
        );
    }

    #[test]
    fn launch_env_empty_value_is_an_override_not_an_omission() {
        let env = LaunchEnv::new().with_wayland_display("");
        assert_eq!(env.wayland_display.as_deref(), Some(""));
        assert_eq!(
            env.overrides(),
            vec![("WAYLAND_DISPLAY".to_string(), String::new())]
        );
    }

    #[test]
    fn launch_env_extra_keeps_insertion_order_and_duplicates() {
        let env = LaunchEnv::new()
            .with_var("K", "first")
            .with_var("J", "middle")
            .with_var("K", "last");
        assert_eq!(
            env.overrides(),
            vec![
                ("K".to_string(), "first".to_string()),
                ("J".to_string(), "middle".to_string()),
                ("K".to_string(), "last".to_string()),
            ]
        );
    }

    #[test]
    fn launch_env_without_records_removals_in_order() {
        let env = LaunchEnv::new()
            .without("DISPLAY")
            .without("XAUTHORITY")
            .without("DISPLAY");

        assert_eq!(
            env.removals(),
            [
                "DISPLAY".to_string(),
                "XAUTHORITY".to_string(),
                "DISPLAY".to_string()
            ]
        );
        // Removals do not leak into the additive overrides.
        assert!(env.overrides().is_empty());
    }

    #[test]
    fn launch_env_new_has_no_removals() {
        assert!(LaunchEnv::new().removals().is_empty());
    }

    #[test]
    fn launch_env_removals_and_overrides_are_independent() {
        let env = LaunchEnv::new()
            .with_wayland_display("wayland-3")
            .with_var("GDK_BACKEND", "wayland")
            .without("DISPLAY");

        assert_eq!(
            env.overrides(),
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-3".to_string()),
                ("GDK_BACKEND".to_string(), "wayland".to_string()),
            ]
        );
        assert_eq!(env.removals(), ["DISPLAY".to_string()]);
    }

    #[test]
    fn spawn_command_new_has_no_args_or_env() {
        let command = SpawnCommand::new("/usr/bin/firefox");
        assert_eq!(command.program, "/usr/bin/firefox");
        assert!(command.args.is_empty());
        assert!(command.env.is_empty());
        assert!(command.env_remove.is_empty());
    }

    #[test]
    fn spawn_command_builders_append_in_order() {
        let command = SpawnCommand::new("kitty")
            .with_arg("-e")
            .with_arg("htop")
            .with_env("A", "1")
            .with_env("B", "2")
            .with_env_remove("DISPLAY")
            .with_env_remove("XAUTHORITY");

        assert_eq!(command.program, "kitty");
        assert_eq!(command.args, vec!["-e".to_string(), "htop".to_string()]);
        assert_eq!(
            command.env,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string())
            ]
        );
        assert_eq!(
            command.env_remove,
            vec!["DISPLAY".to_string(), "XAUTHORITY".to_string()]
        );
    }
    #[test]
    fn build_command_sets_program_and_args_verbatim() {
        // Metacharacters must survive untouched: no shell, no splitting, no globbing.
        let command = SpawnCommand::new("/usr/bin/printf")
            .with_arg("a b; c")
            .with_arg("$HOME")
            .with_arg("*")
            .with_arg("");
        let built = build_command(&command);

        assert_eq!(built.get_program(), OsStr::new("/usr/bin/printf"));
        assert_eq!(args_of(&built), vec!["a b; c", "$HOME", "*", ""]);
        assert!(env_of(&built).is_empty());
    }

    #[test]
    fn build_command_layers_env_overrides_with_last_value_winning() {
        let command = SpawnCommand::new("app")
            .with_env("WAYLAND_DISPLAY", "wayland-1")
            .with_env("XDG_RUNTIME_DIR", "/run/user/1000")
            .with_env("WAYLAND_DISPLAY", "wayland-2");
        let built = build_command(&command);

        assert_eq!(args_of(&built), Vec::<String>::new());
        // `get_envs` exposes only the explicit overrides: the rest is inherited.
        // A repeated key collapses to the last value ("later wins").
        assert_eq!(built.get_envs().count(), 2);
        assert_eq!(
            env_map(&built),
            std::collections::BTreeMap::from([
                ("WAYLAND_DISPLAY".to_string(), Some("wayland-2".to_string())),
                (
                    "XDG_RUNTIME_DIR".to_string(),
                    Some("/run/user/1000".to_string())
                ),
            ])
        );
    }

    #[test]
    fn build_command_applies_launch_env_overrides() {
        let env = LaunchEnv::new()
            .with_wayland_display("wayland-9")
            .with_xdg_runtime_dir("/run/user/1001")
            .with_var("GDK_BACKEND", "wayland");
        let command = SpawnCommand {
            program: "app".to_string(),
            args: vec![],
            env: env.overrides(),
            env_remove: env.removals().to_vec(),
        };

        assert_eq!(
            env_map(&build_command(&command)),
            std::collections::BTreeMap::from([
                ("WAYLAND_DISPLAY".to_string(), Some("wayland-9".to_string())),
                (
                    "XDG_RUNTIME_DIR".to_string(),
                    Some("/run/user/1001".to_string())
                ),
                ("GDK_BACKEND".to_string(), Some("wayland".to_string())),
            ])
        );
    }
    #[test]
    fn build_command_removes_variables_that_the_parent_has_set() {
        // `PATH` is set in this process's environment by the test runner, so a
        // `None` entry proves a removal, not a no-op on an unset key.
        assert!(std::env::var_os("PATH").is_some());
        let command = SpawnCommand::new("app")
            .with_env_remove("PATH")
            .with_env_remove("DISPLAY");

        // `get_envs` reports a removal as a key mapped to `None`.
        assert_eq!(
            env_map(&build_command(&command)),
            std::collections::BTreeMap::from([
                ("PATH".to_string(), None),
                ("DISPLAY".to_string(), None),
            ])
        );
    }

    #[test]
    fn build_command_override_wins_over_a_removal_of_the_same_key() {
        let command = SpawnCommand::new("app")
            .with_env_remove("DISPLAY")
            .with_env("DISPLAY", ":99");

        assert_eq!(
            env_map(&build_command(&command)),
            std::collections::BTreeMap::from([("DISPLAY".to_string(), Some(":99".to_string()))])
        );
    }

    /// A variable the test runner sets and a shell does not synthesize, so its
    /// removal can be observed from a spawned child.
    fn inherited_probe_var() -> &'static str {
        ["HOME", "USER", "LANG", "TERM", "LOGNAME"]
            .into_iter()
            .find(|key| std::env::var_os(key).is_some())
            .expect("the test runner provides at least one of HOME/USER/LANG/TERM/LOGNAME")
    }

    /// `sh -c 'printf %s "${KEY+SET}"'` — a shell builtin, so the probe needs no
    /// `PATH` of its own. `/bin/sh` is the POSIX shell of every platform this
    /// Linux-only crate targets.
    fn probe_command(key: &str, remove: bool) -> std::process::Command {
        let script = format!("printf %s \"${{{key}+SET}}\"");
        let command = SpawnCommand::new("/bin/sh").with_arg("-c").with_arg(script);
        let command = if remove {
            command.with_env_remove(key)
        } else {
            command
        };
        build_command(&command)
    }

    #[test]
    fn spawned_child_loses_removed_variables_and_keeps_inherited_ones() {
        let key = inherited_probe_var();

        // Control: an untouched variable is inherited by the child.
        let inherited = probe_command(key, false).output().expect("spawn /bin/sh");
        assert_eq!(String::from_utf8_lossy(&inherited.stdout), "SET");

        // The removed variable is really gone from the spawned child.
        let removed = probe_command(key, true).output().expect("spawn /bin/sh");
        assert_eq!(
            String::from_utf8_lossy(&removed.stdout),
            "",
            "{key} must not reach the child after with_env_remove({key:?})"
        );
    }
    #[test]
    fn command_spawner_reports_missing_program_as_io_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("adesk-no-such-program");
        let missing = missing.to_str().expect("utf-8 temp path").to_string();

        let error = CommandSpawner::new()
            .spawn(&SpawnCommand::new(missing.as_str()))
            .expect_err("spawning a nonexistent program must fail");
        let rendered = error.to_string();

        match error {
            SpawnError::Io { program, source } => {
                assert_eq!(program, missing);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
                assert!(
                    rendered.contains(&missing),
                    "message lost the program: {rendered}"
                );
            }
            other => panic!("expected SpawnError::Io, got {other:?}"),
        }
    }
    #[test]
    fn command_spawner_is_debug_and_copy() {
        let spawner = CommandSpawner::new();
        let copied = spawner;
        assert!(format!("{spawner:?}").contains("CommandSpawner"));
        assert!(format!("{copied:?}").contains("CommandSpawner"));
    }

    #[test]
    fn spawned_process_new_wraps_pid() {
        assert_eq!(SpawnedProcess::new(Some(4242)).pid, Some(4242));
        assert_eq!(SpawnedProcess::new(None).pid, None);
    }
}
