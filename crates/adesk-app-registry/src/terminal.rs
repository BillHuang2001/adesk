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
    program: String,
    prefix_args: Vec<String>,
}

impl TerminalSpec {
    /// `program` with the conventional [`EXEC_SEPARATOR`] prefix.
    pub fn new(program: impl Into<String>) -> TerminalSpec {
        TerminalSpec {
            program: program.into(),
            prefix_args: vec![EXEC_SEPARATOR.to_string()],
        }
    }

    /// Explicit program and prefix arguments (no implicit separator appended).
    pub fn with_args(program: impl Into<String>, prefix_args: Vec<String>) -> TerminalSpec {
        TerminalSpec {
            program: program.into(),
            prefix_args,
        }
    }

    /// Resolves `$TERMINAL` from the process environment, else [`DEFAULT_TERMINAL`].
    pub fn from_env() -> TerminalSpec {
        let value = std::env::var(TERMINAL_ENV).ok();
        Self::from_env_value(value.as_deref())
    }

    /// Pure core of [`TerminalSpec::from_env`] for tests and explicit configuration.
    ///
    /// The value is split on ASCII whitespace: the first token is the program,
    /// remaining tokens are prefix arguments, and [`EXEC_SEPARATOR`] is appended
    /// last. `None` or an empty value yields `new(DEFAULT_TERMINAL)`.
    pub fn from_env_value(value: Option<&str>) -> TerminalSpec {
        let Some(value) = value else {
            return Self::new(DEFAULT_TERMINAL);
        };
        let mut tokens = value.split_ascii_whitespace();
        let Some(program) = tokens.next() else {
            return Self::new(DEFAULT_TERMINAL);
        };
        let mut prefix_args: Vec<String> = tokens.map(str::to_owned).collect();
        prefix_args.push(EXEC_SEPARATOR.to_string());
        Self::with_args(program, prefix_args)
    }

    /// The terminal program.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Arguments placed before the wrapped command (including the separator).
    pub fn prefix_args(&self) -> &[String] {
        &self.prefix_args
    }

