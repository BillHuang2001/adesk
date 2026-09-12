//! Headless `adesk-viewer` binary — a Viewer Attachment Protocol (VAP) client.
//!
//! The binary connects to an ADesk runtime's viewer endpoint over a Unix socket
//! (or TCP), performs the §2 handshake, and then does exactly one of four
//! things (`docs/viewer.md`):
//! - `--capture <FILE>`: request one frame and write it as a PNG;
//! - `--follow --out-dir <DIR>`: stream frames into `DIR` as `frame-<seq:08>.png`
//!   until a frame/time budget is reached or the stream ends;
//! - `--input <FILE>` / `--input-stdin`: run a small input script
//!   (see `crates/adesk-viewer/CONTEXT.md`, parsed by `parse_script`) against the
//!   runtime;
//! - `--record <FILE>`: start a screen recording of the output and finish it when
//!   a `--duration-ms` budget elapses or the process receives Ctrl-C.
//!
//! It never needs a display, GPU or network: "rendering" one frame means writing
//! a PNG and input is script-driven. Message bodies and pixel payloads are never
//! logged.
//!
//! Exit codes: `0` success; `1` on a runtime failure (a `ViewerError` or an I/O
//! error while connecting, streaming or capturing); `2` on a CLI/config error (a
//! bad mode combination, a missing `--out-dir`, an unparseable `--tcp`, an
//! unknown overlay name, an unreadable `--input`, or a script parse error). A
//! `clap` usage error already exits `2`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{ArgGroup, Parser};
use futures::StreamExt;

use adesk_core::{ButtonState, OverlayKind, WindowId};
use adesk_viewer::{
    parse_script, save_frame_png, ConnectOptions, FrameWriter, RecordRequest, ScriptCommand,
    ViewerClient, ViewerError, ViewerTarget,
};
use adesk_viewer_proto::{RecordingEncoder, DEFAULT_RECORD_FPS};

/// Process exit code for a runtime failure (connect/capture/stream/input error).
const EXIT_FAILURE: u8 = 1;

/// Process exit code for a CLI/config error.
const EXIT_USAGE: u8 = 2;

/// The viewer name sent in the handshake `hello` (`docs/viewer.md` §2).
const CLIENT_NAME: &str = "adesk-viewer";

/// The Unix socket file name used under `$XDG_RUNTIME_DIR` (`docs/viewer.md` §1).
const SOCKET_FILE_NAME: &str = "adesk-viewer.sock";

/// `adesk-viewer` — headless Viewer Attachment Protocol client.
///
/// Connects to an ADesk runtime, then captures one frame (`--capture`), streams
/// frames to a directory (`--follow`), runs an input script (`--input` /
/// `--input-stdin`) or records the output (`--record`). Exactly one of those
/// modes is required.
#[derive(Debug, Parser)]
#[command(name = "adesk-viewer", version, long_about = None)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .multiple(false)
        .args(["capture", "follow", "input", "input_stdin", "record"])
))]
struct Cli {
    /// Connect over a Unix domain socket at PATH (default:
    /// `$XDG_RUNTIME_DIR/adesk-viewer.sock`, else `<temp_dir>/adesk-viewer.sock`).
    #[arg(long, value_name = "PATH")]
    unix: Option<PathBuf>,

    /// Connect over TCP instead of Unix, to `HOST:PORT`.
    #[arg(long, value_name = "HOST:PORT", conflicts_with = "unix")]
    tcp: Option<String>,

    /// Frame pacing: stream at most N frames per second (`0` = unpaced).
    #[arg(long, value_name = "N")]
    fps: Option<u64>,

    /// Debug overlays to request, comma-separated
    /// (`window_ids,app_ids,focus,damage,surface_bounds,cursor,actions,commit_timing`).
    #[arg(long, value_name = "LIST")]
    overlays: Option<String>,

    /// `tracing-subscriber` env-filter directive (also read from `ADESK_LOG`).
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    log: String,

    /// Capture one frame to FILE, then exit.
    #[arg(long, value_name = "FILE")]
    capture: Option<PathBuf>,

    /// Stream frames into `--out-dir` until a budget is reached or the stream ends.
    #[arg(long)]
    follow: bool,

    /// Directory `--follow` writes `frame-<seq:08>.png` files into.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// With `--follow`, stop after N frames have been written.
    #[arg(long, value_name = "N")]
    max_frames: Option<u64>,

    /// With `--follow` or `--record`, stop after M milliseconds have elapsed.
    #[arg(long, value_name = "M")]
    duration_ms: Option<u64>,

