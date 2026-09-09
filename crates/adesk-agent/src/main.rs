//! `adesk-agent` — multimodal GUI agent prototype (CLI).
//!
//! Wires CLI → provider → client → loop and writes the run report.
//!
//! ```text
//! adesk-agent --provider mock --scenario click
//! adesk-agent --provider openai --task "Open the settings dialog and enable dark mode" \
//!             --max-steps 30 --report runs/dark-mode.json
//! ```
//!
//! Exit codes: `0` task succeeded and all expectations passed, `1` the task or a
//! scenario expectation failed, `2` configuration/startup error.

use std::path::PathBuf;

use clap::Parser;

use adesk_agent::{ProviderKind, ScenarioId};

/// Multimodal GUI agent prototype for the ADesk runtime.
#[derive(Debug, Parser)]
#[command(name = "adesk-agent", version, about, long_about = None)]
struct Cli {
    /// AGP Unix socket path (default: `$ADESK_SOCKET` or `$XDG_RUNTIME_DIR/adesk.sock`).
    #[arg(long, env = "ADESK_SOCKET")]
    socket: Option<PathBuf>,

    /// LLM provider backend.
    #[arg(
        long,
        value_enum,
        env = "ADESK_AGENT_PROVIDER",
        default_value = "mock"
    )]
    provider: ProviderKind,

    /// Free-form task goal (mutually exclusive with `--scenario`).
    #[arg(long, conflicts_with = "scenario")]
    task: Option<String>,

    /// Built-in scenario to run instead of a free-form task.
    #[arg(long, value_enum)]
    scenario: Option<ScenarioId>,

    /// Maximum number of loop steps.
    #[arg(long, default_value_t = 20)]
    max_steps: u32,

    /// Write the run report (JSON) to this path.
    #[arg(long, value_name = "PATH")]
    report: Option<PathBuf>,

    /// Model name for the OpenAI-compatible provider.
    #[arg(long, env = "ADESK_AGENT_MODEL")]
    model: Option<String>,

    /// Base URL of the OpenAI-compatible endpoint.
    #[arg(long, env = "ADESK_AGENT_BASE_URL")]
    base_url: Option<String>,

    /// API key (falls back to `ADESK_AGENT_API_KEY` / `OPENAI_API_KEY`).
    #[arg(long, env = "ADESK_AGENT_API_KEY")]
    api_key: Option<String>,

    /// Maximum image dimension sent to the provider (downscale target).
    #[arg(long, default_value_t = 1024)]
    max_dimension: u32,

    /// Quiet window (ms) used when observing after an input action.
    #[arg(long, default_value_t = 250)]
    quiet_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing();
    run(cli).await
}

/// Install `tracing-subscriber` with `ADESK_LOG` (env-filter), defaulting to `info`.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("ADESK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

/// Resolve the task, build the provider and client, drive the loop, emit the report.
async fn run(cli: Cli) -> anyhow::Result<()> {
    let _ = cli;
    todo!("phase 2: wire provider config -> AgpClient -> AgentLoop/ScenarioRunner")
}
