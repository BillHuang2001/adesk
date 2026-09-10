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
//! [`adesk_core::Observation`]s, not raw [`adesk_core::RuntimeEvent`]s, so the loop never
//! invents event timestamps; `ContextBuilder::record_events` is for callers that own an
//! event stream.
//!
//! ## Module layout
//!
//! | File | Responsibility |
//! |---|---|
//! | `mod.rs` (this file) | the loop itself: lifecycle, `run`, context assembly, outcome |
//! | `config.rs` | [`LoopConfig`] and its defaults |
//! | `step.rs` | per-step records, the run outcome, and the loop's bookkeeping structs |
//! | `execute.rs` | decision execution: AGP calls, retries, observation, step recording |
//! | `tests.rs` | inline unit tests |

mod config;
mod execute;
mod step;

#[cfg(test)]
mod tests;

pub use config::LoopConfig;
pub use step::{LoopOutcome, StepRecord, StepStatus};

use std::time::Instant;

use adesk_core::ActionId;
use tracing::{debug, warn};

use crate::client::{AgentClient, RuntimeInfo, PROTOCOL_VERSION};
use crate::context::{AgentContext, ContextBuilder, ContextInput, TaskDescription};
use crate::decision::AgentDecision;
use crate::error::{Error, ErrorClass};
use crate::metrics::{Metrics, StopReason};
use crate::provider::LlmProvider;
use crate::Result;

use self::execute::{bounded_call, elapsed_ms, retry_backoff};
use self::step::{Execution, RuntimeFacts};

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

    /// Whether pixels may cross to the provider.
    ///
    /// Two independent gates, both mandatory: the configured policy — `requested`
    /// when the decision names one, [`LoopConfig::include_image`] otherwise — and
    /// [`LlmProvider::supports_images`]. A text-only provider therefore never
    /// receives a frame, even from a decision that asked for one.
    fn image_policy(&self, requested: Option<bool>) -> bool {
        requested.unwrap_or(self.config.include_image) && self.provider.supports_images()
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
