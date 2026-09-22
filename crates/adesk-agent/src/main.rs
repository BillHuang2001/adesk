//! `adesk-agent` — multimodal GUI agent prototype (CLI).
//!
//! Wires CLI → provider → client → loop and writes the run report.
//!
//! ```text
//! adesk-agent --provider mock --scenario click
//! adesk-agent --provider dummy --dummy-mode random --dummy-seed 42 --task "Explore the desktop"
//! adesk-agent --provider openai --task "Open the settings dialog and enable dark mode" \
//!             --max-steps 30 --report runs/dark-mode.json
//! adesk-agent --watch --task "Handle the notification" --watch-max-wakeups 5
//! ```
//!
//! `--watch` selects idle/watch mode: the agent stays idle until a notification
//! wakes it, runs the standing `--task` job, then returns to idle (see the
//! `adesk_agent::watch` module).
//!
//! Exit codes: `0` task succeeded and all expectations passed, `1` the task or a
//! scenario expectation failed, `2` configuration/startup error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context as _, Result as AnyResult};
use clap::Parser;
use tracing::{error, info, warn};

use adesk_agent::{
    AgentLoop, AgpClient, ContextBudget, DummyMode, LlmProvider, LoopConfig, ProviderConfig,
    ProviderKind, RunReport, Scenario, ScenarioId, ScenarioRunner, TaskDescription, WatchConfig,
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

    /// Idle/watch mode: stay idle and handle the standing `--task` job each time
    /// a notification wakes the agent (mutually exclusive with `--scenario`).
    #[arg(long, env = "ADESK_AGENT_WATCH", conflicts_with = "scenario")]
    watch: bool,

    /// Bound on a single idle wait in watch mode (ms).
    #[arg(long, env = "ADESK_AGENT_WATCH_TIMEOUT_MS", default_value_t = 30_000)]
    watch_timeout_ms: u64,

    /// Stop watch mode after this many handled wakeups (`0` = unbounded).
    #[arg(long, env = "ADESK_AGENT_WATCH_MAX_WAKEUPS", default_value_t = 0)]
    watch_max_wakeups: u32,

    /// Stop watch mode after this many consecutive idle waits (`0` = unbounded).
    #[arg(long, env = "ADESK_AGENT_WATCH_MAX_IDLE_WAITS", default_value_t = 0)]
    watch_max_idle_waits: u32,
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

    /// Opt in to the text-first accessibility capability: the agent may read a
    /// window's UI as a text outline (`accessibility_tree`) instead of a screenshot.
    #[arg(long, env = "ADESK_AGENT_INCLUDE_ACCESSIBILITY")]
    include_accessibility: bool,
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
            if cli.watch {
                return run_watch_mode(
                    &cli,
                    client,
                    provider,
                    &provider_name,
                    &socket,
                    config,
                    task,
                )
                .await;
            }
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
        include_accessibility: cli.include_accessibility,
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

/// Resolve `--task` / `--scenario` into the run target; exactly one is required
/// (and watch mode requires `--task`).
fn resolve_target(cli: &Cli) -> AnyResult<RunTarget> {
    if cli.watch {
        return match &cli.task {
            Some(goal) => Ok(RunTarget::Task(TaskDescription::new(goal.clone()))),
            None => bail!("--watch requires --task \"<goal>\""),
        };
    }
    match (&cli.task, cli.scenario) {
        (Some(goal), None) => Ok(RunTarget::Task(TaskDescription::new(goal.clone()))),
        (None, Some(id)) => Ok(RunTarget::Scenario(Box::new(Scenario::builtin(id)))),
        (None, None) => bail!("provide --task \"<goal>\" or --scenario <id>"),
        (Some(_), Some(_)) => bail!("--task and --scenario are mutually exclusive"),
    }
}

/// Watch configuration derived from the CLI flags; the wake filter and
/// `max_events` keep their [`WatchConfig::default`] values, and a zero budget
/// flag means unbounded (`None`).
fn watch_config(cli: &Cli) -> WatchConfig {
    WatchConfig {
        wait_timeout_ms: cli.watch_timeout_ms,
        max_wakeups: budget(cli.watch_max_wakeups),
        max_idle_waits: budget(cli.watch_max_idle_waits),
        ..WatchConfig::default()
    }
}

/// `0` means unbounded; any other value is a finite budget.
fn budget(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

/// Idle/watch mode: drive [`AgentLoop::run_watch`], log every wakeup and idle
/// wait, and write the last handled wakeup's report when `--report` was given.
///
/// Returns whether every handled wakeup's job succeeded (vacuously true when no
/// wakeup was handled).
async fn run_watch_mode(
    cli: &Cli,
    client: AgpClient,
    provider: Box<dyn LlmProvider>,
    provider_name: &str,
    socket: &Path,
    config: LoopConfig,
    task: TaskDescription,
) -> AnyResult<bool> {
    let watch = watch_config(cli);
    info!(
        provider = %provider_name,
        socket = %socket.display(),
        goal = %task.goal,
        timeout_ms = watch.wait_timeout_ms,
        "watching for wake events"
    );

    let mut agent = AgentLoop::new(client, provider, config);
    let outcome = agent.run_watch(&task, &watch).await?;

    for (index, wakeup) in outcome.wakeups.iter().enumerate() {
        if wakeup.outcome.success {
            info!(
                wakeup = index,
                events = wakeup.events.len(),
                steps = wakeup.outcome.steps,
                "handled wakeup"
            );
        } else {
            warn!(
                wakeup = index,
                events = wakeup.events.len(),
                steps = wakeup.outcome.steps,
                stop_reason = ?wakeup.outcome.stop_reason,
                "wakeup job did not succeed: {}",
                wakeup.outcome.summary
            );
        }
    }
    info!(
        idle_waits = outcome.idle_waits,
        stop_reason = ?outcome.stop_reason,
        "watch stopped"
    );

    match outcome.wakeups.last() {
        Some(last) => {
            let report = RunReport {
                task: task.goal.clone(),
                provider: provider_name.to_owned(),
                socket: Some(socket.display().to_string()),
                metrics: last.outcome.metrics.clone(),
                history: last.outcome.history.clone(),
                scenario: None,
            };
            write_report(&cli.report, &report)?;
        }
        None if cli.report.is_some() => info!("no wakeup was handled; no report written"),
        None => {}
    }

    Ok(outcome.wakeups.iter().all(|wakeup| wakeup.outcome.success))
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

    /// `--watch` and its bounded flags parse; a bare `--task` run is not in watch
    /// mode; `0` maps to an unbounded budget; watch mode requires `--task` and
    /// conflicts with `--scenario`; and the `ADESK_AGENT_WATCH*` env vars are the
    /// fallback. Env mutation lives in this single test so no two tests race over
    /// the same process-wide variables.
    #[test]
    fn cli_watch_flags_parse_and_env_fallbacks_apply() {
        // Hermetic: any ambient watch env vars must not leak into the assertions.
        for var in [
            "ADESK_AGENT_WATCH",
            "ADESK_AGENT_WATCH_TIMEOUT_MS",
            "ADESK_AGENT_WATCH_MAX_WAKEUPS",
            "ADESK_AGENT_WATCH_MAX_IDLE_WAITS",
        ] {
            std::env::remove_var(var);
        }

        let cli = Cli::try_parse_from([
            "adesk-agent",
            "--watch",
            "--task",
            "handle the notification",
            "--watch-timeout-ms",
            "5000",
            "--watch-max-wakeups",
            "3",
        ])
        .expect("the watch invocation parses");
        assert!(cli.watch);
        assert_eq!(cli.watch_timeout_ms, 5000);
        assert_eq!(cli.watch_max_wakeups, 3);
        assert_eq!(cli.watch_max_idle_waits, 0, "unbounded by default");

        let watch = watch_config(&cli);
        assert_eq!(watch.wait_timeout_ms, 5000);
        assert_eq!(watch.max_wakeups, Some(3));
        assert_eq!(watch.max_idle_waits, None);
        assert_eq!(watch.wake_kinds, WatchConfig::default().wake_kinds);
        assert_eq!(watch.max_events, WatchConfig::default().max_events);

        // A bare `--task` run is not in watch mode and keeps the unbounded budget.
        let cli = Cli::try_parse_from(["adesk-agent", "--task", "explore"]).expect("parses");
        assert!(!cli.watch);
        assert_eq!(watch_config(&cli).max_wakeups, None);

        // Watch mode requires `--task`.
        let cli = Cli::try_parse_from(["adesk-agent", "--watch"]).expect("the flag parses");
        assert!(resolve_target(&cli).is_err());

        // `--watch` conflicts with `--scenario`.
        assert!(Cli::try_parse_from([
            "adesk-agent",
            "--watch",
            "--task",
            "x",
            "--scenario",
            "click",
        ])
        .is_err());

        // The env fallback selects watch mode and the idle budget.
        std::env::set_var("ADESK_AGENT_WATCH", "true");
        std::env::set_var("ADESK_AGENT_WATCH_MAX_IDLE_WAITS", "4");
        let cli = Cli::try_parse_from(["adesk-agent", "--task", "explore"])
            .expect("the env-only invocation parses");
        for var in ["ADESK_AGENT_WATCH", "ADESK_AGENT_WATCH_MAX_IDLE_WAITS"] {
            std::env::remove_var(var);
        }
        assert!(cli.watch, "ADESK_AGENT_WATCH selects watch mode");
        assert_eq!(watch_config(&cli).max_idle_waits, Some(4));
    }
}
