//! AGP §5.4 — temporal observation E2E suite (`docs/protocol.md` §5.4).
//!
//! One real runtime per test ([`TestRuntime`]: pixman, 1280x720, empty app-dir
//! list, no Wayland clients) driven by the typed [`adesk_client::Client`]. The
//! two wire-shape cases use the harness's `RawClient`, because the SDK
//! deliberately cannot express a `wait_for_*` that asks for pixels and cannot
//! show what the server actually put on the wire.
//!
//! | Test | Pins |
//! |---|---|
//! | `wait_for_change_times_out_without_error` | an expired wait is an `Observation` (`timed_out: true`), never an AGP error, and every accumulator field is empty |
//! | `wait_for_quiet_times_out_when_timeout_is_shorter_than_quiet_window` | `timeout_ms` still bounds a `quiet_ms` wait |
//! | `wait_for_quiet_reports_quiet_after_the_quiet_window` | the quiet condition resolves with `quiet: true` / `timed_out: false` |
//! | `observe_until_timeout_reports_the_horizon` | `until = timeout` samples the horizon (see note below) |
//! | `observe_until_change_times_out_on_a_quiet_runtime` | `until = change` on an idle runtime expires |
//! | `wait_for_change_with_since_commit_far_ahead_times_out` | the `since_commit` filter drops every commit |
//! | `unknown_after_action_is_invalid_request` | observer `UnknownAction` → AGP `invalid_request`, and the connection survives |
//! | `waits_never_attach_an_image_on_the_wire` | `wait_for_change`/`wait_for_quiet` default `include_image = false` |
//! | `observation_reports_the_watermark_fields` | the §4 `Observation` JSON shape (`seq`/`last_commit_seq` numbers, nullable flags) |
//!
//! ## Note: `until = timeout` reports `timed_out: false`
//!
//! §5.4 defines `timeout` as "waits the full `timeout_ms` and reports what
//! accumulated". The observer owns that semantics and documents reaching the
//! horizon as the condition itself, so `Condition::Timeout` resolves with
//! `timed_out: false` (`crates/adesk-observer/src/service.rs`, `observe` doc and
//! the acceptance test `observe_timeout_condition_never_reports_timed_out`;
//! `crates/adesk-observer/CONTEXT.md`, "`timed_out` is true only when the
//! deadline expired before the condition was met"). The server passes the
//! observation through unchanged (`crates/adesk-server/src/dispatch/capture.rs`,
//! `observe`), so this suite pins the composed behaviour.
//!
//! ## Timing
//!
//! Protocol-level timeouts are asserted on the returned `Observation`
//! (`timed_out`), never on wall-clock timing. The only time bounds are the
//! harness's generous outer `REQUEST_TIMEOUT` and `elapsed_ms <= 60_000`.

mod common;

use adesk_client::{ObserveRequest, WaitForChangeRequest, WaitForQuietRequest};
use adesk_core::{ActionId, ErrorCode};
use common::{assert_error_code, expect_ok, TestRuntime, REQUEST_TIMEOUT};
use serde_json::{json, Value};

/// An `after_action` id no runtime can have recorded (action ids start at 1 and
/// this runtime recorded none).
const UNKNOWN_ACTION: ActionId = ActionId(999_999);

#[test]
fn wait_for_change_times_out_without_error() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let observation = expect_ok(
        runtime.block_on_timeout(
            client.wait_for_change(WaitForChangeRequest::default().timeout_ms(150)),
        ),
        "wait_for_change on an idle runtime",
    );

    assert!(
        observation.timed_out,
        "no window exists and nothing commits: the wait must expire, got {observation:?}"
    );
    assert!(
        observation.window_id.is_none(),
        "an unscoped wait must not invent a window: {observation:?}"
    );
    assert_eq!(
        observation.commits, 0,
        "nothing committed while the wait was pending: {observation:?}"
    );
    assert!(
        observation.new_windows.is_empty(),
        "no window was created: {observation:?}"
    );
    assert!(
        observation.destroyed_windows.is_empty(),
        "no window was destroyed: {observation:?}"
    );
    assert!(
        observation.changed_regions.is_empty(),
        "no damage was observed: {observation:?}"
    );
    assert!(
        observation.elapsed_ms <= 60_000,
        "elapsed_ms is monotonic runtime milliseconds: {observation:?}"
    );
}

