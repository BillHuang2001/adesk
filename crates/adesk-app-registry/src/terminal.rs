//! Terminal wrapping for `Terminal=true` entries (`docs/architecture.md` §7).

use crate::launch::SpawnCommand;

/// Program used when `$TERMINAL` is unset or empty.
pub const DEFAULT_TERMINAL: &str = "x-terminal-emulator";

/// Environment variable consulted before [`DEFAULT_TERMINAL`].
pub const TERMINAL_ENV: &str = "TERMINAL";

/// Argument separating the terminal program from the command it runs.
pub const EXEC_SEPARATOR: &str = "-e";

/// How to run a command inside a terminal emulator.
///
/// `prefix_args` are the full arguments placed before the wrapped command,
/// including the separator (e.g. `["-e"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpec {
    // stub: populated by the implementation.
    #[allow(dead_code)]
    program: String,
    #[allow(dead_code)]
    prefix_args: Vec<String>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl TerminalSpec {
    /// `program` with the conventional [`EXEC_SEPARATOR`] prefix.
    pub fn new(program: impl Into<String>) -> TerminalSpec {
        todo!("stub: implementation phase")
    }

    /// Explicit program and prefix arguments (no implicit separator appended).
    pub fn with_args(program: impl Into<String>, prefix_args: Vec<String>) -> TerminalSpec {
        todo!("stub: implementation phase")
    }

    /// Resolves `$TERMINAL` from the process environment, else [`DEFAULT_TERMINAL`].
    pub fn from_env() -> TerminalSpec {
        todo!("stub: implementation phase")
    }

    /// Pure core of [`TerminalSpec::from_env`] for tests and explicit configuration.
    ///
    /// The value is split on ASCII whitespace: the first token is the program,
    /// remaining tokens are prefix arguments, and [`EXEC_SEPARATOR`] is appended
    /// last. `None` or an empty value yields `new(DEFAULT_TERMINAL)`.
    pub fn from_env_value(value: Option<&str>) -> TerminalSpec {
        todo!("stub: implementation phase")
    }

    /// The terminal program.
    pub fn program(&self) -> &str {
        todo!("stub: implementation phase")
    }

    /// Arguments placed before the wrapped command (including the separator).
    pub fn prefix_args(&self) -> &[String] {
        todo!("stub: implementation phase")
    }

    /// Builds the spawn command that runs `command` inside the terminal.
    ///
    /// Returns `None` when `command` is empty. The returned command's `env` is
    /// empty; [`crate::AppRegistry::launch`] applies [`crate::LaunchEnv`] to it.
    pub fn wrap(&self, command: &[String]) -> Option<SpawnCommand> {
        todo!("stub: implementation phase")
    }
}

impl Default for TerminalSpec {
    /// The deterministic fallback: [`DEFAULT_TERMINAL`] with `-e`.
    fn default() -> Self {
        Self::new(DEFAULT_TERMINAL)
    }
}
