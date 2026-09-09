//! Metrics: the measurable cost and quality of one agent task.
//!
//! Semantics (these are the definitions the reports and tests rely on):
//!
//! | Field | Meaning |
//! |---|---|
//! | `steps` | loop iterations executed (context → decision → act) |
//! | `decisions` | provider decisions received, including `Finish` |
//! | `actions` | executed decisions excluding `Finish` |
//! | `actions_by_kind` | per-[`ActionKind`] counts of `actions` |
//! | `runtime_ops` / `input_actions` | split of `actions` into runtime-native vs seat input |
//! | `gpu_readbacks` | AGP calls that returned an `ImagePayload` (`capture` and `observe` with `include_image`) |
//! | `images_sent` | images embedded in provider contexts (current + keyframe count) |
//! | `visual_tokens` | Σ [`estimate_visual_tokens`] over `images_sent` |
//! | `decision_latency` | per-`complete` wall time: min / mean / p95 (nearest-rank) |
//! | `failures` | steps whose decision or execution errored |
//! | `failure_rate` | `failures / steps` (0 when no steps) |
//! | `recoveries` | failed steps immediately followed by a successful step |
//! | `recovery_rate` | `recoveries / failures` (0 when no failures) |
//! | `elapsed_ms` | monotonic wall time of the whole run |
//!
//! "Actions per task" therefore counts agent-visible effects, not observations
//! the runtime performs internally; "GPU readbacks" counts only frames actually
//! read back for the agent, which is the number the on-demand rendering design
//! promises to minimize.

use std::collections::BTreeMap;
use std::time::Instant;

use adesk_proto::ImagePayload;
use serde::{Deserialize, Serialize};

use crate::decision::ActionKind;
use crate::error::Error;

/// Why the loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The provider emitted `Finish`.
    Finished,
    /// The step budget was exhausted first.
    StepBudgetExhausted,
    /// The consecutive-failure budget was exhausted first.
    FailureBudgetExhausted,
    /// A fatal error ended the run.
    FatalError,
}

/// Latency summary in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencyStats {
    /// Number of samples.
    pub count: u64,
    /// Minimum observed latency.
    pub min_ms: u64,
    /// Arithmetic mean.
    pub mean_ms: f64,
    /// 95th percentile (nearest-rank).
    pub p95_ms: u64,
}

/// Number of 512-pixel tiles one image edge spans (never zero).
///
/// A degenerate edge (0 px) still occupies a tile, which keeps
/// [`estimate_visual_tokens`] non-zero and monotonic for any real image.
fn tiles(edge: u32) -> u64 {
    u64::from(edge).div_ceil(512).max(1)
}

/// Estimate visual tokens for an image of the given pixel size.
///
/// Provider-agnostic heuristic matching the common 28px-patch / 512px-tile
/// accounting: `85 + 170 * tiles`, where `tiles = ceil(w/512) * ceil(h/512)`
/// (1 tile when both edges are ≤ 512). Good enough for cost tracking; it is an
/// *estimate*, never billed truth.
pub fn estimate_visual_tokens(width: u32, height: u32) -> u64 {
    85 + 170 * tiles(width) * tiles(height)
}

