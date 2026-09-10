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
use std::process::ExitCode;

use anyhow::{bail, Context as _, Result as AnyResult};
use async_trait::async_trait;
use clap::Parser;
use tracing::{error, info, warn};

use adesk_agent::{
    AgentContext, AgentDecision, AgentLoop, AgpClient, ContextBudget, LlmProvider, LoopConfig,
    ProviderConfig, ProviderError, ProviderKind, RunReport, Scenario, ScenarioId, ScenarioRunner,
    TaskDescription,
};

/// Multimodal GUI agent prototype for the ADesk runtime.
#[derive(Debug, Parser)]
#[command(name = "adesk-agent", version, about, long_about = None)]
struct Cli {
    /// AGP Unix socket path (default: `$ADESK_SOCKET`, else `$XDG_RUNTIME_DIR/adesk.sock`, else `<temp dir>/adesk.sock`).
    #[arg(long, env = "ADESK_SOCKET")]
    socket: Option<PathBuf>,

    /// LLM provider backend.
    #[arg(long, value_enum, env = "ADESK_AGENT_PROVIDER", default_value = "mock")]
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
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing();
    match run(cli).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            warn!("run did not meet its success criteria");
            ExitCode::from(1)
        }
        Err(err) => {
            error!("{err:#}");
            ExitCode::from(2)
        }
    }
}

/// Install `tracing-subscriber` with `ADESK_LOG` (env-filter), defaulting to `info`.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("ADESK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

/// What the run should execute: a free-form task or a built-in scenario.
enum RunTarget {
    /// `--task "<goal>"`.
    Task(TaskDescription),
    /// `--scenario <id>` (boxed: a [`Scenario`] carries its script and expectations).
    Scenario(Box<Scenario>),
}

/// Resolve the task, build the provider and client, drive the loop, emit the report.
///
/// Returns whether the task succeeded / every scenario expectation passed.
async fn run(cli: Cli) -> AnyResult<bool> {
    let config = loop_config(&cli);
    let provider_config = provider_config(&cli);
    let socket = resolve_socket(&cli);
    let target = resolve_target(&cli)?;

    let provider = provider_config.build().with_context(|| {
        format!(
            "failed to build the {} provider",
            provider_config.kind.as_str()
        )
    })?;
    let provider_name = provider.name().to_string();

    let client = AgpClient::connect(&socket)
        .await
        .with_context(|| format!("cannot reach the ADesk runtime at {}", socket.display()))?;

    match target {
        RunTarget::Task(task) => {
            info!(
                provider = %provider_name,
                socket = %socket.display(),
                goal = %task.goal,
                "running task"
            );
            let mut agent = AgentLoop::new(client, BoxedProvider(provider), config);
            let outcome = agent.run(&task).await?;
            if outcome.success {
                info!(steps = outcome.steps, "task succeeded: {}", outcome.summary);
            } else {
                warn!(
                    steps = outcome.steps,
                    stop_reason = ?outcome.stop_reason,
                    "task failed: {}",
                    outcome.summary
                );
            }
            let report = RunReport {
                task: task.goal.clone(),
                provider: provider_name,
                socket: Some(socket.display().to_string()),
                metrics: outcome.metrics.clone(),
                history: outcome.history.clone(),
                scenario: None,
            };
            write_report(&cli.report, &report)?;
            Ok(outcome.success)
        }
        RunTarget::Scenario(scenario) => {
            let scenario = *scenario;
            let runner = ScenarioRunner::new(config);
            info!(
                provider = %provider_name,
                socket = %socket.display(),
                scenario = scenario.id.as_str(),
                "running scenario"
            );
            let scenario_report = if provider_config.kind == ProviderKind::Mock
                && provider_config.mock_script.is_empty()
            {
                // The scenario's replay script keeps a mock run deterministic.
                runner.run(&scenario, client).await?
            } else {
                runner
                    .run_with_provider(&scenario, client, BoxedProvider(provider))
                    .await?
            };
            for check in scenario_report.checks.iter().filter(|check| !check.passed) {
                warn!(
                    expectation = ?check.expectation,
                    detail = %check.detail,
                    "expectation failed"
                );
            }
            let passed = scenario_report.passed;
            let report = RunReport {
                task: scenario.task.goal.clone(),
                provider: provider_name,
                socket: Some(socket.display().to_string()),
                metrics: scenario_report.outcome.metrics.clone(),
                history: scenario_report.outcome.history.clone(),
                scenario: Some(scenario_report),
            };
            write_report(&cli.report, &report)?;
            Ok(passed)
        }
    }
}

/// Loop configuration derived from the CLI flags; everything else keeps the
/// documented defaults.
fn loop_config(cli: &Cli) -> LoopConfig {
    LoopConfig {
        max_steps: cli.max_steps,
        quiet_ms: cli.quiet_ms,
        capture_max_dimension: Some(cli.max_dimension),
        context_budget: ContextBudget {
            max_dimension: Some(cli.max_dimension),
            ..ContextBudget::default()
        },
        ..LoopConfig::default()
    }
}

/// Provider selection and credentials; env fallbacks are handled by clap and
/// [`ProviderConfig::build`].
fn provider_config(cli: &Cli) -> ProviderConfig {
    ProviderConfig {
        kind: cli.provider,
        model: cli.model.clone(),
        base_url: cli.base_url.clone(),
        api_key: cli.api_key.clone(),
        ..ProviderConfig::default()
    }
}

/// Resolve the AGP socket: `--socket` / `$ADESK_SOCKET` (clap), else
/// [`adesk_client::default_socket_path`]: `$ADESK_SOCKET` → `$XDG_RUNTIME_DIR/adesk.sock`
/// → `<temp dir>/adesk.sock` (the documented default of `docs/protocol.md` §1).
fn resolve_socket(cli: &Cli) -> PathBuf {
    cli.socket
        .clone()
        .unwrap_or_else(adesk_client::default_socket_path)
}

/// Resolve `--task` / `--scenario` into the run target; exactly one is required.
fn resolve_target(cli: &Cli) -> AnyResult<RunTarget> {
    match (&cli.task, cli.scenario) {
        (Some(goal), None) => Ok(RunTarget::Task(TaskDescription::new(goal.clone()))),
        (None, Some(id)) => Ok(RunTarget::Scenario(Box::new(Scenario::builtin(id)))),
        (None, None) => bail!("provide --task \"<goal>\" or --scenario <id>"),
        (Some(_), Some(_)) => bail!("--task and --scenario are mutually exclusive"),
    }
}

/// Write the report when `--report` was given; the library never touches the
/// filesystem itself.
fn write_report(path: &Option<PathBuf>, report: &RunReport) -> AnyResult<()> {
    match path {
        Some(path) => {
            report
                .write(path)
                .with_context(|| format!("writing the run report to {}", path.display()))?;
            info!(path = %path.display(), "wrote run report");
        }
        None => info!(
            success = report.metrics.success,
            stop_reason = ?report.metrics.stop_reason,
            "report not written (pass --report <path> to save it)"
        ),
    }
    Ok(())
}

/// Bridges the runtime-selected `Box<dyn LlmProvider>` to the generic provider
/// seam of [`AgentLoop`] / [`ScenarioRunner`].
struct BoxedProvider(Box<dyn LlmProvider>);

#[async_trait]
impl LlmProvider for BoxedProvider {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        self.0.complete(ctx).await
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn supports_images(&self) -> bool {
        self.0.supports_images()
    }
}
