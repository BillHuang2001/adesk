//! The agent control loop: plan → act → observe → decide.
//!
//! One iteration:
//!
//! 1. **plan** — [`ContextBuilder`] assembles a bounded [`AgentContext`] from the latest
//!    runtime facts, then [`LlmProvider::complete`] returns one [`AgentDecision`].
//! 2. **act** — executed through [`AgentClient`]; input decisions (click/type/keypress/scroll)
//!    return an `action_id`, remembered as `last_action_id`, runtime-native decisions execute
//!    directly.
//! 3. **observe** — after seat input the loop observes `quiet` with
//!    `after_action = last_action_id`, so the observation describes causal history.
//! 4. **decide** — the step is recorded, metrics are updated, the budgets are checked.
//!
//! ## Budgets, timeouts, recovery
//!
//! [`LoopConfig::max_steps`] bounds iterations ([`StopReason::StepBudgetExhausted`]); every
//! provider and client call is bounded by [`LoopConfig::step_timeout_ms`]
//! ([`Error::StepTimeout`]). [`Error::class`] drives recovery: [`ErrorClass::Retryable`]
//! retries the same step up to [`LoopConfig::retries_per_step`] times with
//! [`LoopConfig::retry_backoff_ms`]; [`ErrorClass::Recoverable`] records the failure, refreshes
//! the context (window/app lists) and lets the agent decide again; [`ErrorClass::Fatal`] stops
//! the run. [`LoopConfig::max_consecutive_failures`] consecutive failures stop with
//! [`StopReason::FailureBudgetExhausted`].
//!
//! ## Per-decision AGP calls
//!
//! | Decision | AGP calls |
//! |---|---|
//! | `list_apps` | `list_apps` — caches the app list |
//! | `list_windows` | `list_windows` — caches windows and the active window |
//! | `get_window` | `get_window` — replaces that cached window |
//! | `launch_app` | `launch_app` |
//! | `activate_window` / `close_window` | the matching call; the returned action id becomes `last_action_id` |
//! | `capture` | `capture_window` — one readback; caches the returned window metadata |
//! | `observe` | `observe`, with `after_action` defaulting to `last_action_id` |
//! | `wait` | `observe` with `include_image = false` (never a readback) |
//! | `click` / `type` / `keypress` / `scroll` | the matching seat call, then — when [`LoopConfig::observe_after_input`] — one `observe` carrying `after_action = last_action_id`, `until = quiet(quiet_ms)`, the loop's image policy and [`LoopConfig::observe_timeout_ms`] |
//! | `finish` | none |
//!
//! `ping` is issued once before the first decision when
//! [`LoopConfig::validate_protocol_version`] is set, and a recoverable step failure adds one
//! `list_windows` refresh. Nothing else calls the runtime, so a scripted client sees exactly
//! this sequence. Events are *not* recorded here: the [`AgentClient`] seam returns temporal
//! [`Observation`]s, not raw [`adesk_core::RuntimeEvent`]s, so the loop never invents event
//! timestamps; `ContextBuilder::record_events` is for callers that own an event stream.

use std::time::{Duration, Instant};

use adesk_core::{ActionId, AppInfo, Observation, WindowId, WindowInfo};
use adesk_proto::ImagePayload;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::client::{
    AgentClient, CaptureRequest, ClickRequest, ObserveRequest, RuntimeInfo, ScrollRequest,
    PROTOCOL_VERSION,
};
use crate::context::{
    ActionRecord, AgentContext, ContextBudget, ContextBuilder, ContextInput, TaskDescription,
};
use crate::decision::{AgentDecision, ObserveCondition};
use crate::error::{Error, ErrorClass};
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
    /// AGP client seam.
    client: C,
    /// LLM seam.
    provider: P,
    config: LoopConfig,
    context: ContextBuilder,
    metrics: Metrics,
    history: Vec<StepRecord>,
    last_action_id: Option<ActionId>,
    /// Runtime capabilities cached from `ping`.
    runtime: Option<RuntimeInfo>,
    /// Last error string shown to the agent.
    last_error: Option<String>,
}