    /// Run the input script in FILE against the runtime, then exit.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,

    /// Run the input script read from standard input, then exit.
    #[arg(long)]
    input_stdin: bool,

    /// Record the desktop output to FILE until a `--duration-ms` budget elapses
    /// or the process receives Ctrl-C, then exit.
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,

    /// With `--record`, the recording frame rate in frames per second.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_RECORD_FPS)]
    record_fps: u32,

    /// With `--record`, the encoder to request (`auto`, `software`, `gpu`).
    #[arg(long, value_name = "ENCODER", default_value = "auto")]
    record_encoder: String,
}

/// Why a run could not complete, split by the exit code it maps to.
#[derive(Debug)]
enum Failure {
    /// A runtime failure (`ViewerError` or an I/O error on the wire): exit code `1`.
    Runtime(ViewerError),
    /// A CLI/config error: exit code `2`.
    Config(String),
}

impl From<ViewerError> for Failure {
    fn from(error: ViewerError) -> Failure {
        Failure::Runtime(error)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Runtime(error) => write!(f, "{error}"),
            Failure::Config(message) => f.write_str(message),
        }
    }
}

impl Failure {
    /// The process exit code this failure maps to: `1` for a runtime/connection
    /// failure, `2` for a CLI/config error.
    fn exit_code(&self) -> u8 {
        match self {
            Failure::Runtime(_) => EXIT_FAILURE,
            Failure::Config(_) => EXIT_USAGE,
        }
    }
}

/// The single capture/input/record mode the CLI selected.
#[derive(Debug)]
enum Mode {
    /// Request one frame and write it to the given file.
    Capture(PathBuf),
    /// Stream frames into a directory with an optional frame/time budget.
    Follow {
        /// Directory frames are written into.
        dir: PathBuf,
        /// Stop after this many frames, when set.
        max_frames: Option<u64>,
        /// Stop after this many milliseconds, when set.
        duration_ms: Option<u64>,
    },
    /// Run a parsed input script.
    Script {
        /// The commands to execute in order.
        commands: Vec<ScriptCommand>,
    },
    /// Record the desktop output to a file with an optional time budget.
    Record {
        /// Destination file.
        file: PathBuf,
        /// Recording frame rate in frames per second.
        fps: u32,
        /// Encoder preference.
        encoder: RecordingEncoder,
        /// Stop after this many milliseconds, when set.
        duration_ms: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log);
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("adesk-viewer: {failure}");
            ExitCode::from(failure.exit_code())
        }
    }
}

/// Installs the global `tracing-subscriber` with `filter`.
///
/// An invalid directive falls back to `info`; a subscriber that is already
/// installed is left alone (never panics).
fn init_tracing(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Resolves the connection target, builds the options, selects a mode and runs it.
async fn run(cli: &Cli) -> Result<(), Failure> {
    let target = resolve_target(cli)?;
    let options = build_options(cli, target)?;
    match select_mode(cli)? {
        Mode::Capture(path) => capture_once(&options, &path).await,
        Mode::Follow {
            dir,
            max_frames,
            duration_ms,
        } => follow(&options, &dir, max_frames, duration_ms).await,
        Mode::Script { commands } => apply_script(&options, commands).await,
        Mode::Record {
            file,
            fps,
            encoder,
            duration_ms,
        } => {
            let request = RecordRequest::new()
                .with_path(file)
                .with_fps(fps)
                .with_encoder(encoder);
            record(&options, request, duration_ms).await
        }
    }
}

/// Resolves the viewer endpoint from `--unix`/`--tcp`, defaulting to the
/// runtime-derived Unix socket path when neither is given.
fn resolve_target(cli: &Cli) -> Result<ViewerTarget, Failure> {
    if let Some(path) = &cli.unix {
        return Ok(ViewerTarget::Unix(path.clone()));
    }
    if let Some(address) = &cli.tcp {
        let address = address.parse::<SocketAddr>().map_err(|error| {
            Failure::Config(format!("invalid --tcp address `{address}`: {error}"))
        })?;
        return Ok(ViewerTarget::Tcp(address));
    }
    Ok(ViewerTarget::Unix(default_socket_path()))
}

/// The default Unix socket path: `$XDG_RUNTIME_DIR/adesk-viewer.sock` when the
/// variable is set and non-empty, otherwise `<temp_dir>/adesk-viewer.sock`.
fn default_socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(SOCKET_FILE_NAME),
        _ => std::env::temp_dir().join(SOCKET_FILE_NAME),
    }
}

