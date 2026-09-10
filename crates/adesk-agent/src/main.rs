//! `adesk-agent` — multimodal GUI agent prototype (CLI).
//!
//! Wires CLI → provider → client → loop and writes the run report.
//!
//! ```text
//! adesk-agent --provider mock --scenario click
//! adesk-agent --provider dummy --dummy-mode random --dummy-seed 42 --task "Explore the desktop"
//! adesk-agent --provider openai --task "Open the settings dialog and enable dark mode" \
//!             --max-steps 30 --report runs/dark-mode.json
//! ```
//!
//! Exit codes: `0` task succeeded and all expectations passed, `1` the task or a
//! scenario expectation failed, `2` configuration/startup error.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context as _, Result as AnyResult};
use clap::Parser;
use tracing::{error, info, warn};

use adesk_agent::{
    AgentLoop, AgpClient, ContextBudget, DummyMode, LlmProvider, LoopConfig, ProviderConfig,
    ProviderKind, RunReport, Scenario, ScenarioId, ScenarioRunner, TaskDescription,
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

    /// Dummy provider sampling mode: `fixed` replays a canned script, `random`
    /// draws from an action pool (`--provider dummy`).
    #[arg(long, value_enum, env = "ADESK_AGENT_DUMMY_MODE")]
    dummy_mode: Option<DummyMode>,

    /// PRNG seed for the dummy provider's random mode (default `0x5EED_5EED`).
    #[arg(long, env = "ADESK_AGENT_DUMMY_SEED")]
    dummy_seed: Option<u64>,

    /// Per-step finish probability for the dummy provider's random mode (default `0.15`).
    #[arg(long, env = "ADESK_AGENT_DUMMY_FINISH_PROBABILITY")]
    dummy_finish_probability: Option<f64>,

    /// Hard step budget for the dummy provider's random mode (default `10`).
    #[arg(long, env = "ADESK_AGENT_DUMMY_STEP_BUDGET")]
    dummy_step_budget: Option<u32>,

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
            let mut agent = AgentLoop::new(client, provider, config);
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
                    .run_with_provider(&scenario, client, provider)
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
/// [`ProviderConfig::build`]. Dummy fields override [`ProviderConfig::default`]
/// only when the user (or its `ADESK_AGENT_DUMMY_*` env fallback) set them, so the
/// documented dummy defaults are preserved otherwise.
fn provider_config(cli: &Cli) -> ProviderConfig {
    let mut config = ProviderConfig {
        kind: cli.provider,
        model: cli.model.clone(),
        base_url: cli.base_url.clone(),
        api_key: cli.api_key.clone(),
        ..ProviderConfig::default()
    };
    if let Some(mode) = cli.dummy_mode {
        config.dummy_mode = mode;
    }
    if let Some(seed) = cli.dummy_seed {
        config.dummy_seed = seed;
    }
    if let Some(probability) = cli.dummy_finish_probability {
        config.dummy_finish_probability = probability;
    }
    if let Some(budget) = cli.dummy_step_budget {
        config.dummy_step_budget = budget;
    }
    config
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `--provider dummy --dummy-mode fixed --dummy-seed 1` parses to the expected
    /// fields.
    #[test]
    fn cli_parses_the_dummy_flags() {
        let cli = Cli::try_parse_from([
            "adesk-agent",
            "--provider",
            "dummy",
            "--dummy-mode",
            "fixed",
            "--dummy-seed",
            "1",
            "--dummy-finish-probability",
            "0.25",
            "--dummy-step-budget",
            "3",
            "--task",
            "explore",
        ])
        .expect("the fixed-mode dummy invocation parses");

        assert_eq!(cli.provider, ProviderKind::Dummy);
        assert_eq!(cli.dummy_mode, Some(DummyMode::Fixed));
        assert_eq!(cli.dummy_seed, Some(1));
        assert_eq!(cli.dummy_finish_probability, Some(0.25));
        assert_eq!(cli.dummy_step_budget, Some(3));
    }

    /// Unset dummy flags stay `None`, so [`provider_config`] keeps the documented
    /// defaults, and the `ADESK_AGENT_DUMMY_*` env vars are the fallback. The env
    /// assertions live in this test (rather than a second, parallel one) so no two
    /// tests race over the same process-wide variables.
    #[test]
    fn cli_dummy_flags_default_to_none_and_env_fallbacks_apply() {
        // Hermetic: any ambient dummy env vars must not leak into the assertions.
        for var in [
            "ADESK_AGENT_DUMMY_MODE",
            "ADESK_AGENT_DUMMY_SEED",
            "ADESK_AGENT_DUMMY_FINISH_PROBABILITY",
            "ADESK_AGENT_DUMMY_STEP_BUDGET",
        ] {
            std::env::remove_var(var);
        }

        let cli = Cli::try_parse_from(["adesk-agent", "--task", "explore"])
            .expect("a bare invocation parses");
        assert_eq!(cli.dummy_mode, None);
        assert_eq!(cli.dummy_seed, None);
        assert_eq!(cli.dummy_finish_probability, None);
        assert_eq!(cli.dummy_step_budget, None);

        let default = provider_config(&cli);
        assert_eq!(default.dummy_mode, DummyMode::Random);
        assert_eq!(
            default.dummy_seed,
            adesk_agent::provider::dummy::DEFAULT_SEED
        );
        assert_eq!(
            default.dummy_finish_probability,
            adesk_agent::provider::dummy::DEFAULT_FINISH_PROBABILITY
        );
        assert_eq!(
            default.dummy_step_budget,
            adesk_agent::provider::dummy::DEFAULT_STEP_BUDGET
        );

        std::env::set_var("ADESK_AGENT_DUMMY_MODE", "random");
        std::env::set_var("ADESK_AGENT_DUMMY_SEED", "7");
        std::env::set_var("ADESK_AGENT_DUMMY_FINISH_PROBABILITY", "0.5");
        std::env::set_var("ADESK_AGENT_DUMMY_STEP_BUDGET", "9");
        let cli = Cli::try_parse_from(["adesk-agent", "--task", "explore"])
            .expect("the env-only invocation parses");
        for var in [
            "ADESK_AGENT_DUMMY_MODE",
            "ADESK_AGENT_DUMMY_SEED",
            "ADESK_AGENT_DUMMY_FINISH_PROBABILITY",
            "ADESK_AGENT_DUMMY_STEP_BUDGET",
        ] {
            std::env::remove_var(var);
        }

        assert_eq!(cli.dummy_mode, Some(DummyMode::Random));
        assert_eq!(cli.dummy_seed, Some(7));
        assert_eq!(cli.dummy_finish_probability, Some(0.5));
        assert_eq!(cli.dummy_step_budget, Some(9));

        let from_env = provider_config(&cli);
        assert_eq!(from_env.dummy_mode, DummyMode::Random);
        assert_eq!(from_env.dummy_seed, 7);
        assert_eq!(from_env.dummy_finish_probability, 0.5);
        assert_eq!(from_env.dummy_step_budget, 9);
    }
}
