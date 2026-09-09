//! The agent control loop: plan → act → observe → decide.
//!
//! One iteration:
//!
//! 1. **plan** — [`ContextBuilder`] assembles a bounded [`AgentContext`] from the
//!    latest runtime facts, then [`LlmProvider::complete`] returns one
//!    [`AgentDecision`].
//! 2. **act** — the decision is executed through [`AgentClient`]. Input decisions
//!    (click/type/keypress/scroll) return an `action_id`, remembered as
//!    `last_action_id`; runtime-native decisions execute directly.
//! 3. **observe** — after seat input the loop observes `quiet` (configurable)
//!    with `after_action = last_action_id`, so the observation describes causal
//!    history. Runtime observations are recorded as returned.
//! 4. **decide** — the step is recorded, metrics are updated, and the loop checks
//!    the step budget, the consecutive-failure budget and completion.
//!
//! ## Budgets, timeouts, recovery
//!
//! - **Step budget**: at most [`LoopConfig::max_steps`] iterations; exhaustion
//!   stops with [`StopReason::StepBudgetExhausted`].
//! - **Timeouts**: every provider call and every client call is bounded by
//!   [`LoopConfig::step_timeout_ms`]; a timeout is [`Error::StepTimeout`].
//! - **Recovery**: [`Error::class`] drives the reaction —
//!   [`ErrorClass::Retryable`] retries the same step up to
//!   [`LoopConfig::retries_per_step`] times with [`LoopConfig::retry_backoff_ms`];
//!   [`ErrorClass::Recoverable`] records the failure, refreshes the context
//!   (window/app lists) and lets the agent decide again;
//!   [`ErrorClass::Fatal`] stops the run. [`LoopConfig::max_consecutive_failures`]
//!   consecutive failures stop the run with
//!   [`StopReason::FailureBudgetExhausted`].

use adesk_core::{ActionId, Observation};
use serde::{Deserialize, Serialize};

use crate::client::AgentClient;
use crate::context::{ContextBudget, ContextBuilder, TaskDescription};
use crate::decision::AgentDecision;
use crate::metrics::{Metrics, MetricsReport, StopReason};
use crate::provider::LlmProvider;
use crate::Result;

/// Loop tuning knobs.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Maximum loop iterations before giving up.
    pub max_steps: u32,
    /// Deadline for a single provider call or client call.
    pub step_timeout_ms: u64,
    /// Whether to observe `quiet` automatically after seat input.
    pub observe_after_input: bool,
    /// Quiet period used by the automatic observation.
    pub quiet_ms: u64,
    /// Deadline for observations.
    pub observe_timeout_ms: u64,
    /// Default image policy: attach one image per context.
    pub include_image: bool,
    /// Default downscale target for attached images.
    pub capture_max_dimension: Option<u32>,
    /// Consecutive failed steps tolerated before stopping.
    pub max_consecutive_failures: u32,
    /// Retries of the same step for retryable errors.
    pub retries_per_step: u32,
    /// Backoff between retries.
    pub retry_backoff_ms: u64,
    /// Validate the runtime's AGP version via `ping` before the first step.
    pub validate_protocol_version: bool,
    /// Context caps.
    pub context_budget: ContextBudget,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 20,
            step_timeout_ms: 30_000,
            observe_after_input: true,
            quiet_ms: 250,
            observe_timeout_ms: 5_000,
            include_image: true,
            capture_max_dimension: Some(1024),
            max_consecutive_failures: 3,
            retries_per_step: 2,
            retry_backoff_ms: 200,
            validate_protocol_version: true,
            context_budget: ContextBudget::default(),
        }
    }
}

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

/// The agent loop, generic over the client and provider seams.
#[derive(Debug)]
pub struct AgentLoop<C, P> {
    client: C,
    provider: P,
    config: LoopConfig,
    context: ContextBuilder,
    metrics: Metrics,
    history: Vec<StepRecord>,
    last_action_id: Option<ActionId>,
    runtime: Option<crate::client::RuntimeInfo>,
    last_error: Option<String>,
}

impl<C: AgentClient, P: LlmProvider> AgentLoop<C, P> {
    /// Build a loop; the context builder uses [`LoopConfig::context_budget`].
    pub fn new(client: C, provider: P, config: LoopConfig) -> Self {
        let context = ContextBuilder::new(config.context_budget);
        Self {
            client,
            provider,
            config,
            context,
            metrics: Metrics::new(),
            history: Vec::new(),
            last_action_id: None,
            runtime: None,
            last_error: None,
        }
    }

    /// The active configuration.
    pub fn config(&self) -> &LoopConfig {
        &self.config
    }

    /// The metrics recorder (inspectable before the run ends).
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Steps recorded so far.
    pub fn history(&self) -> &[StepRecord] {
        &self.history
    }

    /// The context builder (tests inspect its caps and image count).
    pub fn context(&self) -> &ContextBuilder {
        &self.context
    }

    /// Id of the most recently executed action (`after_action` wiring source).
    pub fn last_action_id(&self) -> Option<ActionId> {
        self.last_action_id
    }

    /// Run `task` until `Finish`, step budget, or failure budget.
    ///
    /// Validates `ping().protocol_version` first when
    /// [`LoopConfig::validate_protocol_version`] is set. Always returns a
    /// [`LoopOutcome`] for budget stops; only fatal errors return `Err`.
    pub async fn run(&mut self, task: &TaskDescription) -> Result<LoopOutcome> {
        let _ = task;
        todo!("phase 2: implement plan -> act -> observe -> decide")
    }
}
