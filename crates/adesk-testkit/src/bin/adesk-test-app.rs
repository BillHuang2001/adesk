//! `adesk-test-app` — helper process that opens a real toplevel and exits on command.
//!
//! The CLI grammar is frozen here and must stay in sync with
//! `adesk_testkit::TestAppSpec::cli_args`, which builds exactly this command line.
//!
//! # CLI grammar
//!
//! ```text
//! adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] [--fill <PATTERN>] [--exit-after <MS>] [--exit-on-close]
//! ```
//!
//! - `--app-id <ID>` (required): the `xdg_toplevel` app id and, unless `--title` overrides
//!   it, the window title.
//! - `--title <TITLE>`: window title; default = the app id.
//! - `--size <W>x<H>`: committed buffer size in pixels; default `640x480`.
//! - `--fill <PATTERN>`: buffer fill, decoded with [`FillPattern::from_cli_arg`]
//!   (`solid:RRGGBB[AA]`, `checker:SIZE:RRGGBB[AA]:RRGGBB[AA]`, `gradient-h:...`,
//!   `gradient-v:...`); default [`FillPattern::default`].
//! - `--exit-after <MS>`: exit on its own after `MS` milliseconds; default: keep pumping
//!   until the `exit` command on stdin, `--exit-on-close`, or a lost connection.
//! - `--exit-on-close`: exit when the compositor requests close (`xdg_toplevel.close`);
//!   default: ignore the request and keep pumping. This flag takes no value.
//!
//! Flags may appear in any order; a repeated flag's last occurrence wins. Every flag except
//! `--exit-on-close` takes exactly one value.
//!
//! # Behavior
//!
//! 1. connect to `$WAYLAND_DISPLAY` inside `$XDG_RUNTIME_DIR` (failure → exit 2);
//! 2. create an `xdg_toplevel` with the app id and title and commit it;
//! 3. wait (bounded 10 s) for the first `xdg_surface.configure`, then `ack_configure` and
//!    commit one SHM frame filled with `--fill` at `--size`;
//! 4. pump events until `exit\n` arrives on stdin, `--exit-after` elapses, or `--exit-on-close`
//!    is set and the compositor requests close;
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
//! | 0 | clean exit (stdin `exit` command, `--exit-after`, or `--exit-on-close`) |
//! | 2 | Wayland connect/protocol failure |
//! | 64 | usage error |
//!
//! The helper never panics: every failure is a diagnostic plus an exit code, so a test that
//! asserts on the exit status gets a readable message instead of a signal.

#![forbid(unsafe_code)]

use std::io::BufRead;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use adesk_testkit::{FillPattern, Size, TestWindow, TestkitError, ToplevelSpec, WaylandTestClient};

/// Usage line printed after a usage error (exit 64).
const USAGE: &str = "usage: adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] \
                     [--fill <PATTERN>] [--exit-after <MS>] [--exit-on-close]";

/// Exit code for a Wayland connect or protocol failure.
const EXIT_PROTOCOL: u8 = 2;

/// Exit code for a usage error.
const EXIT_USAGE: u8 = 64;

/// Committed buffer size when `--size` is absent.
const DEFAULT_SIZE: Size = Size::new(640, 480);

/// Bound on the first `xdg_surface.configure`.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time a single event pump runs before the exit conditions are re-checked.
///
/// Small enough that `--exit-after` and the stdin `exit` command are honoured promptly,
/// large enough that the helper does not spin on the event queue.
const PUMP_SLICE: Duration = Duration::from_millis(50);

/// The stdin command that makes the helper exit gracefully.
const EXIT_COMMAND: &str = "exit";

/// The value-taking flags the helper understands; every one of them consumes exactly one value.
const FLAGS: [&str; 5] = ["--app-id", "--title", "--size", "--fill", "--exit-after"];

