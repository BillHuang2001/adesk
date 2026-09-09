//! `adesk-test-app` — helper process that opens a real toplevel and exits on command.
//!
//! Phase-1 skeleton: the CLI grammar is frozen here and must stay in sync with
//! `adesk_testkit::TestAppSpec::cli_args`, which builds exactly this command line.
//!
//! # CLI grammar
//!
//! ```text
//! adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] [--fill <PATTERN>] [--exit-after <MS>]
//! ```
//!
//! - `--app-id <ID>` (required): the `xdg_toplevel` app id and, unless `--title` overrides
//!   it, the window title.
//! - `--title <TITLE>`: window title; default = the app id.
//! - `--size <W>x<H>`: committed buffer size in pixels; default `640x480`.
//! - `--fill <PATTERN>`: buffer fill, decoded with [`FillPattern::from_cli_arg`]
//!   (`solid:RRGGBB[AA]`, `checker:SIZE:RRGGBB[AA]:RRGGBB[AA]`, `gradient-h:...`,
//!   `gradient-v:...`); default [`FillPattern::default`].
//! - `--exit-after <MS>`: exit on its own after `MS` milliseconds; default: wait for the
//!   `exit` command on stdin.
//!
//! Flags may appear in any order; a repeated flag's last occurrence wins (Phase 2).
//!
//! # Phase-2 behavior contract
//!
//! 1. connect to `$WAYLAND_DISPLAY` inside `$XDG_RUNTIME_DIR` (failure → exit 2);
//! 2. create an `xdg_toplevel` with the app id and title and commit it;
//! 3. wait (bounded 10 s) for the first `xdg_surface.configure`, then `ack_configure` and
//!    commit one SHM frame filled with `--fill` at `--size`;
//! 4. pump events until `exit\n` arrives on stdin or `--exit-after` elapses;
//! 5. destroy the surface, flush, disconnect and exit 0.
//!
//! A connect or protocol failure prints the error to stderr and exits 2. A usage error
//! (unknown flag, missing value, malformed `--size`/`--fill`/`--exit-after`, missing
//! `--app-id`) prints the message plus [`USAGE`] to stderr and exits 64.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | clean exit (stdin `exit` command or `--exit-after`) |
//! | 2 | Wayland connect/protocol failure |
//! | 64 | usage error |
//!
//! The helper never panics: every failure is a diagnostic plus an exit code, so a test that
//! asserts on the exit status gets a readable message instead of a signal.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use adesk_testkit::{FillPattern, Size};

/// Usage line printed after a usage error (exit 64).
#[allow(dead_code)] // stub: printed by the Phase-2 usage-error path.
const USAGE: &str = "usage: adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] \
                     [--fill <PATTERN>] [--exit-after <MS>]";

/// Parsed command line; see the module docs for the grammar and defaults.
#[allow(dead_code)] // stub: fields are read by the Phase-2 implementation.
#[derive(Debug, Clone)]
struct CliArgs {
    /// `--app-id`, required.
    app_id: String,
    /// `--title`, defaults to the app id.
    title: String,
    /// `--size`, defaults to `640x480`.
    size: Size,
    /// `--fill`, decoded from the [`FillPattern::from_cli_arg`] grammar.
    fill: FillPattern,
    /// `--exit-after`, `None` to wait for the stdin `exit` command.
    exit_after: Option<Duration>,
}

#[allow(dead_code, unused_variables)] // stub: parse is called by main in Phase 2.
impl CliArgs {
    /// Parses `args` (without `argv[0]`).
    ///
    /// Phase-2 semantics: every flag takes exactly one value (a missing value, a value that
    /// looks like another flag, or a non-flag argument is an error); the last occurrence of a
    /// repeated flag wins; an unknown flag is an error; `--app-id` is required. `--size`
    /// requires `<W>x<H>` with two `u32`s (zero dimensions are allowed), `--exit-after`
    /// requires a `u64` of milliseconds and `--fill` must decode via
    /// [`FillPattern::from_cli_arg`]. The returned `Err` message is exactly what is printed
    /// before [`USAGE`].
    fn parse(args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
        todo!("stub: implementation phase — semantics documented above")
    }
}

fn main() -> ExitCode {
    todo!("stub: implementation phase — see the module docs for the exit-code contract")
}
