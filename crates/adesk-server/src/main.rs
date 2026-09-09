//! `adesk-server` binary: CLI flags → [`ServerConfig`] → [`Server::start`].
//!
//! Every flag has an `ADESK_*` environment fallback (`clap`'s `env`), so the
//! binary is usable both from a shell and from a service unit.

use std::path::PathBuf;

use clap::Parser;

use adesk_server::Server;
use adesk_server::config::{ServerConfig, parse_renderer, parse_size};

/// ADesk — AI-native headless Wayland runtime (AGP server).
#[derive(Debug, Parser)]
#[command(name = "adesk-server", version, about, long_about = None)]
struct Cli {
    /// AGP Unix socket path.
    #[arg(long, env = "ADESK_SOCKET", value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Virtual output size, e.g. `1280x800`.
    #[arg(long, env = "ADESK_OUTPUT", value_name = "WxH")]
    output: Option<String>,
    /// Renderer: `auto`, `gl` or `pixman`.
    #[arg(long, env = "ADESK_RENDERER", value_name = "KIND")]
    renderer: Option<String>,
    /// xkb layout list (e.g. `us`, `de,us`).
    #[arg(long, env = "ADESK_XKB_LAYOUT", value_name = "NAME")]
    xkb_layout: Option<String>,
    /// xkb variant list.
    #[arg(long, env = "ADESK_XKB_VARIANT", value_name = "NAME")]
    xkb_variant: Option<String>,
    /// xkb model.
    #[arg(long, env = "ADESK_XKB_MODEL", value_name = "NAME")]
    xkb_model: Option<String>,
    /// xkb rules file.
    #[arg(long, env = "ADESK_XKB_RULES", value_name = "NAME")]
    xkb_rules: Option<String>,
    /// Extra `.desktop` search directories (repeatable, `:`-separated in the env).
    #[arg(long = "apps-dir", env = "ADESK_APPS_DIR", value_name = "DIR", value_delimiter = ':')]
    apps_dir: Vec<PathBuf>,
    /// `tracing-subscriber` env-filter directive.
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    log: String,
}

fn main() -> anyhow::Result<()> {
    todo!()
}

/// Builds a [`ServerConfig`] from parsed CLI flags (pure, unit-testable).
fn build_config(cli: &Cli) -> anyhow::Result<ServerConfig> {
    todo!()
}

/// Installs the global `tracing-subscriber` with the given filter.
fn init_tracing(filter: &str) {
    todo!()
}