/// The valueless boolean flags the helper understands; their mere presence sets a `bool`.
const BOOL_FLAGS: [&str; 1] = ["--exit-on-close"];

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
    /// `--exit-after`, `None` to wait for the stdin `exit` command.
    exit_after: Option<Duration>,
    /// `--exit-on-close`: exit when the compositor requests close.
    exit_on_close: bool,
}

impl CliArgs {
    /// Parses `args` (without `argv[0]`).
    ///
    /// Semantics: every flag except `--exit-on-close` takes exactly one value (a missing
    /// value, a value that looks like another flag, or a non-flag argument is an error);
    /// `--exit-on-close` is a valueless boolean whose presence sets the flag; the last
    /// occurrence of a repeated flag wins; an unknown flag is an error; `--app-id` is
    /// required. `--size` requires `<W>x<H>` with two `u32`s (zero dimensions are allowed),
    /// `--exit-after` requires a `u64` of milliseconds and `--fill` must decode via
    /// [`FillPattern::from_cli_arg`]. The returned `Err` message is exactly what is printed
    /// before [`USAGE`].
    fn parse(mut args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
        let mut app_id: Option<String> = None;
        let mut title: Option<String> = None;
        let mut size: Option<Size> = None;
        let mut fill: Option<FillPattern> = None;
        let mut exit_after: Option<Duration> = None;
        let mut exit_on_close = false;

        while let Some(flag) = args.next() {
            if BOOL_FLAGS.contains(&flag.as_str()) {
                match flag.as_str() {
                    "--exit-on-close" => exit_on_close = true,
                    // Not reachable: `BOOL_FLAGS` and this match cover the same set.
                    _ => return Err(format!("unknown flag `{flag}`")),
                }
                continue;
            }
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
                // Not reachable: `FLAGS` and this match cover the same set.
                "--exit-after" => exit_after = Some(parse_exit_after(&value)?),
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
            exit_on_close,
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

/// Connects, opens the toplevel, commits one frame and pumps until the exit condition.
fn run(args: &CliArgs) -> Result<(), String> {
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| {
        "WAYLAND_DISPLAY is not set; run the helper under a test runtime".to_string()
    })?;
    if display.is_empty() {
        return Err("WAYLAND_DISPLAY is empty; run the helper under a test runtime".to_string());
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

    let exit_rx = spawn_stdin_watcher()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| format!("cannot build the tokio runtime: {e}"))?;
    runtime.block_on(pump_until_exit(
        &mut client,
        &window,
        args.exit_on_close,
        args.exit_after,
        &exit_rx,
    ))?;

    // Teardown is best-effort: the compositor may already be gone, which is not a failure of
    // this helper. Diagnostics still reach stderr (the test inherits it) so a real protocol
    // bug stays visible instead of being swallowed.
    if let Err(e) = window.destroy() {
        eprintln!("adesk-test-app: warning: cannot destroy the toplevel: {e}");
    }
    if let Err(e) = client.flush() {
        eprintln!("adesk-test-app: warning: cannot flush the teardown: {e}");
    }
    if let Err(e) = runtime.block_on(client.close()) {
        eprintln!("adesk-test-app: warning: cannot close the wayland connection: {e}");
    }
    Ok(())
}

/// Spawns the thread that turns the `exit` stdin command into a channel notification.
///
/// Only the `exit` command sends: EOF (the parent closed stdin, or stdin is `/dev/null` for
/// a helper launched by the app registry) means no command can ever arrive again, so the
/// thread ends and the channel disconnects. The helper then keeps pumping until
/// `--exit-after` elapses or the process is signalled — it must never mistake "no stdin"
/// for "exit now", or a registry-launched helper would disappear immediately.
fn spawn_stdin_watcher() -> Result<Receiver<()>, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("adesk-test-app-stdin".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.lock().read_line(&mut line) {
                    // EOF: the parent closed stdin, nothing more can arrive.
                    Ok(0) => break,
                    Ok(_) => {
                        if line.trim() == EXIT_COMMAND {
                            let _ = tx.send(());
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("adesk-test-app: warning: cannot read stdin: {e}");
                        break;
                    }
                }
            }
        })
        .map_err(|e| format!("cannot start the stdin watcher thread: {e}"))?;
    Ok(rx)
}

