//! Metrics accounting semantics.
//!
//! Run with: `cargo test -p adesk-agent`
//!
//! These tests pin the documented semantics: `actions` excludes `Finish`,
//! `gpu_readbacks` counts only calls that returned pixels, `visual_tokens` is the
//! documented estimate, latency is min/mean/p95, and the rates are ratios over
//! steps and failures respectively.

use adesk_agent::{
    estimate_visual_tokens, latency_stats, ActionKind, Error, LatencyStats, LoopConfig, Metrics,
    ProviderError, StopReason,
};
use adesk_core::WindowId;
use adesk_proto::{ImageFormat, ImagePayload};

/// A payload carrying only dimensions: the recorder never inspects pixels.
fn image(width: u32, height: u32) -> ImagePayload {
    ImagePayload {
        width,
        height,
        format: ImageFormat::Png,
        stride: None,
        data: String::new(),
        scale: 1.0,
    }
}

/// A cheap, deterministic failure for the failure/recovery specs.
fn transport_error() -> Error {
    Error::Transport("socket closed".to_string())
}

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
fn actions_are_counted_per_kind() {
    let mut metrics = Metrics::new();
    for kind in [
        ActionKind::ListApps,
        ActionKind::ListWindows,
        ActionKind::GetWindow,
        ActionKind::LaunchApp,
        ActionKind::ActivateWindow,
        ActionKind::CloseWindow,
    ] {
        metrics.record_action(kind, false, None);
    }
    metrics.record_action(ActionKind::Capture, true, None);
    metrics.record_action(ActionKind::Observe, true, None);
    metrics.record_action(ActionKind::Wait, false, None);
    metrics.record_action(ActionKind::Click, false, None);
    metrics.record_action(ActionKind::Click, false, None);
    metrics.record_action(ActionKind::TypeText, false, None);
    metrics.record_action(ActionKind::Keypress, false, None);
    metrics.record_action(ActionKind::Scroll, false, None);
    // The completion marker is a decision, never an action.
    metrics.record_action(ActionKind::Finish, false, None);
    metrics.record_decision(ActionKind::Finish, 12);

    let report = metrics.report("count kinds", true, StopReason::Finished);
    assert_eq!(report.actions, 14);
    assert_eq!(report.decisions, 1);
    assert_eq!(report.actions_by_kind.get(&ActionKind::Click), Some(&2));
    assert_eq!(report.actions_by_kind.get(&ActionKind::Capture), Some(&1));
    assert_eq!(report.actions_by_kind.get(&ActionKind::Finish), None);
    assert_eq!(report.actions_by_kind.len(), 13);
    assert_eq!(report.actions_by_kind.values().sum::<u32>(), report.actions);
    assert_eq!(report.runtime_ops, 9);
    assert_eq!(report.input_actions, 5);
    assert_eq!(report.runtime_ops + report.input_actions, report.actions);
    assert_eq!(report.gpu_readbacks, 2);
    assert_eq!(report.decision_latency.count, 1);
    assert_eq!(report.decision_latency.p95_ms, 12);
}

/// `gpu_readbacks` increments only for calls that returned an `ImagePayload`
/// (`capture`, `observe` with `include_image`) — not for list/activate/wait.
#[test]
fn readbacks_count_only_pixel_returning_calls() {
    let mut metrics = Metrics::new();
    metrics.record_action(ActionKind::ListWindows, false, None);
    metrics.record_action(ActionKind::ActivateWindow, false, None);
    metrics.record_action(ActionKind::Wait, false, None);
    assert_eq!(
        metrics
            .report("no pixels", false, StopReason::StepBudgetExhausted)
            .gpu_readbacks,
        0
    );

    // `capture` returns pixels; `observe` only does when the context embedded
    // the image it returned.
    metrics.record_action(ActionKind::Capture, true, Some(&image(64, 64)));
    metrics.record_action(ActionKind::Observe, true, None);
    metrics.record_action(ActionKind::Observe, false, None);

    let report = metrics.report("pixels", true, StopReason::Finished);
    assert_eq!(report.actions, 6);
    assert_eq!(report.gpu_readbacks, 2);
    // Only the capture's payload was embedded in a provider context.
    assert_eq!(report.images_sent, 1);
    assert_eq!(report.visual_tokens, estimate_visual_tokens(64, 64));
}

