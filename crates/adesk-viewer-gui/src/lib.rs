//! ADesk viewer GUI — a GTK4/libadwaita desktop front-end for the VAP viewer.
//!
//! This crate is the human-facing projection of an ADesk runtime: it connects to
//! the runtime's viewer endpoint over the Viewer Attachment Protocol (VAP,
//! `docs/viewer.md`), renders the streamed desktop frames, and turns local
//! mouse/keyboard activity into VAP input so the human is placed in the same
//! seat the agent drives — a viewer action is never a special code path.
//!
//! It pairs with the headless `adesk-viewer` binary (frames → PNG, scripted
//! input); this crate owns the interactive GUI instead.
//!
//! The crate is split into GTK-free, unit-testable modules and a GTK layer that
//! builds the widgets on top of them:
//! - [`cli`] — command-line parsing and viewer-endpoint resolution.
//! - [`error`] — the crate error type [`GuiError`] and the [`Result`] alias.
//! - [`image`] — decoding VAP image payloads to tightly packed RGBA8.
//! - [`mapping`] — the pure widget ↔ normalized coordinate letterbox math.
//! - [`taskbar`] — the pure task-bar view model derived from a desktop state.
//! - [`address`] — failure-message composition that names the dialed endpoint,
//!   with a targeted hint when the path looks like the AGP socket.
//!
//! The GTK-facing modules are crate-private: `keystroke` (the pure keystroke
//! routing state machine), `record` (the pure recording-control state machine),
//! `bridge` (the tokio ↔ GLib bridge), `frame_view` and `task_bar_view` (the
//! widgets) and `app` (the application and event loop). The public entry point
//! is [`run`].
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod address;
pub mod cli;
pub mod error;
pub mod image;
pub mod mapping;
pub mod taskbar;

mod app;
mod bridge;
mod frame_view;
mod keystroke;
mod record;
mod task_bar_view;

pub use error::{GuiError, Result};

use clap::Parser;

/// Parses the command line, installs logging, resolves the viewer endpoint and
/// runs the GTK application.
///
/// The subscriber is installed from [`cli::Cli::log`] (also `ADESK_LOG`) as an
/// env-filter; an invalid directive falls back to `info`, and an already
/// installed subscriber is ignored rather than panicking. A configuration error
/// (e.g. an unparseable `--tcp`) is reported and mapped to a failing
/// `glib::ExitCode`; a `clap` usage error exits the process first.
///
/// GTK is then invoked with the program name only, so the `GApplication`
/// option parser never sees the real `argv` again: handing it the process
/// arguments would make it re-parse (and reject) the `--unix`/`--tcp`/`--log`
/// flags clap has already consumed.
pub fn run() -> gtk4::glib::ExitCode {
    let cli = cli::Cli::parse();
    install_logging(&cli.log);

    let target = match cli.target() {
        Ok(target) => target,
        Err(error) => {
            tracing::error!(%error, "invalid viewer endpoint");
            return gtk4::glib::ExitCode::FAILURE;
        }
    };

    app::run_application(target, &app::program_name())
}

/// Installs the `tracing-subscriber` from `filter`, tolerating a bad directive
/// and an already-installed subscriber.
fn install_logging(filter: &str) {
    let env_filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}