/// One bounded AGP call, retrying retryable errors up to
/// [`LoopConfig::retries_per_step`].
///
/// `$attempts` counts failed attempts, including the one that is finally
/// returned, so the caller can report [`StepStatus::Recovered`]; each failure is
/// recorded in the metrics before the retry.
macro_rules! client_call {
    ($self:expr, $step:expr, $kind:expr, $attempts:expr, $call:expr) => {{
        let mut retry: u32 = 0;
        loop {
            // Scope the future so its borrow of the client ends before the
            // failure path touches the metrics.
            let result = {
                let call = $call;
                bounded_call($self.config.step_timeout_ms, $step, call).await
            };
            match result {
                Ok(value) => break Ok(value),
                Err(error) => {
                    $attempts += 1;
                    $self.metrics.record_failure($kind, &error);
                    if error.class() == ErrorClass::Retryable
                        && retry < $self.config.retries_per_step
                    {
                        retry += 1;
                        retry_backoff($self.config.retry_backoff_ms).await;
                    } else {
                        break Err(error);
                    }
                }
            }
        }
    }};
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
        self.validate_runtime().await?;

        let mut facts = RuntimeFacts::default();
        let mut steps: u32 = 0;
        let mut consecutive_failures: u32 = 0;

        while steps < self.config.max_steps {
            let step = steps;
            self.metrics.record_step();

            // plan — bounded context, then one decision.
            let context = self.build_context(task, &facts, step);
            let (decision, decision_latency_ms) = self.decide(&context, step).await?;
            self.metrics
                .record_decision(decision.kind(), decision_latency_ms);
            let started = Instant::now();

            if let AgentDecision::Finish { success, summary } = &decision {
                let (success, summary) = (*success, summary.clone());
                self.history.push(StepRecord {
                    step,
                    decision,
                    action_id: None,
                    observation: None,
                    status: StepStatus::Finished,
                    error: None,
                    decision_latency_ms,
                    elapsed_ms: elapsed_ms(started),
                    readback: false,
                });
                debug!(step, success, "task finished by the provider");
                steps += 1;
                return Ok(self.outcome(task, success, summary, steps, StopReason::Finished));
            }

            // act + observe.
            let mut execution = Execution::default();
            let result = self
                .execute(&decision, &mut facts, step, &mut execution)
                .await;
            let step_elapsed_ms = elapsed_ms(started);

            match result {
                Ok(()) => {
                    consecutive_failures = 0;
                    self.record_step(
                        step,
                        &decision,
                        &execution,
                        None,
                        decision_latency_ms,
                        step_elapsed_ms,
                    );
                }
                Err(error) => {
                    consecutive_failures += 1;
                    let class = error.class();
                    self.record_step(
                        step,
                        &decision,
                        &execution,
                        Some(&error),
                        decision_latency_ms,
                        step_elapsed_ms,
                    );
                    if class == ErrorClass::Fatal {
                        return Err(error);
                    }
                    if class == ErrorClass::Recoverable {
                        // The decision named something the runtime no longer
                        // knows: refresh the window list and let the agent
                        // decide again.
                        self.refresh_windows(&mut facts, step).await;
                    }
                    if consecutive_failures >= self.config.max_consecutive_failures {
                        let summary =
                            Error::FailureBudgetExhausted(consecutive_failures).to_string();
                        steps += 1;
                        return Ok(self.outcome(
                            task,
                            false,
                            summary,
                            steps,
                            StopReason::FailureBudgetExhausted,
                        ));
                    }
                }
            }
            steps += 1;
        }

        let summary = Error::StepBudgetExhausted(steps).to_string();
        Ok(self.outcome(task, false, summary, steps, StopReason::StepBudgetExhausted))
    }

    /// `ping` the runtime once, before the first decision, and validate the AGP
    /// version (only when [`LoopConfig::validate_protocol_version`] is set).
    async fn validate_runtime(&mut self) -> Result<()> {
        if !self.config.validate_protocol_version {
            return Ok(());
        }
        let info = bounded_call(self.config.step_timeout_ms, 0, self.client.ping()).await?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(Error::ProtocolVersion {
                expected: PROTOCOL_VERSION,
                got: info.protocol_version,
            });
        }
        debug!(
            version = info.protocol_version,
            renderer = %info.renderer,
            "runtime validated"
        );
        self.runtime = Some(info);
        Ok(())
    }

    /// Assemble the bounded context the provider sees for `step`.
    fn build_context(
        &self,
        task: &TaskDescription,
        facts: &RuntimeFacts,
        step: u32,
    ) -> AgentContext {
        self.context.build(ContextInput {
            task,
            step,
            max_steps: self.config.max_steps,
            runtime: self.runtime.as_ref(),
            windows: &facts.windows,
            active_window: facts.active_window,
            apps: &facts.apps,
            observation: facts.observation.as_ref(),
            last_error: self.last_error.as_deref(),
        })
    }

    /// Ask the provider for one decision, retrying transient provider failures.
    ///
    /// Returns the decision and its latency. A provider that keeps failing after
    /// [`LoopConfig::retries_per_step`] ends the run: without a decision there is
    /// no step to record.
    async fn decide(&self, context: &AgentContext, step: u32) -> Result<(AgentDecision, u64)> {
        let mut retry: u32 = 0;
        loop {
            let started = Instant::now();
            let result = bounded_call(self.config.step_timeout_ms, step, async {
                self.provider
                    .complete(context)
                    .await
                    .map_err(Error::Provider)
            })
            .await;
            match result {
                Ok(decision) => return Ok((decision, elapsed_ms(started))),
                Err(error) => {
                    if error.class() == ErrorClass::Retryable
                        && retry < self.config.retries_per_step
                    {
                        retry += 1;
                        warn!(step, attempt = retry, error = %error, "provider failed, retrying");
                        retry_backoff(self.config.retry_backoff_ms).await;
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Execute one decision through the client, accumulating what it produced.
    ///
    /// Client calls are bounded by [`LoopConfig::step_timeout_ms`]; every failed attempt is
    /// recorded in the metrics before it is retried, so `recoveries` counts failed→successful
    /// transitions. A partial execution (an input that landed but whose automatic observation
    /// failed) stays in `execution`.
    async fn execute(
        &mut self,
        decision: &AgentDecision,
        facts: &mut RuntimeFacts,
        step: u32,
        execution: &mut Execution,
    ) -> Result<()> {
        let kind = decision.kind();
        match decision {
            AgentDecision::ListApps { query } => {
                facts.apps = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.list_apps(query.as_deref())
                )?;
            }
            AgentDecision::ListWindows => {
                let list = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.list_windows()
                )?;
                facts.windows = list.windows;
                facts.active_window = list.active_window_id;
            }
            AgentDecision::GetWindow { window_id } => {
                let window = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.get_window(*window_id)
                )?;
                upsert_window(&mut facts.windows, window);
            }
            AgentDecision::LaunchApp { app_id, args } => {
                let outcome = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.launch_app(app_id, args)
                )?;
                debug!(step, launch_id = %outcome.launch_id, app = %outcome.app_id, "app launched");
            }
            AgentDecision::ActivateWindow { window_id } => {
                let action_id = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.activate_window(*window_id)
                )?;
                execution.action_id = Some(action_id);
                self.last_action_id = Some(action_id);
            }
            AgentDecision::CloseWindow { window_id } => {
                let action_id = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.close_window(*window_id)
                )?;
                execution.action_id = Some(action_id);
                self.last_action_id = Some(action_id);
            }
            AgentDecision::Capture {
                window_id,
                region,
                max_dimension,
            } => {
                let request = CaptureRequest {
                    window_id: *window_id,
                    region: *region,
                    max_dimension: max_dimension.or(self.config.capture_max_dimension),
                };
                let outcome = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.capture_window(&request)
                )?;
                upsert_window(&mut facts.windows, outcome.window);
                execution.image = Some(outcome.image);
            }
            AgentDecision::Observe {
                window_id,
                after_action,
                until,
                timeout_ms,
                include_image,
                max_dimension,
                region,
            } => {
                let request = ObserveRequest {
                    window_id: *window_id,
                    after_action: after_action.or(self.last_action_id),
                    until: *until,
                    timeout_ms: timeout_ms.unwrap_or(self.config.observe_timeout_ms),
                    include_image: include_image.unwrap_or(self.config.include_image),
                    max_dimension: max_dimension.or(self.config.capture_max_dimension),
                    region: *region,
                };
                let outcome = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.observe(&request)
                )?;
                record_observation(facts, execution, outcome);
            }
            AgentDecision::Wait {
                window_id,
                until,
                timeout_ms,
            } => {
                // `wait` is an observation without pixels: it never reads back.
                let request = ObserveRequest {
                    window_id: *window_id,
                    after_action: self.last_action_id,
                    until: *until,
                    timeout_ms: timeout_ms.unwrap_or(self.config.observe_timeout_ms),
                    include_image: false,
                    max_dimension: None,
                    region: None,
                };
                let outcome = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.observe(&request)
                )?;
                record_observation(facts, execution, outcome);
            }
            AgentDecision::Click {
                window_id,
                position,
                button,
                count,
            } => {
                let request = ClickRequest {
                    window_id: *window_id,
                    position: *position,
                    button: *button,
                    count: *count,
                };
                let action_id = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.click(&request)
                )?;
                execution.action_id = Some(action_id);
                self.last_action_id = Some(action_id);
                self.observe_after_input(Some(*window_id), step, kind, execution, facts)
                    .await?;
            }
            AgentDecision::Type { window_id, text } => {
                let outcome = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.type_text(text, *window_id)
                )?;
                execution.action_id = Some(outcome.action_id);
                self.last_action_id = Some(outcome.action_id);
                if !outcome.skipped.is_empty() {
                    debug!(step, skipped = ?outcome.skipped, "characters skipped by the keymap");
                }
                self.observe_after_input(*window_id, step, kind, execution, facts)
                    .await?;
            }
            AgentDecision::Keypress { window_id, keys } => {
                let action_id = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.keypress(keys, *window_id)
                )?;
                execution.action_id = Some(action_id);
                self.last_action_id = Some(action_id);
                self.observe_after_input(*window_id, step, kind, execution, facts)
                    .await?;
            }
            AgentDecision::Scroll {
                window_id,
                position,
                dx,
                dy,
            } => {
                let request = ScrollRequest {
                    window_id: *window_id,
                    position: *position,
                    dx: *dx,
                    dy: *dy,
                };
                let action_id = client_call!(
                    self,
                    step,
                    kind,
                    execution.attempts,
                    self.client.scroll(&request)
                )?;
                execution.action_id = Some(action_id);
                self.last_action_id = Some(action_id);
                self.observe_after_input(Some(*window_id), step, kind, execution, facts)
                    .await?;
            }
            AgentDecision::Finish { .. } => {
                return Err(Error::InvalidDecision(String::from(
                    "finish is handled by the loop, never executed",
                )));
            }
        }
        Ok(())
    }

    /// Observe `quiet` after seat input, carrying the action id as causal filter.
    async fn observe_after_input(
        &mut self,
        window_id: Option<WindowId>,
        step: u32,
        kind: crate::decision::ActionKind,
        execution: &mut Execution,
        facts: &mut RuntimeFacts,
    ) -> Result<()> {
        if !self.config.observe_after_input {
            return Ok(());
        }
        let request = ObserveRequest {
            window_id,
            after_action: execution.action_id,
            until: ObserveCondition::Quiet {
                quiet_ms: self.config.quiet_ms,
            },
            timeout_ms: self.config.observe_timeout_ms,
            include_image: self.config.include_image,
            max_dimension: self.config.capture_max_dimension,
            region: None,
        };
        let outcome = client_call!(
            self,
            step,
            kind,
            execution.attempts,
            self.client.observe(&request)
        )?;
        record_observation(facts, execution, outcome);
        Ok(())
    }

    /// Re-read the window list after a recoverable failure.
    ///
    /// A failed refresh is logged and ignored: the cached list simply stays
    /// stale, and the agent still sees the original error in its context.
    async fn refresh_windows(&self, facts: &mut RuntimeFacts, step: u32) {
        match bounded_call(
            self.config.step_timeout_ms,
            step,
            self.client.list_windows(),
        )
        .await
        {
            Ok(list) => {
                facts.windows = list.windows;
                facts.active_window = list.active_window_id;
            }
            Err(error) => {
                warn!(step, error = %error, "window refresh failed, keeping the cached list");
            }
        }
    }

    /// Record one executed step: action, image, metrics, context and history.
    ///
    /// `error` is `None` for a successful step. Failed *attempts* are already in the metrics
    /// (recorded by [`client_call`]); a partially executed step still counts the action that
    /// landed before the failure.
    fn record_step(
        &mut self,
        step: u32,
        decision: &AgentDecision,
        execution: &Execution,
        error: Option<&Error>,
        decision_latency_ms: u64,
        step_elapsed_ms: u64,
    ) {
        let kind = decision.kind();
        let readback = execution.image.is_some();
        match error {
            None => {
                self.metrics
                    .record_action(kind, readback, execution.image.as_ref());
                self.metrics.record_success();
                self.last_error = None;
            }
            Some(error) => {
                if execution.action_id.is_some()
                    || execution.observation.is_some()
                    || execution.image.is_some()
                {
                    self.metrics
                        .record_action(kind, readback, execution.image.as_ref());
                }
                self.last_error = Some(error.to_string());
                warn!(step, action = kind.as_str(), error = %error, "step failed");
            }
        }
        if let Some(image) = execution.image.clone() {
            self.context.set_image(Some(image));
        }
        self.context
            .record_action(action_record(step, decision, execution, error.is_none()));

        let status = match error {
            Some(_) => StepStatus::Failed,
            None if execution.attempts > 0 => StepStatus::Recovered,
            None => StepStatus::Ok,
        };
        self.history.push(StepRecord {
            step,
            decision: decision.clone(),
            action_id: execution.action_id,
            observation: execution.observation.clone(),
            status,
            error: error.map(ToString::to_string),
            decision_latency_ms,
            elapsed_ms: step_elapsed_ms,
            readback,
        });
        debug!(
            step,
            action = kind.as_str(),
            status = ?status,
            attempts = execution.attempts,
            "step recorded"
        );
    }

    /// Snapshot the run into a [`LoopOutcome`].
    fn outcome(
        &self,
        task: &TaskDescription,
        success: bool,
        summary: String,
        steps: u32,
        stop_reason: StopReason,
    ) -> LoopOutcome {
        LoopOutcome {
            success,
            summary,
            steps,
            stop_reason,
            metrics: self.metrics.report(&task.goal, success, stop_reason),
            history: self.history.clone(),
        }
    }
}