/// Builds the `ConnectOptions` from the CLI flags.
fn build_options(cli: &Cli, target: ViewerTarget) -> Result<ConnectOptions, Failure> {
    let mut options = ConnectOptions::new(target).with_client_name(CLIENT_NAME);
    if let Some(list) = &cli.overlays {
        options = options.with_overlays(parse_overlays(list)?);
    }
    if let Some(fps) = cli.fps {
        options = options.with_min_interval_ms(min_interval_ms(fps));
    }
    Ok(options)
}

/// Maps a `--fps` value to a frame pacing interval: `0` is unpaced, otherwise
/// `1000 / N` milliseconds between frames (`docs/viewer.md` §2).
fn min_interval_ms(fps: u64) -> u64 {
    1000u64.checked_div(fps).unwrap_or_default()
}

/// Parses the comma-separated `--overlays` list into [`OverlayKind`]s.
///
/// Blank entries (including the empty string) are ignored; an unknown name is a
/// config error.
fn parse_overlays(list: &str) -> Result<Vec<OverlayKind>, Failure> {
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            serde_json::from_value::<OverlayKind>(serde_json::Value::String(name.to_owned()))
                .map_err(|_| Failure::Config(format!("unknown overlay `{name}`")))
        })
        .collect()
}

/// Parses the `--record-encoder` value into a [`RecordingEncoder`].
///
/// The wire names are `"auto"`/`"software"`/`"gpu"`; an unknown name is a config
/// error.
fn parse_encoder(name: &str) -> Result<RecordingEncoder, Failure> {
    serde_json::from_value::<RecordingEncoder>(serde_json::Value::String(name.trim().to_owned()))
        .map_err(|_| Failure::Config(format!("unknown encoder `{name}`")))
}

/// Selects the capture/input/record mode, reading and parsing the script source
/// when the input modes were chosen.
fn select_mode(cli: &Cli) -> Result<Mode, Failure> {
    if let Some(path) = &cli.capture {
        return Ok(Mode::Capture(path.clone()));
    }
    if cli.follow {
        let dir = cli
            .out_dir
            .clone()
            .ok_or_else(|| Failure::Config("--follow requires --out-dir <DIR>".to_owned()))?;
        return Ok(Mode::Follow {
            dir,
            max_frames: cli.max_frames,
            duration_ms: cli.duration_ms,
        });
    }
    if let Some(file) = &cli.record {
        return Ok(Mode::Record {
            file: file.clone(),
            fps: cli.record_fps,
            encoder: parse_encoder(&cli.record_encoder)?,
            duration_ms: cli.duration_ms,
        });
    }
    let source = read_script_source(cli)?;
    let commands =
        parse_script(&source).map_err(|error| Failure::Config(format!("input script: {error}")))?;
    Ok(Mode::Script { commands })
}

/// Reads the input script from `--input <FILE>` or standard input.
fn read_script_source(cli: &Cli) -> Result<String, Failure> {
    if cli.input_stdin {
        std::io::read_to_string(std::io::stdin()).map_err(|error| {
            Failure::Config(format!("cannot read the input script from stdin: {error}"))
        })
    } else if let Some(path) = &cli.input {
        std::fs::read_to_string(path).map_err(|error| {
            Failure::Config(format!(
                "cannot read the input script {}: {error}",
                path.display()
            ))
        })
    } else {
        Err(Failure::Config(
            "no capture or input mode selected".to_owned(),
        ))
    }
}

/// Connects, requests exactly one frame, writes it to `path`, then closes.
async fn capture_once(options: &ConnectOptions, path: &Path) -> Result<(), Failure> {
    let client = ViewerClient::connect_with(options.clone()).await?;
    let frame = client.request_frame().await?;
    save_frame_png(&frame.image, path)?;
    client.close().await?;
    Ok(())
}

/// Connects and writes pushed frames into `dir` until a budget is reached or the
/// frame stream ends.
async fn follow(
    options: &ConnectOptions,
    dir: &Path,
    max_frames: Option<u64>,
    duration_ms: Option<u64>,
) -> Result<(), Failure> {
    let client = ViewerClient::connect_with(options.clone()).await?;
    let writer = FrameWriter::new(dir);
    let mut frames = std::pin::pin!(client.frames());
    let deadline = duration_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    let mut written: u64 = 0;
    loop {
        if max_frames.is_some_and(|limit| written >= limit) {
            break;
        }
        let next = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, frames.next()).await {
                    Ok(frame) => frame,
                    Err(_elapsed) => break,
                }
            }
            None => frames.next().await,
        };
        match next {
            Some(Ok(frame)) => {
                writer.write(&frame)?;
                written += 1;
            }
            Some(Err(error)) => return Err(Failure::Runtime(error)),
            None => break,
        }
    }
    tracing::debug!(frames = written, "viewer capture stream finished");
    client.close().await?;
    Ok(())
}

