//! What one loop iteration produces: the step record, the run outcome, and the
//! loop's per-step bookkeeping.

use adesk_core::{ActionId, AppInfo, Observation, WindowId, WindowInfo};
use adesk_proto::ImagePayload;
use serde::{Deserialize, Serialize};

use crate::decision::AgentDecision;
use crate::metrics::{MetricsReport, StopReason};

/// Outcome of one loop iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Decision executed and observed without error.
    Ok,
    /// The step failed at least once but ultimately succeeded.
    Recovered,
    /// The step failed; the loop applied its recovery policy.
    Failed,
    /// The decision was `Finish`.
    Finished,
}

/// Full record of one loop iteration, kept for the run report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    /// 0-based step index.
    pub step: u32,
    /// Decision the provider produced.
    pub decision: AgentDecision,
    /// Action id returned by the runtime, when the decision produced one.
    #[serde(default)]
    pub action_id: Option<ActionId>,
    /// Observation produced by the step, when any.
    #[serde(default)]
    pub observation: Option<Observation>,
    /// Step outcome.
    pub status: StepStatus,
    /// Error message, when the step failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Provider decision latency.
    pub decision_latency_ms: u64,
    /// Total step wall time.
    pub elapsed_ms: u64,
    /// Whether the step read pixels back from the runtime.
    pub readback: bool,
}

/// Result of a whole task run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopOutcome {
    /// Whether the task finished successfully (`Finish { success: true }`).
    pub success: bool,
    /// Summary from `Finish`, or the stop reason when the budget won.
    pub summary: String,
    /// Steps executed.
    pub steps: u32,
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// Aggregated metrics.
    pub metrics: MetricsReport,
    /// Per-step history.
    pub history: Vec<StepRecord>,
}

/// Live runtime facts cached between steps; nothing here grows with step count.
#[derive(Debug, Default)]
pub(super) struct RuntimeFacts {
    pub(super) windows: Vec<WindowInfo>,
    pub(super) active_window: Option<WindowId>,
    pub(super) apps: Vec<AppInfo>,
    pub(super) observation: Option<Observation>,
    /// Latest accessibility outline, when the backend produced one.
    pub(super) accessibility: Option<String>,
}

/// What one executed decision produced.
#[derive(Debug, Default)]
pub(super) struct Execution {
    /// Action id returned by an input or state-mutating call.
    pub(super) action_id: Option<ActionId>,
    /// Observation returned by the step, when any.
    pub(super) observation: Option<Observation>,
    /// Image returned by the step, when any (a readback).
    pub(super) image: Option<ImagePayload>,
    /// Failed attempts before the step succeeded (or the total, on failure).
    pub(super) attempts: u32,
}