#[test]
fn wait_for_quiet_times_out_when_timeout_is_shorter_than_quiet_window() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    // `WaitForQuietRequest::default()` carries the protocol default `quiet_ms = 250`.
    let observation = expect_ok(
        runtime.block_on_timeout(
            client.wait_for_quiet(WaitForQuietRequest::default().timeout_ms(150)),
        ),
        "wait_for_quiet with timeout_ms (150) < quiet_ms (250)",
    );

    assert!(
        observation.timed_out,
        "the quiet window (250 ms) outlasts the timeout (150 ms), so the wait \
         expires as an observation rather than an error: {observation:?}"
    );
    assert!(
        observation.window_id.is_none(),
        "an unscoped wait must not invent a window: {observation:?}"
    );
}

#[test]
fn wait_for_quiet_reports_quiet_after_the_quiet_window() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let observation = expect_ok(
        runtime.block_on_timeout(
            client.wait_for_quiet(
                WaitForQuietRequest::default()
                    .quiet_ms(50)
                    .timeout_ms(5_000),
            ),
        ),
        "wait_for_quiet with a 50 ms quiet window inside a 5 s bound",
    );

    assert!(
        !observation.timed_out,
        "the condition was met well inside the bound: {observation:?}"
    );
    assert!(
        observation.quiet,
        "the wait resolved because nothing counted as a commit for 50 ms: {observation:?}"
    );
    assert!(
        observation.window_id.is_none(),
        "an unscoped wait must not invent a window: {observation:?}"
    );
    assert_eq!(
        observation.commits, 0,
        "a runtime without windows cannot count commits: {observation:?}"
    );
}

#[test]
fn observe_until_timeout_reports_the_horizon() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let result = expect_ok(
        runtime.block_on_timeout(
            client.observe(
                ObserveRequest::timeout()
                    .timeout_ms(150)
                    .include_image(false),
            ),
        ),
        "observe(until = timeout) on an idle runtime",
    );

    // See the module note: reaching the horizon *is* the condition for
    // `Condition::Timeout`, so the observer resolves with `timed_out: false`.
    assert!(
        !result.observation.timed_out,
        "`until = timeout` reaches its horizon by definition: {:?}",
        result.observation
    );
    assert!(
        result.image.is_none(),
        "include_image(false) must not attach pixels: {:?}",
        result.image
    );
    assert!(
        result.observation.elapsed_ms <= 60_000,
        "elapsed_ms is monotonic runtime milliseconds: {:?}",
        result.observation
    );
}

#[test]
fn observe_until_change_times_out_on_a_quiet_runtime() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let result = expect_ok(
        runtime.block_on_timeout(
            client.observe(
                ObserveRequest::change()
                    .timeout_ms(150)
                    .include_image(false),
            ),
        ),
        "observe(until = change) on an idle runtime",
    );

    assert!(
        result.observation.timed_out,
        "nothing changes on an idle runtime, so the wait expires: {:?}",
        result.observation
    );
    assert!(
        result.image.is_none(),
        "include_image(false) must not attach pixels: {:?}",
        result.image
    );
}

#[test]
fn wait_for_change_with_since_commit_far_ahead_times_out() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let observation = expect_ok(
        runtime.block_on_timeout(
            client.wait_for_change(
                WaitForChangeRequest::default()
                    .since_commit(1_000_000)
                    .timeout_ms(150),
            ),
        ),
        "wait_for_change with since_commit far beyond any commit seq",
    );

    assert!(
        observation.timed_out,
        "only commits with commit_seq > 1_000_000 count, and none exist: {observation:?}"
    );
    assert_eq!(
        observation.commits, 0,
        "the filter must drop every commit: {observation:?}"
    );
}