/// Connects, starts a recording of the output, records until `duration_ms`
/// elapses (when set) or the process receives Ctrl-C, then stops and closes
/// (`docs/viewer.md` §4, §5).
///
/// The runtime owns the file: `request.path` is the requested destination and the
/// status the server returns reports the resolved path and the encoder actually in
/// use, which are logged here.
async fn record(
    options: &ConnectOptions,
    request: RecordRequest,
    duration_ms: Option<u64>,
) -> Result<(), Failure> {
    let client = ViewerClient::connect_with(options.clone()).await?;
    let started = client.start_recording(request).await?;
    tracing::info!(
        recording = started.recording,
        path = ?started.path,
        encoder = ?started.encoder,
        fps = started.fps,
        "viewer recording started"
    );

    wait_for_recording_stop(duration_ms).await;

    let finished = client.stop_recording().await?;
    tracing::info!(
        frames = finished.frames,
        duration_ms = finished.duration_ms,
        path = ?finished.path,
        "viewer recording stopped"
    );
    client.close().await?;
    Ok(())
}

/// Resolves when the recording should stop: after `duration_ms` when a budget is
/// set, or on Ctrl-C, whichever comes first. With no budget it waits only for
/// Ctrl-C.
///
/// If the Ctrl-C handler cannot be installed the time budget is awaited instead
/// (waiting forever when no budget is set), so a failure to install the handler
/// never truncates a recording.
async fn wait_for_recording_stop(duration_ms: Option<u64>) {
    let budget = async move {
        match duration_ms {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(budget);
    tokio::select! {
        () = &mut budget => {}
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                tracing::debug!(%error, "could not install the Ctrl-C handler");
                budget.await;
            }
        }
    }
}

/// Connects and executes a parsed input script in order, then closes.
async fn apply_script(
    options: &ConnectOptions,
    commands: Vec<ScriptCommand>,
) -> Result<(), Failure> {
    let client = ViewerClient::connect_with(options.clone()).await?;
    for command in commands {
        apply(&client, command).await?;
    }
    client.close().await?;
    Ok(())
}