/// Live runtime facts cached between steps; nothing here grows with step count.
#[derive(Debug, Default)]
struct RuntimeFacts {
    windows: Vec<WindowInfo>,
    active_window: Option<WindowId>,
    apps: Vec<AppInfo>,
    observation: Option<Observation>,
}

/// What one executed decision produced.
#[derive(Debug, Default)]
struct Execution {
    /// Action id returned by an input or state-mutating call.
    action_id: Option<ActionId>,
    /// Observation returned by the step, when any.
    observation: Option<Observation>,
    /// Image returned by the step, when any (a readback).
    image: Option<ImagePayload>,
    /// Failed attempts before the step succeeded (or the total, on failure).
    attempts: u32,
}

/// Deadline for one provider or client call.
async fn bounded_call<T>(
    timeout_ms: u64,
    step: u32,
    call: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(Duration::from_millis(timeout_ms), call).await {
        Ok(result) => result,
        Err(_) => Err(Error::StepTimeout { step, timeout_ms }),
    }
}

/// Sleep between retries (`0` = no wait).
async fn retry_backoff(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// Milliseconds elapsed since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Remember an observation in the step and in the cached runtime facts.
fn record_observation(
    facts: &mut RuntimeFacts,
    execution: &mut Execution,
    outcome: crate::client::ObserveOutcome,
) {
    facts.observation = Some(outcome.observation.clone());
    execution.image = outcome.image;
    execution.observation = Some(outcome.observation);
}

/// Insert or replace one window in the cached list.
fn upsert_window(windows: &mut Vec<WindowInfo>, window: WindowInfo) {
    match windows.iter_mut().find(|known| known.id == window.id) {
        Some(known) => *known = window,
        None => windows.push(window),
    }
}

/// One context action record for an executed decision.
fn action_record(
    step: u32,
    decision: &AgentDecision,
    execution: &Execution,
    ok: bool,
) -> ActionRecord {
    ActionRecord {
        step,
        action_id: execution.action_id,
        kind: decision.kind(),
        window_id: decision_window(decision),
        position: match decision {
            AgentDecision::Click { position, .. } | AgentDecision::Scroll { position, .. } => {
                Some(*position)
            }
            _ => None,
        },
        detail: describe_decision(decision),
        ok,
    }
}

/// Target window of a decision, when it names one.
fn decision_window(decision: &AgentDecision) -> Option<WindowId> {
    match decision {
        AgentDecision::GetWindow { window_id }
        | AgentDecision::ActivateWindow { window_id }
        | AgentDecision::CloseWindow { window_id }
        | AgentDecision::Capture { window_id, .. }
        | AgentDecision::Click { window_id, .. }
        | AgentDecision::Scroll { window_id, .. } => Some(*window_id),
        AgentDecision::Observe { window_id, .. }
        | AgentDecision::Wait { window_id, .. }
        | AgentDecision::Type { window_id, .. }
        | AgentDecision::Keypress { window_id, .. } => *window_id,
        AgentDecision::ListApps { .. }
        | AgentDecision::ListWindows
        | AgentDecision::LaunchApp { .. }
        | AgentDecision::Finish { .. } => None,
    }
}

/// One-line, payload-free description of a decision for the context.
fn describe_decision(decision: &AgentDecision) -> String {
    match decision {
        AgentDecision::ListApps { query } => match query {
            Some(query) => format!("query={query}"),
            None => String::from("all apps"),
        },
        AgentDecision::ListWindows => String::from("windows + active"),
        AgentDecision::GetWindow { window_id } => format!("window {window_id}"),
        AgentDecision::LaunchApp { app_id, args } => format!("app={app_id} args={args:?}"),
        AgentDecision::ActivateWindow { window_id } => format!("activate window {window_id}"),
        AgentDecision::CloseWindow { window_id } => format!("close window {window_id}"),
        AgentDecision::Capture {
            window_id,
            region,
            max_dimension,
        } => {
            format!("capture window {window_id} region={region:?} max_dimension={max_dimension:?}")
        }
        AgentDecision::Observe {
            window_id, until, ..
        } => format!(
            "observe window_id={window_id:?} until={}",
            describe_condition(*until)
        ),
        AgentDecision::Wait {
            window_id, until, ..
        } => format!(
            "wait window_id={window_id:?} until={}",
            describe_condition(*until)
        ),
        AgentDecision::Click {
            position,
            button,
            count,
            ..
        } => format!("click at {position:?} button={button:?} count={count}"),
        AgentDecision::Type { text, .. } => format!("type {:?}", truncate(text, 40)),
        AgentDecision::Keypress { keys, .. } => format!("keypress {keys:?}"),
        AgentDecision::Scroll {
            position, dx, dy, ..
        } => {
            format!("scroll at {position:?} dx={dx} dy={dy}")
        }
        AgentDecision::Finish { success, .. } => format!("finish success={success}"),
    }
}

/// Compact description of an observation condition.
fn describe_condition(condition: ObserveCondition) -> String {
    match condition {
        ObserveCondition::Quiet { quiet_ms } => format!("quiet({quiet_ms})"),
        ObserveCondition::Change => String::from("change"),
        ObserveCondition::Timeout => String::from("timeout"),
    }
}

/// Truncate `text` to `max` characters, marking the cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push_str("...");
    truncated
}