/// Compute min/mean/p95 from raw samples (p95 by nearest rank, empty → zeros).
///
/// The nearest-rank percentile is the `ceil(0.95 * n)`-th smallest sample
/// (1-based), so for ten samples it is the largest one and it never
/// interpolates between samples.
pub fn latency_stats(samples: &[u64]) -> LatencyStats {
    if samples.is_empty() {
        return LatencyStats::default();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let count = sorted.len();
    let sum: u128 = sorted.iter().map(|&sample| u128::from(sample)).sum();
    let rank = (95 * count).div_ceil(100).clamp(1, count);
    LatencyStats {
        count: count as u64,
        min_ms: sorted[0],
        mean_ms: sum as f64 / count as f64,
        p95_ms: sorted[rank - 1],
    }
}

/// Mutable recorder owned by the loop; [`Metrics::report`] snapshots it.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    steps: u32,
    decisions: u32,
    actions: u32,
    actions_by_kind: BTreeMap<ActionKind, u32>,
    runtime_ops: u32,
    input_actions: u32,
    gpu_readbacks: u32,
    images_sent: u32,
    visual_tokens: u64,
    latencies_ms: Vec<u64>,
    failures: u32,
    failures_by_kind: BTreeMap<String, u32>,
    recoveries: u32,
    consecutive_failures: u32,
    last_step_failed: bool,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Start a recorder (elapsed time counts from now).
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            steps: 0,
            decisions: 0,
            actions: 0,
            actions_by_kind: BTreeMap::new(),
            runtime_ops: 0,
            input_actions: 0,
            gpu_readbacks: 0,
            images_sent: 0,
            visual_tokens: 0,
            latencies_ms: Vec::new(),
            failures: 0,
            failures_by_kind: BTreeMap::new(),
            recoveries: 0,
            consecutive_failures: 0,
            last_step_failed: false,
        }
    }

    /// Record the start of a loop step.
    pub fn record_step(&mut self) {
        self.steps += 1;
    }

    /// Record a provider decision (kind + decision latency).
    pub fn record_decision(&mut self, kind: ActionKind, latency_ms: u64) {
        tracing::trace!(kind = ?kind, latency_ms, "provider decision recorded");
        self.decisions += 1;
        self.latencies_ms.push(latency_ms);
    }

    /// Record one executed action.
    ///
    /// `readback` marks operations that returned an image; `image` contributes to
    /// `images_sent`/`visual_tokens` when the loop embedded it in a context.
    /// [`ActionKind::Finish`] is a completion marker, not an action, and is
    /// ignored.
    pub fn record_action(
        &mut self,
        kind: ActionKind,
        readback: bool,
        image: Option<&ImagePayload>,
    ) {
        if kind == ActionKind::Finish {
            return;
        }
        self.actions += 1;
        *self.actions_by_kind.entry(kind).or_insert(0) += 1;
        if is_input_kind(kind) {
            self.input_actions += 1;
        } else {
            self.runtime_ops += 1;
        }
        if readback {
            self.gpu_readbacks += 1;
        }
        if let Some(image) = image {
            self.record_image_sent(image);
        }
    }

    /// Record that `image` was embedded in a provider context.
    pub fn record_image_sent(&mut self, image: &ImagePayload) {
        self.images_sent += 1;
        self.visual_tokens += estimate_visual_tokens(image.width, image.height);
    }

    /// Record a failed step and its error.
    ///
    /// The failure is keyed by [`Error::kind_key`], the single source of truth
    /// for [`MetricsReport::failures_by_kind`] keys.
    pub fn record_failure(&mut self, kind: ActionKind, error: &Error) {
        tracing::debug!(action = ?kind, error = %error, "step failed");
        self.failures += 1;
        self.consecutive_failures += 1;
        self.last_step_failed = true;
        *self.failures_by_kind.entry(error.kind_key()).or_insert(0) += 1;
    }

    /// Record a successful step (used to derive `recoveries`).
    pub fn record_success(&mut self) {
        if self.last_step_failed {
            self.recoveries += 1;
        }
        self.consecutive_failures = 0;
        self.last_step_failed = false;
    }

    /// Consecutive failures recorded so far (the loop's failure budget).
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Snapshot the run into a serializable report.
    pub fn report(&self, task: &str, success: bool, stop_reason: StopReason) -> MetricsReport {
        let steps = self.steps;
        let failures = self.failures;
        MetricsReport {
            task: task.to_string(),
            success,
            stop_reason,
            steps,
            decisions: self.decisions,
            actions: self.actions,
            actions_by_kind: self.actions_by_kind.clone(),
            runtime_ops: self.runtime_ops,
            input_actions: self.input_actions,
            gpu_readbacks: self.gpu_readbacks,
            images_sent: self.images_sent,
            visual_tokens: self.visual_tokens,
            decision_latency: latency_stats(&self.latencies_ms),
            failures,
            failures_by_kind: self.failures_by_kind.clone(),
            recoveries: self.recoveries,
            failure_rate: if steps == 0 {
                0.0
            } else {
                f64::from(failures) / f64::from(steps)
            },
            recovery_rate: if failures == 0 {
                0.0
            } else {
                f64::from(self.recoveries) / f64::from(failures)
            },
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// True for action kinds delivered through the Wayland seat (application input).
///
/// Every other kind (including [`ActionKind::Finish`], which
/// [`Metrics::record_action`] ignores) is runtime-native.
fn is_input_kind(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::Click | ActionKind::TypeText | ActionKind::Keypress | ActionKind::Scroll
    )
}

/// Serializable snapshot of one task run; written by `--report`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsReport {
    /// Task goal.
    pub task: String,
    /// Whether the task finished successfully.
    pub success: bool,
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// Loop iterations executed.
    pub steps: u32,
    /// Provider decisions received (including `Finish`).
    pub decisions: u32,
    /// Executed decisions excluding `Finish`.
    pub actions: u32,
    /// Per-kind action counts.
    pub actions_by_kind: BTreeMap<ActionKind, u32>,
    /// Runtime-native operations among `actions`.
    pub runtime_ops: u32,
    /// Seat input actions among `actions`.
    pub input_actions: u32,
    /// AGP calls that returned an image.
    pub gpu_readbacks: u32,
    /// Images embedded in provider contexts.
    pub images_sent: u32,
    /// Estimated visual tokens across all images sent.
    pub visual_tokens: u64,
    /// Decision latency statistics.
    pub decision_latency: LatencyStats,
    /// Failed steps.
    pub failures: u32,
    /// Failures keyed by [`Error::kind_key`].
    pub failures_by_kind: BTreeMap<String, u32>,
    /// Failed steps recovered by a later successful step.
    pub recoveries: u32,
    /// `failures / steps`.
    pub failure_rate: f64,
    /// `recoveries / failures`.
    pub recovery_rate: f64,
    /// Wall-clock duration of the run.
    pub elapsed_ms: u64,
}
