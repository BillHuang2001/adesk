//! Decision execution: one [`AgentDecision`] turned into AGP calls, retries,
//! observations and a recorded step.
//!
//! Everything here is plumbing for a single iteration — deadlines and retry
//! backoff ([`bounded_call`], [`retry_backoff`]), the per-decision call table,
//! the automatic post-input observation, the recoverable-failure window refresh
//! and the step record the run report is built from. The control flow that
//! decides *which* decision to execute lives in [`crate::agent_loop`].

use std::time::{Duration, Instant};

use adesk_core::{ErrorCode, WindowId, WindowInfo};
use tracing::{debug, warn};

use crate::client::{
    AccessibilityTreeRequest, AgentClient, CaptureRequest, ClickRequest, FindAccessibleOutcome,
    FindAccessibleRequest, InvokeAccessibleActionRequest, ObserveOutcome, ObserveRequest,
    ScrollRequest,
};
use crate::context::ActionRecord;
use crate::decision::{ActionKind, AgentDecision, ObserveCondition};
use crate::error::{Error, ErrorClass};
use crate::provider::LlmProvider;
use crate::Result;

use super::step::{Execution, RuntimeFacts, StepRecord, StepStatus};
use super::AgentLoop;

/// One bounded AGP call, retrying retryable errors up to
/// [`LoopConfig::retries_per_step`].
///
/// `$attempts` counts failed attempts, including the one that is finally
/// returned, so the caller can report [`StepStatus::Recovered`]; each failure is
/// recorded in the metrics before the retry.
///
/// [`LoopConfig::retries_per_step`]: crate::agent_loop::LoopConfig::retries_per_step
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
    /// Execute one decision through the client, accumulating what it produced.
    ///
    /// Client calls are bounded by [`LoopConfig::step_timeout_ms`]; every failed attempt is
    /// recorded in the metrics before it is retried, so `recoveries` counts failed→successful
    /// transitions. A partial execution (an input that landed but whose automatic observation
    /// failed) stays in `execution`.
    ///
    /// [`LoopConfig::step_timeout_ms`]: crate::agent_loop::LoopConfig::step_timeout_ms
    pub(super) async fn execute(
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
                    // The provider's capability is a hard gate: a decision cannot
                    // pull pixels into a context the provider cannot read.
                    include_image: self.image_policy(*include_image),
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
                // Proactive text-first enrichment: once an observation resolves,
                // pull the observed window's accessibility outline so the next
                // decision can reason over UI text without a pixel readback.
                if self.config.include_accessibility {
                    let observed = facts
                        .observation
                        .as_ref()
                        .and_then(|observation| observation.window_id)
                        .or(*window_id);
                    self.enrich_accessibility(observed, None, kind, facts, step, execution)
                        .await?;
                }
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
            AgentDecision::AccessibilityTree {
                window_id,
                max_nodes,
            } => {
                if !self.config.include_accessibility {
                    // Opt-in capability: with the gate off the loop makes no
                    // runtime call at all and leaves a benign note for the agent
                    // instead of failing the step.
                    self.last_error = Some(String::from(
                        "accessibility is disabled; set include_accessibility",
                    ));
                    return Ok(());
                }
                self.enrich_accessibility(*window_id, *max_nodes, kind, facts, step, execution)
                    .await?;
            }
            AgentDecision::FindAccessible {
                window_id,
                role,
                name,
                name_contains,
                value_contains,
                max_results,
            } => {
                if !self.config.include_accessibility {
                    // Same opt-in gate as `AccessibilityTree`: with it off the
                    // loop makes no runtime call and leaves a benign note
                    // instead of failing the step.
                    self.last_error = Some(String::from(
                        "accessibility is disabled; set include_accessibility",
                    ));
                    return Ok(());
                }
                let request = FindAccessibleRequest {
                    window_id: *window_id,
                    role: role.clone(),
                    name: name.clone(),
                    name_contains: name_contains.clone(),
                    value_contains: value_contains.clone(),
                    max_results: *max_results,
                };
                let outcome = client_call!(self, step, kind, execution.attempts, async {
                    degrade_accessibility(
                        step,
                        "find_accessible",
                        self.client.find_accessible(&request).await,
                    )
                })?;
                match outcome {
                    Some(outcome) => {
                        // The found elements land in the SAME accessibility slot
                        // as a tree outline, so the model reads UI text with no
                        // pixel readback.
                        facts.accessibility = Some(render_accessible_matches(&outcome));
                    }
                    None => {
                        facts.accessibility = None;
                        self.last_error = Some(String::from("find_accessible unavailable"));
                    }
                }
            }
            AgentDecision::InvokeAccessibleAction { node_id, action } => {
                if !self.config.include_accessibility {
                    self.last_error = Some(String::from(
                        "accessibility is disabled; set include_accessibility",
                    ));
                    return Ok(());
                }
                let request = InvokeAccessibleActionRequest {
                    node_id: *node_id,
                    action: action.clone(),
                };
                let outcome = client_call!(self, step, kind, execution.attempts, async {
                    degrade_accessibility(
                        step,
                        "invoke_accessible_action",
                        self.client.invoke_accessible_action(&request).await,
                    )
                })?;
                match outcome {
                    Some(action_id) => {
                        // Runtime-native actuation: the returned action id is the
                        // loop's causal anchor, exactly like `activate_window`.
                        execution.action_id = Some(action_id);
                        self.last_action_id = Some(action_id);
                    }
                    None => {
                        self.last_error =
                            Some(String::from("invoke_accessible_action unavailable"));
                    }
                }
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
    pub(super) async fn observe_after_input(
        &mut self,
        window_id: Option<WindowId>,
        step: u32,
        kind: ActionKind,
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
            include_image: self.image_policy(None),
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
        // Same proactive text-first enrichment as the explicit `observe` arm.
        if self.config.include_accessibility {
            let observed = facts
                .observation
                .as_ref()
                .and_then(|observation| observation.window_id)
                .or(window_id);
            self.enrich_accessibility(observed, None, kind, facts, step, execution)
                .await?;
        }
        Ok(())
    }

    /// Fetch a window's accessibility outline and cache it in `facts`.
    ///
    /// `window_id` is `None` for the runtime's active window. This is the single
    /// implementation of the accessibility call: the explicit
    /// [`AgentDecision::AccessibilityTree`] arm and the proactive text-first
    /// enrichment both go through it, so the degradation policy lives in exactly
    /// one place.
    ///
    /// A backend that cannot serve the request (`not_supported` /
    /// `unknown_accessible` / `unknown_window`) is degraded benignly: the error is
    /// swallowed *inside* `client_call!`, so it records no failure and never ends
    /// the run — the cached outline is cleared and a note is left in `last_error`.
    /// Every other error keeps the generic [`ErrorClass`] policy (retryable errors
    /// are retried, anything else fails the step).
    async fn enrich_accessibility(
        &mut self,
        window_id: Option<WindowId>,
        max_nodes: Option<u32>,
        kind: ActionKind,
        facts: &mut RuntimeFacts,
        step: u32,
        execution: &mut Execution,
    ) -> Result<()> {
        let request = AccessibilityTreeRequest {
            window_id,
            max_nodes,
        };
        let outcome = client_call!(self, step, kind, execution.attempts, async {
            degrade_accessibility(
                step,
                "accessibility_tree",
                self.client.accessibility_tree(&request).await,
            )
        })?;
        match outcome {
            Some(outcome) => {
                let window = outcome.window_id;
                facts.accessibility = Some(outcome.text);
                // Mirror the `Capture` arm's window upsert. The outcome carries
                // only the window id (no metadata), so an untracked window is
                // refreshed through `get_window` — best-effort, like
                // [`AgentLoop::refresh_windows`]: a failed refresh leaves the
                // cached list untouched.
                if !facts.windows.iter().any(|known| known.id == window) {
                    match bounded_call(
                        self.config.step_timeout_ms,
                        step,
                        self.client.get_window(window),
                    )
                    .await
                    {
                        Ok(window) => upsert_window(&mut facts.windows, window),
                        Err(error) => debug!(
                            step,
                            window = %window,
                            error = %error,
                            "accessibility window not refreshed"
                        ),
                    }
                }
            }
            None => {
                facts.accessibility = None;
                self.last_error = Some(String::from("accessibility tree unavailable"));
            }
        }
        Ok(())
    }

    /// Re-read the window list after a recoverable failure.
    ///
    /// A failed refresh is logged and ignored: the cached list simply stays
    /// stale, and the agent still sees the original error in its context.
    pub(super) async fn refresh_windows(&self, facts: &mut RuntimeFacts, step: u32) {
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
    /// (recorded by the `client_call!` macro); a partially executed step still counts the
    /// action that landed before the failure.
    ///
    /// Invariant: **pixels only ever reach a provider that can consume them.** Any readback
    /// is recorded as such, but it is attached to the context only when
    /// [`LlmProvider::supports_images`] holds; observation requests are gated earlier, when
    /// the request is built (`AgentLoop::image_policy`).
    ///
    /// [`LlmProvider::supports_images`]: crate::provider::LlmProvider::supports_images
    pub(super) fn record_step(
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
        // The capability gate is unconditional here: an explicit `capture` has no
        // `include_image` flag for `image_policy` to fold the provider's capability
        // into, so this is the only place that can keep its readback out of a
        // text-only provider's context.
        if self.provider.supports_images() {
            if let Some(image) = execution.image.clone() {
                self.context.set_image(Some(image));
            }
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
}

/// Deadline for one provider or client call.
///
/// Shared with the loop's control flow ([`crate::agent_loop`]), which bounds
/// `ping` and the provider call the same way.
pub(super) async fn bounded_call<T>(
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
pub(super) async fn retry_backoff(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// Milliseconds elapsed since `started`, saturating.
pub(super) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Remember an observation in the step and in the cached runtime facts.
fn record_observation(
    facts: &mut RuntimeFacts,
    execution: &mut Execution,
    outcome: ObserveOutcome,
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

/// True for the errors that mean the accessibility backend simply cannot serve
/// this request.
///
/// They are degraded rather than fatal: the loop continues without accessibility
/// text. `NotSupported` (no backend / build without one), `UnknownAccessible`
/// (stale element) and `UnknownWindow` (stale window) are listed explicitly,
/// because the generic [`ErrorClass`] policy would treat `NotSupported` as
/// [`ErrorClass::Fatal`] and end the run.
fn is_accessibility_unavailable(error: &Error) -> bool {
    matches!(
        error,
        Error::Client(err)
            if matches!(
                err.code,
                ErrorCode::NotSupported | ErrorCode::UnknownAccessible | ErrorCode::UnknownWindow
            )
    )
}

/// Classify one §5.11 accessibility call result, degrading a backend-unavailable
/// error to `Ok(None)`.
///
/// This is the single implementation of the accessibility degradation policy:
/// [`AgentLoop::enrich_accessibility`] and the `FindAccessible` /
/// `InvokeAccessibleAction` arms all funnel their result through it, so a
/// missing, stale or unsupported backend is a benign no-op in exactly one place.
/// Every other error is passed through unchanged and keeps the generic
/// [`ErrorClass`] policy.
fn degrade_accessibility<T>(step: u32, what: &str, result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if is_accessibility_unavailable(&error) => {
            warn!(
                step,
                operation = what,
                error = %error,
                "accessibility backend unavailable, continuing without it"
            );
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Render `find_accessible` matches as a compact TEXT block for the model.
///
/// One line per match (role, name, value, node id and ancestor path), so the
/// agent can pick an element to invoke without any pixel readback. The block is
/// stored in the same accessibility slot as a tree outline, so it is truncated
/// by the same `max_accessibility_chars` context budget.
fn render_accessible_matches(outcome: &FindAccessibleOutcome) -> String {
    let mut text = format!(
        "find_accessible window={} matches={} truncated={}",
        outcome.window_id,
        outcome.matches.len(),
        outcome.truncated
    );
    if outcome.matches.is_empty() {
        text.push_str("\n(no matches)");
    }
    for element in &outcome.matches {
        text.push_str(&format!(
            "\n- {} \"{}\" value={:?} id={} path={:?}",
            element.role, element.name, element.value, element.id, element.path
        ));
    }
    text
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
        | AgentDecision::AccessibilityTree { window_id, .. }
        | AgentDecision::FindAccessible { window_id, .. }
        | AgentDecision::Type { window_id, .. }
        | AgentDecision::Keypress { window_id, .. } => *window_id,
        AgentDecision::ListApps { .. }
        | AgentDecision::ListWindows
        | AgentDecision::LaunchApp { .. }
        | AgentDecision::InvokeAccessibleAction { .. }
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
        AgentDecision::AccessibilityTree {
            window_id,
            max_nodes,
        } => format!("accessibility_tree window_id={window_id:?} max_nodes={max_nodes:?}"),
        AgentDecision::FindAccessible {
            window_id,
            role,
            name,
            name_contains,
            value_contains,
            max_results,
        } => format!(
            "find_accessible window_id={window_id:?} role={role:?} name={name:?} \
             name_contains={name_contains:?} value_contains={value_contains:?} \
             max_results={max_results:?}"
        ),
        AgentDecision::InvokeAccessibleAction { node_id, action } => {
            format!("invoke_accessible_action node_id={node_id} action={action:?}")
        }
        AgentDecision::Click {
            position,
            button,
            count,
            ..
        } => format!("click at {position:?} button={button:?} count={count}"),
        // Typed text is arbitrary; the context keeps one short, payload-free line.
        AgentDecision::Type { text, .. } => {
            format!("type {:?}", crate::text::truncate(text, 40, "..."))
        }
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
