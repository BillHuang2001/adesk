//! Metrics accounting semantics.
//!
//! Run with: `cargo test -p adesk-agent`
//!
//! These tests pin the documented semantics: `actions` excludes `Finish`,
//! `gpu_readbacks` counts only calls that returned pixels, `visual_tokens` is the
//! documented estimate, latency is min/mean/p95, and the rates are ratios over
//! steps and failures respectively.

use adesk_agent::{estimate_visual_tokens, latency_stats, LoopConfig, Metrics, StopReason};

/// `LoopConfig::default()` and `StopReason` are the documented values.
#[test]
fn loop_config_defaults_match_documentation() {
    let config = LoopConfig::default();
    assert_eq!(config.max_steps, 20);
    assert_eq!(config.step_timeout_ms, 30_000);
    assert!(config.observe_after_input);
    assert_eq!(config.quiet_ms, 250);
    assert_eq!(config.observe_timeout_ms, 5_000);
    assert!(config.include_image);
    assert_eq!(config.capture_max_dimension, Some(1024));
    assert_eq!(config.max_consecutive_failures, 3);
    assert_eq!(config.retries_per_step, 2);
    assert_eq!(config.retry_backoff_ms, 200);
    assert!(config.validate_protocol_version);
    let metrics = Metrics::new();
    assert_eq!(metrics.consecutive_failures(), 0);
    let _ = StopReason::Finished;
}

/// `actions` counts executed decisions except `Finish`; `actions_by_kind` splits
/// them and `runtime_ops`/`input_actions` partition them.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn actions_are_counted_per_kind() {
    todo!("phase 2: record mixed actions, assert counts")
}

/// `gpu_readbacks` increments only for calls that returned an `ImagePayload`
/// (`capture`, `observe` with `include_image`) — not for list/activate/wait.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn readbacks_count_only_pixel_returning_calls() {
    todo!("phase 2: record actions with/without images")
}

/// `images_sent`/`visual_tokens` accumulate once per image embedded in a context
/// (current + keyframe), using `estimate_visual_tokens`.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn visual_tokens_track_embedded_images() {
    todo!("phase 2: two images per step, assert tokens = sum of estimates")
}

/// `latency_stats` reports min, mean and nearest-rank p95 over the samples.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn latency_stats_are_min_mean_p95() {
    todo!("phase 2: samples [10,20,30,...,100] -> min 10, p95 95")
}

/// `failure_rate == failures / steps` and `recovery_rate == recoveries /
/// failures`, both 0 for empty denominators.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn failure_and_recovery_rates() {
    todo!("phase 2: assert ratios and zero-denominator behavior")
}

/// Consecutive failures increment `consecutive_failures`; a success resets it and
/// counts a recovery.
#[test]
#[ignore = "phase 2: metrics not implemented"]
fn consecutive_failures_track_recovery() {
    todo!("phase 2: fail fail success -> recoveries == 1, consecutive == 0")
}

/// The visual-token estimate is monotonic in area and never zero for a real
/// image, and stays in the documented `85 + 170 * tiles` ballpark.
#[test]
#[ignore = "phase 2: estimator not implemented"]
fn visual_token_estimate_is_sane() {
    todo!("phase 2: assert estimate_visual_tokens(1280,800) bounds")
}

/// `estimate_visual_tokens`/`latency_stats` are pure and callable without a
/// runtime; referenced here so the API surface stays exercised.
#[test]
fn metrics_helpers_are_public() {
    let _ = estimate_visual_tokens as fn(u32, u32) -> u64;
    let _ = latency_stats as fn(&[u64]) -> adesk_agent::LatencyStats;
}
