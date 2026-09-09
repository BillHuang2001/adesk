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

use adesk_core::{AppId, Button, Position, WindowId};
use serde::{Deserialize, Serialize};

use crate::agent_loop::{AgentLoop, LoopConfig, LoopOutcome, StepStatus};
use crate::client::AgentClient;
use crate::context::TaskDescription;
use crate::decision::{ActionKind, AgentDecision, ObserveCondition};
use crate::metrics::{MetricsReport, StopReason};
use crate::provider::{LlmProvider, MockProvider, ScriptEntry};
use crate::Result;

/// Identifier of a built-in scenario (`--scenario <id>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
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
        match id {
            ScenarioId::Launch => launch_scenario(),
            ScenarioId::Activate => activate_scenario(),
            ScenarioId::Click => click_scenario(),
            ScenarioId::Type => type_scenario(),
            ScenarioId::Scroll => scroll_scenario(),
            ScenarioId::Dialog => dialog_scenario(),
            ScenarioId::Navigation => navigation_scenario(),
            ScenarioId::ErrorRecovery => error_recovery_scenario(),
        }
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
    ///
    /// The loop gets `max_steps = min(runner.max_steps, scenario.max_steps)`;
    /// failed expectations are reported in the returned [`ScenarioReport`], never
    /// turned into an `Err` — only fatal loop errors (protocol mismatch, provider
    /// or configuration failures) propagate.
    pub async fn run_with_provider<C: AgentClient, P: LlmProvider>(
        &self,
        scenario: &Scenario,
        client: C,
        provider: P,
    ) -> Result<ScenarioReport> {
        let config = LoopConfig {
            max_steps: effective_max_steps(self, scenario),
            ..self.config.clone()
        };
        let mut agent = AgentLoop::new(client, provider, config);
        let outcome = agent.run(&scenario.task).await?;
        let checks = Self::evaluate(scenario, &outcome);
        let passed = checks.iter().all(|check| check.passed);
        Ok(ScenarioReport {
            id: scenario.id,
            name: scenario.name.to_owned(),
            passed,
            checks,
            outcome,
        })
    }

    /// Check `scenario`'s expectations against a finished run.
    ///
    /// Every result carries the observed value in its `detail`, so a failing
    /// report explains itself without re-running anything.
    pub fn evaluate(scenario: &Scenario, outcome: &LoopOutcome) -> Vec<ExpectationResult> {
        scenario
            .expectations
            .iter()
            .map(|expectation| check_expectation(expectation, outcome))
            .collect()
    }
}

/// Check one expectation against a finished run.
fn check_expectation(expectation: &Expectation, outcome: &LoopOutcome) -> ExpectationResult {
    let metrics = &outcome.metrics;
    let (passed, detail) = match expectation {
        Expectation::Finished { success } => (
            outcome.stop_reason == StopReason::Finished && outcome.success == *success,
            format!(
                "stop_reason={:?} success={} (want finished with success={success})",
                outcome.stop_reason, outcome.success
            ),
        ),
        Expectation::MaxSteps(max) => (
            outcome.steps <= *max,
            format!("steps={} (max {max})", outcome.steps),
        ),
        Expectation::MaxReadbacks(max) => (
            metrics.gpu_readbacks <= *max,
            format!("gpu_readbacks={} (max {max})", metrics.gpu_readbacks),
        ),
        Expectation::MaxVisualTokens(max) => (
            metrics.visual_tokens <= *max,
            format!("visual_tokens={} (max {max})", metrics.visual_tokens),
        ),
        Expectation::NoFailedSteps => {
            let failed = outcome
                .history
                .iter()
                .filter(|record| record.status == StepStatus::Failed)
                .count();
            (
                failed == 0,
                format!("failed_steps={failed} of {} (max 0)", outcome.history.len()),
            )
        }
        Expectation::ActionSeen(kind) => {
            let seen = actions_of_kind(metrics, *kind);
            (seen >= 1, format!("{}={seen} (min 1)", kind.as_str()))
        }
        Expectation::MinActions { kind, count } => {
            let seen = actions_of_kind(metrics, *kind);
            (
                seen >= *count,
                format!("{}={seen} (min {count})", kind.as_str()),
            )
        }
    };
    ExpectationResult {
        expectation: expectation.clone(),
        passed,
        detail,
    }
}

/// Executed actions of one kind, as counted by the run metrics.
fn actions_of_kind(metrics: &MetricsReport, kind: ActionKind) -> u32 {
    metrics.actions_by_kind.get(&kind).copied().unwrap_or(0)
}