/// `images_sent`/`visual_tokens` accumulate once per image embedded in a context
/// (current + keyframe), using `estimate_visual_tokens`.
#[test]
fn visual_tokens_track_embedded_images() {
    let mut metrics = Metrics::new();
    let current = image(1280, 800);
    let keyframe = image(640, 480);
    metrics.record_image_sent(&current);
    metrics.record_image_sent(&keyframe);

    let report = metrics.report("two images", true, StopReason::Finished);
    assert_eq!(report.images_sent, 2);
    assert_eq!(
        report.visual_tokens,
        estimate_visual_tokens(1280, 800) + estimate_visual_tokens(640, 480)
    );
    // 1280x800 = 3x2 tiles, 640x480 = 2x1 tiles.
    assert_eq!(report.visual_tokens, 85 + 170 * 6 + 85 + 170 * 2);
    assert_eq!(report.visual_tokens, 1_530);

    // Re-embedding the retained frame is another send: the context never grows
    // a history, it re-sends the current + keyframe pair.
    metrics.record_image_sent(&current);
    let report = metrics.report("three images", true, StopReason::Finished);
    assert_eq!(report.images_sent, 3);
    assert_eq!(report.visual_tokens, 1_530 + 85 + 170 * 6);
}

/// `latency_stats` reports min, mean and nearest-rank p95 over the samples.
///
/// Nearest rank picks the `ceil(0.95 * n)`-th sample without interpolating, so
/// ten samples ending in `100` have p95 `100`, while a set ending in `95` has
/// p95 `95`. Empty input yields zeros.
#[test]
fn latency_stats_are_min_mean_p95() {
    let samples: Vec<u64> = (1..=10).map(|step| step * 10).collect();
    let stats = latency_stats(&samples);
    assert_eq!(stats.count, 10);
    assert_eq!(stats.min_ms, 10);
    assert_eq!(stats.mean_ms, 55.0);
    assert_eq!(stats.p95_ms, 100);

    // The same rank lands on 95 when the largest sample is 95.
    let mut samples: Vec<u64> = (1..=9).map(|step| step * 10).collect();
    samples.push(95);
    let stats = latency_stats(&samples);
    assert_eq!(stats.count, 10);
    assert_eq!(stats.min_ms, 10);
    assert_eq!(stats.p95_ms, 95);

    // Nearest rank rounds up: 20 samples -> rank 19, not 19.0 interpolated.
    let samples: Vec<u64> = (1..=20).collect();
    let stats = latency_stats(&samples);
    assert_eq!(stats.count, 20);
    assert_eq!(stats.mean_ms, 10.5);
    assert_eq!(stats.p95_ms, 19);

    // Unsorted input, one sample, and the empty case.
    let stats = latency_stats(&[30, 10, 20]);
    assert_eq!(stats.count, 3);
    assert_eq!(stats.min_ms, 10);
    assert_eq!(stats.mean_ms, 20.0);
    assert_eq!(stats.p95_ms, 30);

    let stats = latency_stats(&[5]);
    assert_eq!(stats.count, 1);
    assert_eq!(stats.min_ms, 5);
    assert_eq!(stats.mean_ms, 5.0);
    assert_eq!(stats.p95_ms, 5);

    assert_eq!(latency_stats(&[]), LatencyStats::default());
    assert_eq!(latency_stats(&[]).count, 0);
}