#[test]
fn unknown_after_action_is_invalid_request() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    assert_error_code(
        runtime.block_on_timeout(
            client.wait_for_quiet(
                WaitForQuietRequest::default()
                    .after_action(UNKNOWN_ACTION)
                    .timeout_ms(150),
            ),
        ),
        ErrorCode::InvalidRequest,
        "wait_for_quiet with an unknown after_action",
    );
    assert_error_code(
        runtime.block_on_timeout(
            client.observe(
                ObserveRequest::timeout()
                    .after_action(UNKNOWN_ACTION)
                    .timeout_ms(150)
                    .include_image(false),
            ),
        ),
        ErrorCode::InvalidRequest,
        "observe with an unknown after_action",
    );

    // A request error must not close the connection (§6).
    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping after two invalid_request answers on the same connection",
    );
    assert_eq!(
        ping.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "the connection survived the error responses"
    );
}

#[test]
fn waits_never_attach_an_image_on_the_wire() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_raw();

    runtime.block_on_timeout(async {
        raw.send_json(&json!({
            "id": 1,
            "method": "wait_for_change",
            "params": {"timeout_ms": 150},
        }))
        .await;
        let response = raw
            .expect_json_matching(REQUEST_TIMEOUT, |frame| frame["id"].as_u64() == Some(1))
            .await;
        assert_no_wire_image(&response, "wait_for_change");

        raw.send_json(&json!({
            "id": 2,
            "method": "wait_for_quiet",
            "params": {"timeout_ms": 150},
        }))
        .await;
        let response = raw
            .expect_json_matching(REQUEST_TIMEOUT, |frame| frame["id"].as_u64() == Some(2))
            .await;
        assert_no_wire_image(&response, "wait_for_quiet");
    });
}

#[test]
fn observation_reports_the_watermark_fields() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_raw();

    runtime.block_on_timeout(async {
        raw.send_json(&json!({
            "id": 1,
            "method": "wait_for_quiet",
            "params": {"quiet_ms": 50, "timeout_ms": 5_000},
        }))
        .await;
        let response = raw
            .expect_json_matching(REQUEST_TIMEOUT, |frame| frame["id"].as_u64() == Some(1))
            .await;
        assert!(
            response.get("error").is_none(),
            "wait_for_quiet answered with an error frame: {response}"
        );

        let observation = &response["result"]["observation"];
        assert!(
            observation.is_object(),
            "result.observation is missing: {response}"
        );
        assert!(
            observation["seq"].is_u64(),
            "`seq` is the global watermark (u64): {response}"
        );
        assert!(
            observation["last_commit_seq"].is_u64(),
            "`last_commit_seq` is a commit watermark (u64): {response}"
        );
        assert_eq!(
            observation["commits"].as_u64(),
            Some(0),
            "no window exists, so nothing committed: {response}"
        );
        assert!(
            observation["after_action"].is_null(),
            "`after_action` is null when the request omitted it: {response}"
        );
        assert!(
            observation["focus_changed"].is_null() || observation["focus_changed"].is_boolean(),
            "§4 allows `focus_changed` as false or null: {response}"
        );
        assert!(
            observation["title_changed"].is_boolean(),
            "`title_changed` is a bool: {response}"
        );
    });
}

/// Asserts that a `wait_for_*` response carries no image.
///
/// §5.4 defaults `include_image` to `false` for both waits, and the §4
/// `observation` object always carries the field — as `null` when absent — so
/// both spellings are accepted.
///
/// # Panics
///
/// Panics with the full response if it is an error frame, if
/// `result.observation` is missing, or if an image payload is attached.
fn assert_no_wire_image(response: &Value, method: &str) {
    assert!(
        response.get("error").is_none(),
        "{method}: answered with an error frame: {response}"
    );
    let observation = &response["result"]["observation"];
    assert!(
        observation.is_object(),
        "{method}: result.observation is missing: {response}"
    );
    let image = observation.get("image");
    assert!(
        matches!(image, None | Some(Value::Null)),
        "{method}: a wait must not attach pixels (include_image defaults to \
         false): {response}"
    );
}