/// The step budget a scenario runner should apply for `scenario`.
pub fn effective_max_steps(runner: &ScenarioRunner, scenario: &Scenario) -> u32 {
    runner.config.max_steps.min(scenario.max_steps)
}

/// One scripted decision for [`MockProvider`].
fn scripted(decision: AgentDecision) -> ScriptEntry {
    ScriptEntry::decision(decision)
}

/// A successful `Finish` script entry.
fn finished(summary: &str) -> ScriptEntry {
    ScriptEntry::decision(AgentDecision::Finish {
        success: true,
        summary: summary.to_owned(),
    })
}

/// An explicit observation that requests no pixels (never a GPU readback).
fn observe_without_image(
    window_id: Option<WindowId>,
    until: ObserveCondition,
    timeout_ms: u64,
) -> AgentDecision {
    AgentDecision::Observe {
        window_id,
        after_action: None,
        until,
        timeout_ms: Some(timeout_ms),
        include_image: Some(false),
        max_dimension: None,
        region: None,
    }
}

/// A single left click at a window-relative position.
fn click_at(window_id: WindowId, position: Position) -> AgentDecision {
    AgentDecision::Click {
        window_id,
        position,
        button: Button::Left,
        count: 1,
    }
}

/// `launch`: app discovery → `launch_app` → wait for the window (no pixels).
fn launch_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Launch,
        name: "launch",
        description: "Discover an application, launch it, and wait for its window to settle.",
        task: TaskDescription {
            goal: String::from("Launch the file manager and wait until its window is ready"),
            success_criteria: Some(String::from(
                "a window of org.example.files is mapped and has gone quiet",
            )),
            hints: vec![String::from(
                "use launch_app, then wait for the window instead of polling for it",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListApps {
                query: Some(String::from("files")),
            }),
            scripted(AgentDecision::LaunchApp {
                app_id: AppId::from("org.example.files"),
                args: Vec::new(),
            }),
            scripted(AgentDecision::Wait {
                window_id: None,
                until: ObserveCondition::Quiet { quiet_ms: 250 },
                timeout_ms: Some(5_000),
            }),
            finished("launched the file manager and observed its window settle"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::LaunchApp),
            Expectation::MaxReadbacks(0),
            Expectation::MaxSteps(6),
            Expectation::NoFailedSteps,
        ],
        max_steps: 6,
    }
}

/// `activate`: two windows, focus the inactive one with `activate_window`.
fn activate_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Activate,
        name: "activate",
        description: "Focus a background window with activate_window, never synthetic input.",
        task: TaskDescription {
            goal: String::from("Bring the text editor window to the front"),
            success_criteria: Some(String::from("window 2 is the active window")),
            hints: vec![String::from(
                "activate_window mutates compositor state; never synthesize Alt+Tab",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListWindows),
            scripted(AgentDecision::ActivateWindow {
                window_id: WindowId(2),
            }),
            scripted(observe_without_image(
                Some(WindowId(2)),
                ObserveCondition::Change,
                2_000,
            )),
            finished("editor window activated by the runtime, no synthetic input"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::ActivateWindow),
            Expectation::MaxReadbacks(0),
            Expectation::MaxSteps(6),
            Expectation::NoFailedSteps,
        ],
        max_steps: 6,
    }
}

/// `click`: normalized-position click, then quiet observation without pixels.
fn click_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Click,
        name: "click",
        description:
            "Click a toolbar button at a normalized position and observe the window settle.",
        task: TaskDescription {
            goal: String::from("Click the Save button in the toolbar"),
            success_criteria: Some(String::from(
                "the document is saved and the window stops changing",
            )),
            hints: vec![String::from(
                "positions are window-relative; normalized 0..1 avoids pixel math",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListWindows),
            scripted(click_at(WindowId(1), Position::normalized(0.5, 0.05))),
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Quiet { quiet_ms: 250 },
                5_000,
            )),
            finished("clicked Save and the window went quiet"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::Click),
            Expectation::ActionSeen(ActionKind::Observe),
            Expectation::MaxReadbacks(1),
            Expectation::MaxVisualTokens(1_105),
            Expectation::MaxSteps(8),
            Expectation::NoFailedSteps,
        ],
        max_steps: 8,
    }
}

/// `type`: `type_text` into the focused window, then quiet observation.
fn type_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Type,
        name: "type",
        description: "Type a filename into the focused window and observe the result settle.",
        task: TaskDescription {
            goal: String::from("Type the filename report.txt into the save dialog"),
            success_criteria: Some(String::from("the filename field contains report.txt")),
            hints: vec![String::from(
                "type_text goes through the seat keymap; no keypress chords are needed",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListWindows),
            scripted(AgentDecision::Type {
                window_id: Some(WindowId(1)),
                text: String::from("report.txt"),
            }),
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Quiet { quiet_ms: 250 },
                5_000,
            )),
            finished("typed report.txt into the save dialog"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::TypeText),
            Expectation::ActionSeen(ActionKind::Observe),
            Expectation::MaxReadbacks(1),
            Expectation::MaxSteps(8),
            Expectation::NoFailedSteps,
        ],
        max_steps: 8,
    }
}

