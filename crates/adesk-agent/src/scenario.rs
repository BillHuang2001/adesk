//! Built-in task scenarios and their runner.
//!
//! A [`Scenario`] bundles a [`TaskDescription`], a replay script for
//! [`crate::provider::MockProvider`], expectations checked against the resulting
//! [`LoopOutcome`], and a step budget. Built-in scenarios:
//!
//! | Scenario | Exercises |
//! |---|---|
//! | `launch` | app discovery → `launch_app` → wait for the window |
//! | `activate` | multi-window focus via `activate_window` (never synthetic input) |
//! | `click` | normalized-coordinate click + quiet observation |
//! | `type` | `type_text` into the focused window + quiet observation |
//! | `scroll` | pointer axis scrolling + change observation |
//! | `dialog` | popup/dialog handling (`popup_appeared` → dismiss → verify) |
//! | `navigation` | cross-page navigation (click link → title change → verify) |
//! | `error_recovery` | injected `unknown_window` → refresh → continue |
//!
//! The runner drives a real [`AgentClient`] with either the scenario script
//! (mock provider, deterministic) or a live provider (`run_with_provider`).

use serde::{Deserialize, Serialize};

use crate::agent_loop::{AgentLoop, LoopConfig, LoopOutcome};
use crate::client::AgentClient;
use crate::context::TaskDescription;
use crate::decision::ActionKind;
use crate::provider::{LlmProvider, MockProvider, ScriptEntry};
use crate::Result;

/// Identifier of a built-in scenario (`--scenario <id>`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioId {
    /// Launch an application and wait for its window.
    Launch,
    /// Activate a window among several.
    Activate,
    /// Click at a normalized position and observe quiet.
    Click,
    /// Type text and observe quiet.
    Type,
    /// Scroll and observe the change.
    Scroll,
    /// Detect and dismiss a dialog/popup.
    Dialog,
    /// Cross-page navigation with title verification.
    Navigation,
    /// Recover from an injected runtime error.
    ErrorRecovery,
}

impl ScenarioId {
    /// Stable name, matching the `--scenario` CLI values.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Activate => "activate",
            Self::Click => "click",
            Self::Type => "type",
            Self::Scroll => "scroll",
            Self::Dialog => "dialog",
            Self::Navigation => "navigation",
            Self::ErrorRecovery => "error-recovery",
        }
    }

    /// Every built-in scenario, in the order they are exercised by tests.
    pub fn all() -> &'static [ScenarioId] {
        &[
            ScenarioId::Launch,
            ScenarioId::Activate,
            ScenarioId::Click,
            ScenarioId::Type,
            ScenarioId::Scroll,
            ScenarioId::Dialog,
            ScenarioId::Navigation,
            ScenarioId::ErrorRecovery,
        ]
    }
}

/// One built-in scenario definition.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Identifier.
    pub id: ScenarioId,
    /// Human-readable name.
    pub name: &'static str,
    /// What the scenario exercises.
    pub description: &'static str,
    /// Task handed to the loop.
    pub task: TaskDescription,
    /// Replay script for the mock provider.
    pub script: Vec<ScriptEntry>,
    /// Expectations checked after the run.
    pub expectations: Vec<Expectation>,
    /// Step budget for the run.
    pub max_steps: u32,
}

impl Scenario {
    /// Fetch a built-in scenario by id.
    pub fn builtin(id: ScenarioId) -> Scenario {
        let _ = id;
        todo!("phase 2: define the eight built-in scenarios")
    }
}

/// One post-run expectation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expectation {
    /// The loop stopped with `Finished` and the given success flag.
    Finished {
        /// Expected `LoopOutcome::success`.
        success: bool,
    },
    /// The run used at most this many steps.
    MaxSteps(u32),
    /// The run performed at most this many GPU readbacks.
    MaxReadbacks(u32),
    /// The run sent at most this many estimated visual tokens.
    MaxVisualTokens(u64),
    /// No step ended in [`crate::agent_loop::StepStatus::Failed`].
    NoFailedSteps,
    /// At least one action of this kind was executed.
    ActionSeen(ActionKind),
    /// At least `count` actions of `kind` were executed.
    MinActions {
        /// Action kind.
        kind: ActionKind,
        /// Minimum count.
        count: u32,
    },
}

/// Result of checking one [`Expectation`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectationResult {
    /// The expectation that was checked.
    pub expectation: Expectation,
    /// Whether it held.
    pub passed: bool,
    /// Observed value / explanation.
    pub detail: String,
}

/// Outcome of running one scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioReport {
    /// Scenario identifier.
    pub id: ScenarioId,
    /// Scenario name.
    pub name: String,
    /// True when every expectation passed.
    pub passed: bool,
    /// Per-expectation results.
    pub checks: Vec<ExpectationResult>,
    /// The underlying loop outcome.
    pub outcome: LoopOutcome,
}

/// Runs scenarios against an [`AgentClient`] with a fixed [`LoopConfig`].
#[derive(Debug, Clone)]
pub struct ScenarioRunner {
    config: LoopConfig,
}

impl ScenarioRunner {
    /// Build a runner with the given loop configuration.
    pub fn new(config: LoopConfig) -> Self {
        Self { config }
    }

    /// The loop configuration used for every run.
    pub fn config(&self) -> &LoopConfig {
        &self.config
    }

    /// Run `scenario` with its own replay script (deterministic mock provider).
    pub async fn run<C: AgentClient>(
        &self,
        scenario: &Scenario,
        client: C,
    ) -> Result<ScenarioReport> {
        let provider = MockProvider::new(scenario.script.clone());
        self.run_with_provider(scenario, client, provider).await
    }

    /// Run `scenario` with a caller-supplied provider (e.g. a live LLM).
    pub async fn run_with_provider<C: AgentClient, P: LlmProvider>(
        &self,
        scenario: &Scenario,
        client: C,
        provider: P,
    ) -> Result<ScenarioReport> {
        let _ = (scenario, client, provider);
        todo!("phase 2: run AgentLoop with scenario.max_steps, then evaluate()")
    }

    /// Check `scenario`'s expectations against a finished run.
    pub fn evaluate(scenario: &Scenario, outcome: &LoopOutcome) -> Vec<ExpectationResult> {
        let _ = (scenario, outcome);
        todo!("phase 2: check each Expectation against the outcome")
    }
}

/// The step budget a scenario runner should apply for `scenario`.
pub fn effective_max_steps(runner: &ScenarioRunner, scenario: &Scenario) -> u32 {
    runner.config.max_steps.min(scenario.max_steps)
}