    /// Builds the spawn command that runs `command` inside the terminal.
    ///
    /// Returns `None` when `command` is empty. The returned command's `env` is
    /// empty; [`crate::AppRegistry::launch`] applies [`crate::LaunchEnv`] to it.
    pub fn wrap(&self, command: &[String]) -> Option<SpawnCommand> {
        if command.is_empty() {
            return None;
        }
        let mut args = Vec::with_capacity(self.prefix_args.len() + command.len());
        args.extend(self.prefix_args.iter().cloned());
        args.extend(command.iter().cloned());
        Some(SpawnCommand {
            program: self.program.clone(),
            args,
            env: Vec::new(),
        })
    }
}
impl Default for TerminalSpec {
    /// The deterministic fallback: [`DEFAULT_TERMINAL`] with `-e`.
    fn default() -> Self {
        Self::new(DEFAULT_TERMINAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn new_appends_the_exec_separator() {
        let spec = TerminalSpec::new("kitty");
        assert_eq!(spec.program(), "kitty");
        assert_eq!(spec.prefix_args(), owned(&["-e"]).as_slice());
    }

    #[test]
    fn new_accepts_owned_strings_and_str_slices() {
        assert_eq!(TerminalSpec::new(String::from("foot")).program(), "foot");
        assert_eq!(TerminalSpec::new("foot").program(), "foot");
        assert_eq!(TerminalSpec::new("").program(), "");
    }

    #[test]
    fn with_args_uses_prefix_args_verbatim() {
        let spec = TerminalSpec::with_args("kitty", owned(&["--single-instance", "-e"]));
        assert_eq!(spec.program(), "kitty");
        assert_eq!(
            spec.prefix_args(),
            owned(&["--single-instance", "-e"]).as_slice()
        );

        // No implicit separator is appended.
        let bare = TerminalSpec::with_args("foot", Vec::new());
        assert_eq!(bare.program(), "foot");
        assert!(bare.prefix_args().is_empty());
    }

    #[test]
    fn constants_are_pinned() {
        assert_eq!(DEFAULT_TERMINAL, "x-terminal-emulator");
        assert_eq!(TERMINAL_ENV, "TERMINAL");
        assert_eq!(EXEC_SEPARATOR, "-e");
    }

    #[test]
    fn from_env_value_table() {
        let cases: Vec<(Option<&str>, &str, Vec<String>)> = vec![
            (None, DEFAULT_TERMINAL, owned(&["-e"])),
            (Some(""), DEFAULT_TERMINAL, owned(&["-e"])),
            (Some("   "), DEFAULT_TERMINAL, owned(&["-e"])),
            (Some("\t\n"), DEFAULT_TERMINAL, owned(&["-e"])),
            (Some("kitty"), "kitty", owned(&["-e"])),
            (
                Some("kitty --single-instance"),
                "kitty",
                owned(&["--single-instance", "-e"]),
            ),
            (
                Some("  kitty   --single-instance  "),
                "kitty",
                owned(&["--single-instance", "-e"]),
            ),
            (
                Some("wezterm start --"),
                "wezterm",
                owned(&["start", "--", "-e"]),
            ),
        ];

        for (value, program, prefix_args) in cases {
            let spec = TerminalSpec::from_env_value(value);
            assert_eq!(spec.program(), program, "program for {value:?}");
            assert_eq!(
                spec.prefix_args(),
                prefix_args.as_slice(),
                "prefix for {value:?}"
            );
        }
    }
    #[test]
    fn from_env_value_splits_ascii_whitespace_only() {
        // U+00A0 is whitespace to `str::split_whitespace` but not ASCII, so it stays
        // part of the program name (the documented "ASCII whitespace" rule).
        let spec = TerminalSpec::from_env_value(Some("kitty\u{a0}--single-instance"));
        assert_eq!(spec.program(), "kitty\u{a0}--single-instance");
        assert_eq!(spec.prefix_args(), owned(&["-e"]).as_slice());
    }

    #[test]
    fn from_env_matches_the_environment() {
        // Snapshot-compare: never mutate the process environment in tests.
        let value = std::env::var(TERMINAL_ENV).ok();
        let from_env = TerminalSpec::from_env();
        let expected = TerminalSpec::from_env_value(value.as_deref());
        assert_eq!(from_env, expected);
    }

    #[test]
    fn default_is_the_deterministic_fallback() {
        assert_eq!(TerminalSpec::default(), TerminalSpec::new(DEFAULT_TERMINAL));
        assert_eq!(TerminalSpec::default().program(), DEFAULT_TERMINAL);
        assert_eq!(
            TerminalSpec::default().prefix_args(),
            owned(&["-e"]).as_slice()
        );
    }

    #[test]
    fn wrap_returns_none_for_an_empty_command() {
        assert_eq!(TerminalSpec::new("kitty").wrap(&[]), None);
        assert_eq!(TerminalSpec::from_env_value(None).wrap(&[]), None);
    }

    #[test]
    fn wrap_places_prefix_args_then_the_command() {
        let spec = TerminalSpec::new("kitty");
        assert_eq!(
            spec.wrap(&owned(&["htop"])),
            Some(SpawnCommand {
                program: "kitty".to_string(),
                args: owned(&["-e", "htop"]),
                env: Vec::new(),
            })
        );
    }

    #[test]
    fn wrap_keeps_prefix_args_before_the_separator() {
        let spec = TerminalSpec::from_env_value(Some("kitty --single-instance"));
        assert_eq!(
            spec.wrap(&owned(&["vim", "/tmp/a b"])),
            Some(SpawnCommand {
                program: "kitty".to_string(),
                args: owned(&["--single-instance", "-e", "vim", "/tmp/a b"]),
                env: Vec::new(),
            })
        );
    }

    #[test]
    fn wrap_without_prefix_args_omits_the_separator() {
        let spec = TerminalSpec::with_args("foot", Vec::new());
        assert_eq!(
            spec.wrap(&owned(&["sh", "-c", "echo hi"])),
            Some(SpawnCommand {
                program: "foot".to_string(),
                args: owned(&["sh", "-c", "echo hi"]),
                env: Vec::new(),
            })
        );
    }

    #[test]
    fn wrap_leaves_env_empty_and_does_not_mutate_the_spec() {
        let spec = TerminalSpec::new("kitty");
        let wrapped = spec.wrap(&owned(&["htop"])).expect("non-empty command");
        assert!(wrapped.env.is_empty());
        assert_eq!(spec, TerminalSpec::new("kitty"));
        assert_eq!(spec.wrap(&owned(&["htop"])), Some(wrapped));
    }

    #[test]
    fn wrap_preserves_empty_arguments() {
        let spec = TerminalSpec::new("kitty");
        assert_eq!(
            spec.wrap(&owned(&["cmd", ""])),
            Some(SpawnCommand {
                program: "kitty".to_string(),
                args: owned(&["-e", "cmd", ""]),
                env: Vec::new(),
            })
        );
    }
}