/// `scroll`: pointer axis scroll, then a change observation.
fn scroll_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Scroll,
        name: "scroll",
        description: "Scroll the window with pointer axis events and observe the resulting change.",
        task: TaskDescription {
            goal: String::from("Scroll the document down to the next section"),
            success_criteria: Some(String::from("new content is visible in the window")),
            hints: vec![String::from(
                "scroll takes a window-relative position and axis deltas",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListWindows),
            scripted(AgentDecision::Scroll {
                window_id: WindowId(1),
                position: Position::normalized(0.5, 0.5),
                dx: 0.0,
                dy: -3.0,
            }),
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Change,
                2_000,
            )),
            finished("scrolled the document and observed the change"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::Scroll),
            Expectation::ActionSeen(ActionKind::Observe),
            Expectation::MaxReadbacks(1),
            Expectation::MaxSteps(8),
            Expectation::NoFailedSteps,
        ],
        max_steps: 8,
    }
}

/// `dialog`: observe `popup_appeared`, dismiss it, verify the disappearance.
fn dialog_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Dialog,
        name: "dialog",
        description: "Notice a popup, dismiss it, and verify that it disappeared.",
        task: TaskDescription {
            goal: String::from("Dismiss the confirmation dialog that appeared"),
            success_criteria: Some(String::from("no popup remains mapped on the window")),
            hints: vec![String::from(
                "observations report popups_appeared / popups_disappeared, not just damage",
            )],
        },
        script: vec![
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Change,
                2_000,
            )),
            scripted(click_at(WindowId(1), Position::normalized(0.5, 0.75))),
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Quiet { quiet_ms: 250 },
                5_000,
            )),
            finished("dismissed the confirmation dialog and verified it is gone"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::Click),
            Expectation::MinActions {
                kind: ActionKind::Observe,
                count: 2,
            },
            Expectation::MaxReadbacks(1),
            Expectation::MaxSteps(8),
            Expectation::NoFailedSteps,
        ],
        max_steps: 8,
    }
}

/// `navigation`: click a link, observe the change, verify the title changed.
fn navigation_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::Navigation,
        name: "navigation",
        description: "Click a link, observe the change, and verify the title changed.",
        task: TaskDescription {
            goal: String::from("Open the second page by clicking its link"),
            success_criteria: Some(String::from("the window title changed to the new page")),
            hints: vec![String::from(
                "a title_changed observation is the evidence that navigation completed",
            )],
        },
        script: vec![
            scripted(AgentDecision::ListWindows),
            scripted(click_at(WindowId(1), Position::normalized(0.25, 0.4))),
            scripted(observe_without_image(
                Some(WindowId(1)),
                ObserveCondition::Quiet { quiet_ms: 250 },
                5_000,
            )),
            finished("navigated to page two; the title changed"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::ActionSeen(ActionKind::Click),
            Expectation::ActionSeen(ActionKind::Observe),
            Expectation::MaxReadbacks(1),
            Expectation::MaxSteps(8),
            Expectation::NoFailedSteps,
        ],
        max_steps: 8,
    }
}

/// `error_recovery`: a stale window id, one refresh, then continue to completion.
fn error_recovery_scenario() -> Scenario {
    Scenario {
        id: ScenarioId::ErrorRecovery,
        name: "error-recovery",
        description: "Hit a stale window id, refresh the window list, and continue to completion.",
        task: TaskDescription {
            goal: String::from("Click the Continue button in the wizard"),
            success_criteria: Some(String::from("the wizard advanced to the next step")),
            hints: vec![String::from(
                "a recoverable runtime error refreshes the window list; re-read it before retrying",
            )],
        },
        script: vec![
            scripted(click_at(WindowId(9), Position::normalized(0.9, 0.9))),
            scripted(click_at(WindowId(1), Position::normalized(0.9, 0.9))),
            finished("recovered from a stale window id and clicked Continue"),
        ],
        expectations: vec![
            Expectation::Finished { success: true },
            Expectation::MinActions {
                kind: ActionKind::Click,
                count: 1,
            },
            Expectation::MaxReadbacks(1),
            Expectation::MaxSteps(8),
        ],
        max_steps: 8,
    }
}
