//! `adesk-e2e-app` — the fixture application of the `adesk-agent` end-to-end suite.
//!
//! The launch tests in `tests/e2e_runtime.rs` start a *real* application through the
//! runtime's XDG registry, so they need a helper that opens a real `xdg_toplevel` and
//! commits pixels the test can assert on. `adesk-testkit` ships such a helper
//! (`adesk-test-app`), but a downstream crate cannot point testkit's [`TestAppSpec`] /
//! [`TestApp`] at a binary of its own: both resolve the program through
//! `adesk_testkit::helper_bin_path`, which only looks next to the current test binary and
//! on `$PATH` — never in this package's `target/<profile>/examples`.
//!
//! Declaring the helper as an *example* of this package is what makes the suite
//! self-contained: `cargo test` builds examples of the local package together with its
//! dev-dependencies, and the suite writes the `.desktop` entry itself from the public
//! [`DesktopEntryFixture`] / [`FixtureDir::write_entry`] API, with `Exec[0]` pointing at the
//! example binary next to the test binary.
//!
//! [`TestAppSpec`]: adesk_testkit::TestAppSpec
//! [`TestApp`]: adesk_testkit::TestApp
//! [`DesktopEntryFixture`]: adesk_testkit::DesktopEntryFixture
//! [`FixtureDir::write_entry`]: adesk_testkit::FixtureDir::write_entry
//!
//! # CLI grammar
//!
//! ```text
//! adesk-e2e-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] [--fill <PATTERN>] [--exit-after <MS>]
//! ```
//!
//! - `--app-id <ID>` (required): the `xdg_toplevel` app id and, unless `--title` overrides
//!   it, the window title.
//! - `--title <TITLE>`: window title; default = the app id.
//! - `--size <W>x<H>`: committed buffer size in pixels; default `640x480`.
//! - `--fill <PATTERN>`: buffer fill, decoded with [`FillPattern::from_cli_arg`]
//!   (`solid:RRGGBB[AA]`, `checker:SIZE:RRGGBB[AA]:RRGGBB[AA]`, `gradient-h:...`,
//!   `gradient-v:...`); default [`FillPattern::default`].
//! - `--exit-after <MS>`: exit on its own after `MS` milliseconds; default 10 s.
//!
//! Flags may appear in any order; a repeated flag's last occurrence wins. The grammar is
//! byte-compatible with [`TestAppSpec::cli_args`], which is what the suite feeds this
//! binary through the `.desktop` entry's `Exec` line.
//!
//! # Behavior
//!
//! 1. connect to `$WAYLAND_DISPLAY` inside `$XDG_RUNTIME_DIR` (failure → exit 2);
//! 2. create an `xdg_toplevel` with the app id and title;
//! 3. wait (bounded 10 s) for the first `xdg_surface.configure`, then `ack_configure` and
//!    commit one SHM frame filled with `--fill` at `--size`;
//! 4. stay alive for `--exit-after` while the client's reader thread keeps dispatching
//!    events (no manual event loop: the window must simply remain mapped and responsive
//!    while the test observes, injects input into and captures it); the reader thread
//!    reports EOF when the runtime goes away, which also ends the wait early;
//! 5. best-effort destroy the surface, flush and close the connection, then exit 0.
//!
//! A connect or protocol failure prints the error to stderr and exits 2. A usage error
//! (unknown flag, missing value, malformed `--size`/`--fill`/`--exit-after`, missing
//! `--app-id`) prints the message plus [`USAGE`] to stderr and exits 64.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | clean exit (`--exit-after` elapsed, or the runtime closed the connection) |
//! | 2 | Wayland connect/protocol failure |
//! | 64 | usage error |
//!
//! The app never panics: every failure is a diagnostic plus an exit code, so a test that
//! asserts on the exit status gets a readable message instead of a signal.
//!
//! [`TestAppSpec::cli_args`]: adesk_testkit::TestAppSpec::cli_args

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::{Duration, Instant};

use adesk_testkit::{FillPattern, Size, ToplevelSpec, WaylandTestClient};

/// Usage line printed after a usage error (exit 64).
const USAGE: &str = "usage: adesk-e2e-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] \
                     [--fill <PATTERN>] [--exit-after <MS>]";

/// Exit code for a Wayland connect or protocol failure.
const EXIT_PROTOCOL: u8 = 2;

/// Exit code for a usage error.
const EXIT_USAGE: u8 = 64;

/// Committed buffer size when `--size` is absent.
const DEFAULT_SIZE: Size = Size::new(640, 480);

/// Lifetime used when `--exit-after` is absent.
///
/// Bounded so a forgotten flag cannot leak a helper process into the rest of a test run.
/// The suite always passes `--exit-after`.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(10);

/// Bound on the first `xdg_surface.configure`.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Granularity of the stay-alive wait, so the deadline is never overshot by much.
const STAY_ALIVE_SLICE: Duration = Duration::from_millis(25);

/// The flags the app understands; every one of them takes exactly one value.
const FLAGS: [&str; 5] = ["--app-id", "--title", "--size", "--fill", "--exit-after"];

/// Parsed command line; see the module docs for the grammar and defaults.
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
    /// `--exit-after`, defaults to [`DEFAULT_LIFETIME`].
    exit_after: Option<Duration>,
}