/// `failure_rate == failures / steps` and `recovery_rate == recoveries /
/// failures`, both 0 for empty denominators.
#[test]
fn failure_and_recovery_rates() {
    let mut metrics = Metrics::new();
    let empty = metrics.report("empty", false, StopReason::FatalError);
    assert_eq!(empty.steps, 0);
    assert_eq!(empty.failures, 0);
    assert_eq!(empty.failure_rate, 0.0);
    assert_eq!(empty.recovery_rate, 0.0);

    for _ in 0..4 {
        metrics.record_step();
    }
    metrics.record_failure(ActionKind::Click, &transport_error());
    metrics.record_failure(ActionKind::Click, &transport_error());
    metrics.record_success();
    metrics.record_success();

    let report = metrics.report("rates", true, StopReason::Finished);
    assert_eq!(report.steps, 4);
    assert_eq!(report.failures, 2);
    assert_eq!(report.failure_rate, 0.5);
    assert_eq!(report.recoveries, 1);
    assert_eq!(report.recovery_rate, 0.5);
    assert_eq!(report.failures_by_kind.get("transport"), Some(&2));
    assert_eq!(
        report.failures_by_kind.values().sum::<u32>(),
        report.failures
    );

    // A clean run never divides by zero: both rates stay 0.
    let mut clean = Metrics::new();
    clean.record_step();
    clean.record_success();
    let report = clean.report("clean", true, StopReason::Finished);
    assert_eq!(report.failures, 0);
    assert_eq!(report.failure_rate, 0.0);
    assert_eq!(report.recovery_rate, 0.0);

    // Failures are keyed by the error, not by the action that produced them.
    let mut keyed = Metrics::new();
    keyed.record_failure(
        ActionKind::GetWindow,
        &Error::Client(adesk_core::Error::unknown_window(WindowId(7))),
    );
    keyed.record_failure(
        ActionKind::TypeText,
        &Error::Provider(ProviderError::Timeout(5_000)),
    );
    let report = keyed.report("keys", false, StopReason::FatalError);
    assert_eq!(report.failures, 2);
    assert_eq!(report.failures_by_kind.get("unknown_window"), Some(&1));
    assert_eq!(report.failures_by_kind.get("provider_timeout"), Some(&1));
}

/// Consecutive failures increment `consecutive_failures`; a success resets it and
/// counts a recovery.
#[test]
fn consecutive_failures_track_recovery() {
    let mut metrics = Metrics::new();
    assert_eq!(metrics.consecutive_failures(), 0);

    metrics.record_failure(ActionKind::Capture, &transport_error());
    assert_eq!(metrics.consecutive_failures(), 1);
    metrics.record_failure(ActionKind::Capture, &transport_error());
    assert_eq!(metrics.consecutive_failures(), 2);

    metrics.record_success();
    assert_eq!(metrics.consecutive_failures(), 0);
    let report = metrics.report("recovered", true, StopReason::Finished);
    assert_eq!(report.failures, 2);
    assert_eq!(report.recoveries, 1);
    assert_eq!(report.recovery_rate, 0.5);

    // A success that does not follow a failure is not a recovery.
    metrics.record_success();
    assert_eq!(
        metrics
            .report("still one", true, StopReason::Finished)
            .recoveries,
        1
    );

    // Each failed -> successful transition counts once.
    let mut alternating = Metrics::new();
    for _ in 0..2 {
        alternating.record_failure(ActionKind::Scroll, &transport_error());
        alternating.record_success();
    }
    assert_eq!(alternating.consecutive_failures(), 0);
    let report = alternating.report("alternating", true, StopReason::Finished);
    assert_eq!(report.failures, 2);
    assert_eq!(report.recoveries, 2);
    assert_eq!(report.recovery_rate, 1.0);
}

/// The visual-token estimate is monotonic in area and never zero for a real
/// image, and stays in the documented `85 + 170 * tiles` ballpark.
#[test]
fn visual_token_estimate_is_sane() {
    // Documented shape: 85 + 170 * ceil(w/512) * ceil(h/512).
    assert_eq!(estimate_visual_tokens(1, 1), 85 + 170);
    assert_eq!(estimate_visual_tokens(512, 512), 85 + 170);
    assert_eq!(estimate_visual_tokens(513, 512), 85 + 2 * 170);
    assert_eq!(estimate_visual_tokens(1280, 800), 85 + 170 * 6);
    assert_eq!(estimate_visual_tokens(1920, 1080), 85 + 170 * 12);

    // The documented 1280x800 case: three columns by two rows.
    let tokens = estimate_visual_tokens(1280, 800);
    assert_eq!(tokens, 1_105);
    assert!((255..=2_125).contains(&tokens));

    // Monotonic in area, never zero for a real image.
    assert!(estimate_visual_tokens(640, 480) < estimate_visual_tokens(1280, 960));
    assert!(estimate_visual_tokens(1024, 768) >= estimate_visual_tokens(512, 512));
    assert!(estimate_visual_tokens(1, 1) > 0);
    assert!(estimate_visual_tokens(0, 0) > 0);
}

/// `estimate_visual_tokens`/`latency_stats` are pure and callable without a
/// runtime; referenced here so the API surface stays exercised.
#[test]
fn metrics_helpers_are_public() {
    let _ = estimate_visual_tokens as fn(u32, u32) -> u64;
    let _ = latency_stats as fn(&[u64]) -> adesk_agent::LatencyStats;
}
