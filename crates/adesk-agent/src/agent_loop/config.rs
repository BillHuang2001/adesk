//! Loop tuning knobs.

use crate::context::ContextBudget;

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
    ///
    /// A hard gate sits on top of it: pixels are only ever requested from the
    /// runtime when the provider also reports [`supports_images`].
    ///
    /// [`supports_images`]: crate::provider::LlmProvider::supports_images
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