impl CliArgs {
    /// Parses `args` (without `argv[0]`).
    ///
    /// Semantics: every flag takes exactly one value (a missing value, a value that looks
    /// like another flag, or a non-flag argument is an error); the last occurrence of a
    /// repeated flag wins; an unknown flag is an error; `--app-id` is required. `--size`
    /// requires `<W>x<H>` with two `u32`s (zero dimensions are allowed), `--exit-after`
    /// requires a `u64` of milliseconds and `--fill` must decode via
    /// [`FillPattern::from_cli_arg`]. The returned `Err` message is exactly what is printed
    /// before [`USAGE`].
    fn parse(mut args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
        let mut app_id: Option<String> = None;
        let mut title: Option<String> = None;
        let mut size: Option<Size> = None;
        let mut fill: Option<FillPattern> = None;
        let mut exit_after: Option<Duration> = None;

        while let Some(flag) = args.next() {
            if !FLAGS.contains(&flag.as_str()) {
                return Err(if flag.starts_with("--") {
                    format!("unknown flag `{flag}`")
                } else {
                    format!("unexpected argument `{flag}`")
                });
            }
            let value = args.next().ok_or_else(|| {
                format!("flag `{flag}` requires a value (usage: `{flag} <value>`)")
            })?;
            if value.starts_with("--") {
                return Err(format!(
                    "flag `{flag}` requires a value, found the flag `{value}`"
                ));
            }
            match flag.as_str() {
                "--app-id" => app_id = Some(value),
                "--title" => title = Some(value),
                "--size" => size = Some(parse_size(&value)?),
                "--fill" => {
                    fill = Some(
                        FillPattern::from_cli_arg(&value)
                            .map_err(|e| format!("invalid --fill value `{value}`: {e}"))?,
                    );
                }
                "--exit-after" => exit_after = Some(parse_exit_after(&value)?),
                // Not reachable: `FLAGS` and this match cover the same set.
                _ => return Err(format!("unknown flag `{flag}`")),
            }
        }

        let app_id = app_id.ok_or_else(|| "missing required flag `--app-id`".to_string())?;
        let title = title.unwrap_or_else(|| app_id.clone());
        Ok(CliArgs {
            app_id,
            title,
            size: size.unwrap_or(DEFAULT_SIZE),
            fill: fill.unwrap_or_default(),
            exit_after,
        })
    }
}

/// Parses `<W>x<H>`; zero dimensions are allowed, anything else is a usage error.
fn parse_size(value: &str) -> Result<Size, String> {
    let (w, h) = value
        .split_once('x')
        .ok_or_else(|| format!("invalid --size value `{value}`: expected <W>x<H>"))?;
    let w = w
        .parse::<u32>()
        .map_err(|_| format!("invalid --size value `{value}`: `{w}` is not a u32 width"))?;
    let h = h
        .parse::<u32>()
        .map_err(|_| format!("invalid --size value `{value}`: `{h}` is not a u32 height"))?;
    Ok(Size::new(w, h))
}

/// Parses a `u64` of milliseconds.
fn parse_exit_after(value: &str) -> Result<Duration, String> {
    let ms = value.parse::<u64>().map_err(|_| {
        format!("invalid --exit-after value `{value}`: expected milliseconds as a u64")
    })?;
    Ok(Duration::from_millis(ms))
}

fn main() -> ExitCode {
    let args = match CliArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

/// Connects, opens the toplevel, commits one frame, stays alive, then tears down.
fn run(args: &CliArgs) -> Result<(), String> {
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| {
        "WAYLAND_DISPLAY is not set; launch this app under a test runtime".to_string()
    })?;
    if display.is_empty() {
        return Err("WAYLAND_DISPLAY is empty; launch this app under a test runtime".to_string());
    }

    let mut client = WaylandTestClient::connect(&display)
        .map_err(|e| format!("cannot connect to wayland display `{display}`: {e}"))?;

    let spec =
        ToplevelSpec::new(args.app_id.clone(), args.title.clone(), args.size).with_fill(args.fill);
    let window = client
        .create_toplevel(spec)
        .map_err(|e| format!("cannot create the toplevel for `{}`: {e}", args.app_id))?;

    let configured = window
        .wait_for_configure(CONFIGURE_TIMEOUT)
        .map_err(|e| format!("no xdg configure within {CONFIGURE_TIMEOUT:?}: {e}"))?;
    window
        .apply_configure()
        .map_err(|e| format!("cannot ack configure {configured:?}: {e}"))?;
    window.commit_frame(args.fill).map_err(|e| {
        format!(
            "cannot commit the {}x{} frame: {e}",
            args.size.w, args.size.h
        )
    })?;
    client
        .flush()
        .map_err(|e| format!("cannot flush the initial commit: {e}"))?;

    // The client's reader thread dispatches events on its own; the app only has to keep the
    // window mapped and the connection open for as long as the test needs it.
    stay_alive(&client, args.exit_after.unwrap_or(DEFAULT_LIFETIME));

    // Teardown is best-effort: the compositor may already be gone, which is not a failure of
    // this helper. Diagnostics still reach stderr (the test inherits it) so a real protocol
    // bug stays visible instead of being swallowed.
    if let Err(e) = window.destroy() {
        eprintln!("adesk-e2e-app: warning: cannot destroy the toplevel: {e}");
    }
    if let Err(e) = client.flush() {
        eprintln!("adesk-e2e-app: warning: cannot flush the teardown: {e}");
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| format!("cannot build the tokio runtime: {e}"))?;
    if let Err(e) = runtime.block_on(client.close()) {
        eprintln!("adesk-e2e-app: warning: cannot close the wayland connection: {e}");
    }
    Ok(())
}

/// Sleeps for `lifetime` in [`STAY_ALIVE_SLICE`] steps, so the deadline is never overshot.
///
/// Ends early once the reader thread reports the connection closed (the runtime went away),
/// so a shut-down test run cannot leave this app running for the rest of its lifetime.
fn stay_alive(client: &WaylandTestClient, lifetime: Duration) {
    let deadline = Instant::now() + lifetime;
    loop {
        if client.is_closed() {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        std::thread::sleep(remaining.min(STAY_ALIVE_SLICE));
    }
}