/// Whether the helper has been asked to exit through the stdin `exit` command.
///
/// A disconnected channel means stdin reached EOF (or the watcher failed) and no further
/// command can arrive; that is not an exit request.
fn exit_requested(rx: &Receiver<()>) -> bool {
    matches!(rx.try_recv(), Ok(()))
}

/// Pumps events in short slices until the exit condition is met.
///
/// Exits on the stdin `exit` command, when `exit_after` elapses, when `exit_on_close` is set
/// and the compositor asked the window to close, and when the compositor closes the
/// connection (nothing left to pump). Any other pump error is a protocol failure.
async fn pump_until_exit(
    client: &mut WaylandTestClient,
    window: &TestWindow,
    exit_on_close: bool,
    exit_after: Option<Duration>,
    exit_rx: &Receiver<()>,
) -> Result<(), String> {
    let deadline = exit_after.map(|after| Instant::now() + after);
    loop {
        if exit_on_close && window.close_requested() {
            // The compositor asked us to close: leave the loop and run the ordinary teardown,
            // exactly as a real application would.
            return Ok(());
        }
        if exit_requested(exit_rx) {
            return Ok(());
        }
        let slice = match deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => PUMP_SLICE,
        };
        let slice = slice.min(PUMP_SLICE);
        if slice.is_zero() {
            // `--exit-after` elapsed.
            return Ok(());
        }
        match client.pump_for(slice).await {
            // The compositor closed the connection; the helper has nothing left to drive.
            Ok(stats) if stats.closed => return Ok(()),
            Ok(_) => {}
            Err(TestkitError::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(format!("wayland event pump failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CliArgs, String> {
        CliArgs::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn app_id_only_applies_the_documented_defaults() {
        let args = parse(&["--app-id", "org.example.demo"]).expect("--app-id alone is valid");
        assert_eq!(args.app_id, "org.example.demo");
        assert_eq!(
            args.title, "org.example.demo",
            "title defaults to the app id"
        );
        assert_eq!(args.size, DEFAULT_SIZE);
        assert_eq!(args.size, Size::new(640, 480));
        assert_eq!(args.fill, FillPattern::default());
        assert_eq!(args.exit_after, None);
        assert!(!args.exit_on_close, "--exit-on-close defaults to false");
    }

    #[test]
    fn flags_are_order_independent_and_last_occurrence_wins() {
        let args = parse(&[
            "--exit-after",
            "1500",
            "--title",
            "first",
            "--app-id",
            "org.example.demo",
            "--size",
            "10x20",
            "--title",
            "second",
            "--size",
            "0x0",
            "--app-id",
            "org.example.other",
        ])
        .expect("every flag is known and well formed");
        assert_eq!(args.app_id, "org.example.other");
        assert_eq!(args.title, "second");
        assert_eq!(args.size, Size::new(0, 0), "zero dimensions are allowed");
        assert_eq!(args.exit_after, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn size_parsing_accepts_two_u32s_and_rejects_the_rest() {
        let args = parse(&["--app-id", "a", "--size", "1280x800"]).expect("valid size");
        assert_eq!(args.size, Size::new(1280, 800));
        let args = parse(&["--app-id", "a", "--size", "4294967295x1"]).expect("u32::MAX is valid");
        assert_eq!(args.size, Size::new(u32::MAX, 1));

        for bad in [
            "1280",
            "1280x",
            "x800",
            "1280x800x2",
            "a x b",
            "-1x800",
            "1280x-1",
        ] {
            let err = parse(&["--app-id", "a", "--size", bad])
                .expect_err("malformed size must be a usage error");
            assert!(
                err.contains("invalid --size"),
                "size `{bad}` should report an --size error, got: {err}"
            );
        }
    }

    #[test]
    fn exit_after_parsing_takes_whole_milliseconds() {
        let args = parse(&["--app-id", "a", "--exit-after", "0"]).expect("zero is valid");
        assert_eq!(args.exit_after, Some(Duration::ZERO));
        let args = parse(&["--app-id", "a", "--exit-after", "250"]).expect("250 ms is valid");
        assert_eq!(args.exit_after, Some(Duration::from_millis(250)));

        for bad in ["", "250ms", "-1", "1.5", "abc"] {
            let err = parse(&["--app-id", "a", "--exit-after", bad])
                .expect_err("malformed --exit-after must be a usage error");
            assert!(
                err.contains("invalid --exit-after"),
                "value `{bad}` should report an --exit-after error, got: {err}"
            );
        }
    }

    #[test]
    fn exit_on_close_is_a_valueless_boolean_flag() {
        // Absent by default; the mere presence of the flag turns it on.
        let args = parse(&["--app-id", "a"]).expect("valid");
        assert!(!args.exit_on_close);
        let args = parse(&["--app-id", "a", "--exit-on-close"]).expect("valid");
        assert!(args.exit_on_close);

        // Repeating it is harmless (a bool has no "last occurrence" other than itself).
        let args = parse(&["--app-id", "a", "--exit-on-close", "--exit-on-close"]).expect("valid");
        assert!(args.exit_on_close);

        // It consumes no value: the argument that follows is parsed as its own flag.
        let args = parse(&["--app-id", "a", "--exit-on-close", "--title", "t"]).expect("valid");
        assert!(args.exit_on_close);
        assert_eq!(args.title, "t");

        // ... so a bare argument after it is still a usage error, proving no value was eaten.
        let err = parse(&["--app-id", "a", "--exit-on-close", "stray"])
            .expect_err("--exit-on-close must not consume the next argument");
        assert_eq!(err, "unexpected argument `stray`");
    }

    #[test]
    fn usage_errors_are_reported_before_usage() {
        let err = parse(&[]).expect_err("--app-id is required");
        assert_eq!(err, "missing required flag `--app-id`");

        let err = parse(&["--bogus", "x"]).expect_err("unknown flag");
        assert_eq!(err, "unknown flag `--bogus`");

        let err = parse(&["org.example.demo"]).expect_err("a bare argument is not a flag");
        assert_eq!(err, "unexpected argument `org.example.demo`");

        let err = parse(&["--app-id"]).expect_err("a missing value is an error");
        assert!(err.starts_with("flag `--app-id` requires a value"), "{err}");

        let err = parse(&["--app-id", "--title", "t"]).expect_err("a flag is not a value");
        assert!(
            err.starts_with("flag `--app-id` requires a value"),
            "a value that looks like a flag is a missing value: {err}"
        );

        // A usage error before the app id is still reported as-is.
        let err = parse(&["--size", "640x480"]).expect_err("--app-id is required");
        assert_eq!(err, "missing required flag `--app-id`");
    }

    #[test]
    fn fill_flag_decodes_via_from_cli_arg_and_last_occurrence_wins() {
        let args = parse(&["--app-id", "a", "--fill", "solid:112233"]).expect("valid fill");
        assert_eq!(args.fill, FillPattern::Solid([0x11, 0x22, 0x33, 0xff]));

        let args = parse(&[
            "--app-id",
            "a",
            "--fill",
            "solid:112233",
            "--fill",
            "checker:4:112233:445566",
        ])
        .expect("valid fills");
        assert_eq!(
            args.fill,
            FillPattern::checker(4, [0x11, 0x22, 0x33, 0xff], [0x44, 0x55, 0x66, 0xff])
        );

        let err = parse(&["--app-id", "a", "--fill", "bogus"])
            .expect_err("a malformed fill must be a usage error");
        assert!(err.starts_with("invalid --fill value `bogus`"), "{err}");
    }
}