/// Turns one [`ScriptCommand`] into VAP input messages (`docs/viewer.md` §4).
///
/// `click` is a `Pressed` then a `Released` on the same button; `capture`
/// requests and writes a fresh frame.
async fn apply(client: &ViewerClient, command: ScriptCommand) -> Result<(), ViewerError> {
    match command {
        ScriptCommand::Move { x, y } => client.pointer_move(x, y).await,
        ScriptCommand::Click { button } => {
            client
                .pointer_button(button, ButtonState::Pressed, None)
                .await?;
            client
                .pointer_button(button, ButtonState::Released, None)
                .await
        }
        ScriptCommand::Down { button } => {
            client
                .pointer_button(button, ButtonState::Pressed, None)
                .await
        }
        ScriptCommand::Up { button } => {
            client
                .pointer_button(button, ButtonState::Released, None)
                .await
        }
        ScriptCommand::Scroll { dx, dy } => client.scroll(dx, dy, None).await,
        ScriptCommand::Key { keys, action } => client.key(keys, action).await,
        ScriptCommand::Text { text } => client.text(text).await,
        ScriptCommand::ActivateWindow { window_id } => {
            client.activate_window(WindowId(window_id)).await
        }
        ScriptCommand::Control { owner } => client.set_control(owner).await,
        ScriptCommand::Wait { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
        ScriptCommand::Capture { path } => {
            let frame = client.request_frame().await?;
            save_frame_png(&frame.image, &path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `args` (with the program name first) as the CLI.
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn fps_maps_to_a_frame_interval() {
        assert_eq!(min_interval_ms(0), 0);
        assert_eq!(min_interval_ms(1), 1000);
        assert_eq!(min_interval_ms(10), 100);
        assert_eq!(min_interval_ms(50), 20);
    }

    #[test]
    fn overlays_parse_every_wire_name() {
        let overlays = parse_overlays(
            "window_ids,app_ids,focus,damage,surface_bounds,cursor,actions,commit_timing",
        )
        .unwrap();
        assert_eq!(
            overlays,
            vec![
                OverlayKind::WindowIds,
                OverlayKind::AppIds,
                OverlayKind::Focus,
                OverlayKind::Damage,
                OverlayKind::SurfaceBounds,
                OverlayKind::Cursor,
                OverlayKind::Actions,
                OverlayKind::CommitTiming,
            ]
        );
    }

    #[test]
    fn overlays_ignore_whitespace_and_blank_entries() {
        assert_eq!(
            parse_overlays(" cursor ,, focus ").unwrap(),
            vec![OverlayKind::Cursor, OverlayKind::Focus]
        );
        assert!(parse_overlays("").unwrap().is_empty());
    }

    #[test]
    fn unknown_overlay_is_a_config_error() {
        assert!(matches!(
            parse_overlays("cursor,nonsense").unwrap_err(),
            Failure::Config(_)
        ));
    }

    #[test]
    fn cli_requires_exactly_one_mode() {
        assert!(parse(&["adesk-viewer"]).is_err());
        assert!(parse(&["adesk-viewer", "--capture", "f.png"]).is_ok());
        assert!(parse(&["adesk-viewer", "--follow", "--out-dir", "dir"]).is_ok());
        assert!(parse(&["adesk-viewer", "--input", "script.txt"]).is_ok());
        assert!(parse(&["adesk-viewer", "--input-stdin"]).is_ok());
        assert!(parse(&["adesk-viewer", "--record", "out.mkv"]).is_ok());
    }

    #[test]
    fn cli_rejects_two_modes() {
        assert!(parse(&[
            "adesk-viewer",
            "--capture",
            "f.png",
            "--follow",
            "--out-dir",
            "dir"
        ])
        .is_err());
        assert!(parse(&["adesk-viewer", "--record", "out.mkv", "--capture", "f.png"]).is_err());
    }

    #[test]
    fn cli_rejects_two_transports() {
        assert!(parse(&[
            "adesk-viewer",
            "--unix",
            "/tmp/x.sock",
            "--tcp",
            "127.0.0.1:1",
            "--capture",
            "f.png"
        ])
        .is_err());
    }

    #[test]
    fn resolve_target_prefers_the_explicit_transport() {
        let cli = parse(&[
            "adesk-viewer",
            "--unix",
            "/tmp/viewer.sock",
            "--capture",
            "f.png",
        ])
        .unwrap();
        assert_eq!(
            resolve_target(&cli).unwrap(),
            ViewerTarget::Unix(PathBuf::from("/tmp/viewer.sock"))
        );

        let cli = parse(&[
            "adesk-viewer",
            "--tcp",
            "127.0.0.1:7100",
            "--capture",
            "f.png",
        ])
        .unwrap();
        assert_eq!(
            resolve_target(&cli).unwrap(),
            ViewerTarget::Tcp("127.0.0.1:7100".parse().unwrap())
        );
    }

    #[test]
    fn resolve_target_rejects_a_bad_tcp_address() {
        let cli = parse(&[
            "adesk-viewer",
            "--tcp",
            "not-an-address",
            "--capture",
            "f.png",
        ])
        .unwrap();
        assert!(matches!(
            resolve_target(&cli).unwrap_err(),
            Failure::Config(_)
        ));
    }

    #[test]
    fn build_options_applies_fps_overlays_and_name() {
        let cli = parse(&[
            "adesk-viewer",
            "--capture",
            "f.png",
            "--fps",
            "10",
            "--overlays",
            "cursor,focus",
        ])
        .unwrap();
        let options =
            build_options(&cli, ViewerTarget::Unix(PathBuf::from("/tmp/x.sock"))).unwrap();
        assert_eq!(options.min_interval_ms, 100);
        assert_eq!(
            options.overlays,
            vec![OverlayKind::Cursor, OverlayKind::Focus]
        );
        assert_eq!(options.client_name.as_deref(), Some(CLIENT_NAME));
    }

    #[test]
    fn follow_requires_an_output_directory() {
        let cli = parse(&["adesk-viewer", "--follow"]).unwrap();
        assert!(matches!(select_mode(&cli).unwrap_err(), Failure::Config(_)));
    }

    #[test]
    fn select_mode_parses_an_input_script() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.txt");
        std::fs::write(&path, "move 0.5 0.5\nclick\n").unwrap();
        let cli = parse(&["adesk-viewer", "--input", path.to_str().unwrap()]).unwrap();
        match select_mode(&cli).unwrap() {
            Mode::Script { commands } => assert_eq!(commands.len(), 2),
            other => panic!("expected a script mode, got {other:?}"),
        }
    }

    #[test]
    fn select_mode_reports_a_script_parse_error_as_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.txt");
        std::fs::write(&path, "frobnicate\n").unwrap();
        let cli = parse(&["adesk-viewer", "--input", path.to_str().unwrap()]).unwrap();
        assert!(matches!(select_mode(&cli).unwrap_err(), Failure::Config(_)));
    }

    #[test]
    fn select_mode_reports_an_unreadable_input_as_config() {
        let cli = parse(&[
            "adesk-viewer",
            "--input",
            "/nonexistent/adesk-viewer-script",
        ])
        .unwrap();
        assert!(matches!(select_mode(&cli).unwrap_err(), Failure::Config(_)));
    }

    #[test]
    fn select_mode_records_the_follow_budget() {
        let cli = parse(&[
            "adesk-viewer",
            "--follow",
            "--out-dir",
            "frames",
            "--max-frames",
            "5",
            "--duration-ms",
            "2000",
        ])
        .unwrap();
        match select_mode(&cli).unwrap() {
            Mode::Follow {
                dir,
                max_frames,
                duration_ms,
            } => {
                assert_eq!(dir, PathBuf::from("frames"));
                assert_eq!(max_frames, Some(5));
                assert_eq!(duration_ms, Some(2000));
            }
            other => panic!("expected a follow mode, got {other:?}"),
        }
    }

    #[test]
    fn parse_encoder_maps_every_wire_name() {
        assert_eq!(parse_encoder("auto").unwrap(), RecordingEncoder::Auto);
        assert_eq!(
            parse_encoder("software").unwrap(),
            RecordingEncoder::Software
        );
        assert_eq!(parse_encoder("gpu").unwrap(), RecordingEncoder::Gpu);
        assert_eq!(parse_encoder("  gpu ").unwrap(), RecordingEncoder::Gpu);
    }

    #[test]
    fn unknown_encoder_is_a_config_error() {
        assert!(matches!(
            parse_encoder("nonsense").unwrap_err(),
            Failure::Config(_)
        ));
    }

    #[test]
    fn select_mode_records_the_record_defaults() {
        let cli = parse(&["adesk-viewer", "--record", "out.mkv"]).unwrap();
        match select_mode(&cli).unwrap() {
            Mode::Record {
                file,
                fps,
                encoder,
                duration_ms,
            } => {
                assert_eq!(file, PathBuf::from("out.mkv"));
                assert_eq!(fps, DEFAULT_RECORD_FPS);
                assert_eq!(encoder, RecordingEncoder::Auto);
                assert_eq!(duration_ms, None);
            }
            other => panic!("expected a record mode, got {other:?}"),
        }
    }

    #[test]
    fn select_mode_records_the_record_budget_and_encoder() {
        let cli = parse(&[
            "adesk-viewer",
            "--record",
            "out.mkv",
            "--record-fps",
            "15",
            "--record-encoder",
            "software",
            "--duration-ms",
            "3000",
        ])
        .unwrap();
        match select_mode(&cli).unwrap() {
            Mode::Record {
                file,
                fps,
                encoder,
                duration_ms,
            } => {
                assert_eq!(file, PathBuf::from("out.mkv"));
                assert_eq!(fps, 15);
                assert_eq!(encoder, RecordingEncoder::Software);
                assert_eq!(duration_ms, Some(3000));
            }
            other => panic!("expected a record mode, got {other:?}"),
        }
    }

    #[test]
    fn select_mode_reports_a_bad_record_encoder_as_config() {
        let cli = parse(&[
            "adesk-viewer",
            "--record",
            "out.mkv",
            "--record-encoder",
            "nonsense",
        ])
        .unwrap();
        assert!(matches!(select_mode(&cli).unwrap_err(), Failure::Config(_)));
    }

    #[test]
    fn failure_exit_codes_are_fixed() {
        assert_eq!(
            Failure::Runtime(ViewerError::Closed).exit_code(),
            EXIT_FAILURE
        );
        assert_eq!(
            Failure::Config("bad flag".to_owned()).exit_code(),
            EXIT_USAGE
        );
    }

    #[test]
    fn failure_display_renders_the_underlying_message() {
        assert_eq!(
            Failure::Runtime(ViewerError::Closed).to_string(),
            ViewerError::Closed.to_string()
        );
        assert_eq!(
            Failure::Config("bad flag".to_owned()).to_string(),
            "bad flag"
        );
    }
}
