//! AGP transport and dispatch protocol suite (`docs/protocol.md` §1, §5, §6).
//!
//! One real `adesk-server` runtime per test (see [`common`]), driven through the
//! typed `adesk-client` SDK and, where the SDK deliberately cannot express the
//! case, through the raw NDJSON [`common::RawClient`].
//!
//! Coverage:
//!
//! - `ping` identity: protocol/runtime version, renderer, output size, and
//!   monotonically non-decreasing `uptime_ms` (scenario 1).
//! - every §5 method answers exactly once with the protocol-mandated outcome;
//!   mismatches are aggregated so one run reports every mismatching method
//!   (scenario 2).
//! - an unknown method answers `unknown_method` and keeps the connection open
//!   (§1, scenario 3).
//! - a request whose `params` fail validation answers `invalid_request` and
//!   keeps the connection open (§6, scenario 3b).
//! - an AGP error response does not close the connection (§6, scenarios 4/5).
//! - a line that identifies no request — not JSON at all, JSON that is not an
//!   object, or a request-shaped object without a `u64` id — closes only the
//!   offending connection (§1/§6, scenario 6).
//! - concurrent (scenario 7) and pipelined (scenario 8) requests on one
//!   connection all resolve with exactly one response each (§1).
//! - blank lines are ignored (§1, scenario 9).
//!
//! Note on `observe(until = timeout)`: sampling the full `timeout_ms` *is* the
//! condition (§5.4, animation sampling), so the observation reports
//! `timed_out: false` with the full `elapsed_ms` — the domain vocabulary defines
//! `timed_out` as "the wait expired *before* the condition was met"
//! (`adesk-core::Observation::timed_out`) and `adesk-observer` pins exactly that
//! (`src/spec.rs:190-194`, `src/service.rs:716-721`). This suite asserts that
//! documented semantics, not an expiry.

mod common;

use std::time::Duration;

use adesk_client::{
    CaptureRegionRequest, CaptureRequest, ClickRequest, ClientError, DragRequest, EventFilter,
    InspectCaptureRequest, InspectSubscribeRequest, ObserveRequest, ObserveResult,
    PointerButtonRequest, Renderer, ScrollRequest, WaitForChangeRequest, WaitForQuietRequest,
};
use adesk_core::{AppId, ErrorCode, Observation, OverlayKind, Position, Rect, WindowId};
use common::{
    assert_error_code, expect_ok, output_size, TestRuntime, OUTPUT_HEIGHT, OUTPUT_WIDTH,
    REQUEST_TIMEOUT,
};
use serde_json::json;

/// Upper bound for `ping.uptime_ms`: a test runtime is seconds old, never ten
/// minutes old. Generous so a loaded CI machine cannot make it flaky.
const UPTIME_CEILING_MS: u64 = 10 * 60 * 1000;

/// Aggregates per-method expectations so scenario 2 reports **every**
/// mismatching method in one run instead of failing on the first one.
#[derive(Default)]
struct Sweep {
    /// One human-readable line per method that did not answer as required.
    mismatches: Vec<String>,
}

impl Sweep {
    /// Records a method expected to succeed; `check` reports a mismatch as
    /// `Err(detail)`.
    fn ok<T>(
        &mut self,
        method: &str,
        expected: &str,
        result: Result<T, ClientError>,
        check: impl FnOnce(&T) -> Result<(), String>,
    ) {
        match result {
            Ok(value) => {
                if let Err(detail) = check(&value) {
                    self.mismatches
                        .push(format!("{method}: expected {expected}, got Ok({detail})"));
                }
            }
            Err(error) => self
                .mismatches
                .push(format!("{method}: expected {expected}, got {error:?}")),
        }
    }

    /// Records a method expected to fail with the AGP `expected` code (§6).
    fn err<T: std::fmt::Debug>(
        &mut self,
        method: &str,
        expected: ErrorCode,
        result: Result<T, ClientError>,
    ) {
        match result {
            Err(ClientError::Server { code, .. }) if code == expected => {}
            Err(ClientError::Server { code, message }) => self.mismatches.push(format!(
                "{method}: expected AGP error `{}`, got `{code}` ({message})",
                expected.as_str()
            )),
            Err(other) => self.mismatches.push(format!(
                "{method}: expected AGP error `{}`, got {other:?}",
                expected.as_str()
            )),
            Ok(value) => self.mismatches.push(format!(
                "{method}: expected AGP error `{}`, got Ok({value:?})",
                expected.as_str()
            )),
        }
    }
}

/// The observation outcome a timed-out wait must report (§5.4).
fn assert_timed_out(method: &str, observation: &Observation) -> Result<(), String> {
    if !observation.timed_out {
        return Err(format!(
            "{method} did not expire within its short `timeout_ms`: {observation:?}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 1 — ping identity
// ---------------------------------------------------------------------------

#[test]
fn ping_reports_protocol_version_runtime_version_renderer_and_output() {
    let t = TestRuntime::start();
    // `connect` performs the version handshake itself (`ping` + version check).
    let client = t.connect();

    let first = expect_ok(t.block_on_timeout(client.ping()), "first ping");
    let second = expect_ok(t.block_on_timeout(client.ping()), "second ping");

    assert_eq!(
        first.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "ping.protocol_version must be the server's PROTOCOL_VERSION"
    );
    assert_eq!(
        first.protocol_version, 1,
        "AGP v1 is protocol version 1 (docs/protocol.md §5.1)"
    );
    assert_eq!(
        first.runtime_version,
        adesk_server::RUNTIME_VERSION,
        "ping.runtime_version must be the runtime's crate version"
    );
    assert_eq!(
        first.renderer,
        Renderer::Pixman,
        "the test runtime is started with the pixman renderer"
    );
    assert_eq!(
        first.output,
        output_size(),
        "ping.output must be the virtual output size the runtime was started with"
    );
    assert_eq!(
        (first.output.w, first.output.h),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT),
        "the harness starts every runtime with a {OUTPUT_WIDTH}x{OUTPUT_HEIGHT} output"
    );
    assert!(
        first.uptime_ms < UPTIME_CEILING_MS,
        "ping.uptime_ms = {} is not a fresh runtime's uptime (< {UPTIME_CEILING_MS})",
        first.uptime_ms
    );
    assert!(
        second.uptime_ms >= first.uptime_ms,
        "uptime_ms must be monotonically non-decreasing: {} then {}",
        first.uptime_ms,
        second.uptime_ms
    );
}

// ---------------------------------------------------------------------------
// Scenario 2 — every §5 method answers exactly once
// ---------------------------------------------------------------------------

#[test]
fn every_method_answers_exactly_once() {
    let t = TestRuntime::start();
    let client = t.connect();

    // A fresh runtime has no windows and an empty app-dir list.
    let missing_window = WindowId(1);
    let missing_app = AppId::from("org.example.absent");
    let position = Position::pixels(8, 9);

    let mut sweep = Sweep::default();

    t.block_on(async {
        // §5.1 runtime
        let ping = client.ping().await;
        sweep.ok("ping", "Ok(PingInfo)", ping, |info| {
            if info.protocol_version != adesk_server::PROTOCOL_VERSION {
                return Err(format!(
                    "protocol_version {} != {}",
                    info.protocol_version,
                    adesk_server::PROTOCOL_VERSION
                ));
            }
            Ok(())
        });

        // §5.2 applications
        let apps = client.list_apps(None, false).await;
        sweep.ok("list_apps", "Ok(no apps)", apps, |apps| {
            if apps.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "{} app(s) listed by a runtime started with no app dirs",
                    apps.len()
                ))
            }
        });

        let app = client.get_app(&missing_app).await;
        sweep.err("get_app", ErrorCode::UnknownApp, app);

        let launch = client.launch_app(&missing_app, &[]).await;
        sweep.err("launch_app", ErrorCode::UnknownApp, launch);

        // §5.3 windows
        let windows = client.list_windows().await;
        sweep.ok(
            "list_windows",
            "Ok(no windows, no active window)",
            windows,
            |list| {
                if !list.windows.is_empty() {
                    return Err(format!(
                        "{} window(s) in a fresh runtime",
                        list.windows.len()
                    ));
                }
                if list.active_window_id.is_some() {
                    return Err(format!(
                        "active_window_id = {:?} with no windows",
                        list.active_window_id
                    ));
                }
                Ok(())
            },
        );

        let window = client.get_window(missing_window).await;
        sweep.err("get_window", ErrorCode::UnknownWindow, window);

        let activated = client.activate_window(missing_window).await;
        sweep.err("activate_window", ErrorCode::UnknownWindow, activated);

        let closed = client.close_window(missing_window).await;
        sweep.err("close_window", ErrorCode::UnknownWindow, closed);

        let focus = client.get_focus().await;
        sweep.ok("get_focus", "Ok(no focus)", focus, |focus| {
            if focus.window_id.is_some() {
                return Err(format!("window_id = {:?} with no windows", focus.window_id));
            }
            if focus.surface_focus {
                return Err("surface_focus = true with no surfaces".to_owned());
            }
            Ok(())
        });

        // §5.4 capture and observation
        let captured = client
            .capture_window(CaptureRequest::window(missing_window))
            .await;
        sweep.err("capture_window", ErrorCode::UnknownWindow, captured);

        let region = client
            .capture_region(CaptureRegionRequest::new(
                missing_window,
                Rect::new(0, 0, 4, 4),
            ))
            .await;
        sweep.err("capture_region", ErrorCode::UnknownWindow, region);

        let observed = client
            .observe(
                ObserveRequest::timeout()
                    .timeout_ms(150)
                    .include_image(false),
            )
            .await;
        sweep.ok(
            "observe",
            "Ok(until=timeout samples its full horizon, timed_out: false, no image)",
            observed,
            |result: &ObserveResult| {
                // `until = timeout` samples the whole `timeout_ms` and reports
                // what accumulated (protocol §5.4). Reaching that horizon *is*
                // the condition, so it is not an expiry: `timed_out` stays
                // false (`adesk-observer/src/spec.rs:190-194`,
                // `service.rs:716-721`, `adesk-core`'s `Observation::timed_out`
                // doc: "whether the wait expired *before* the condition was
                // met").
                if result.observation.timed_out {
                    return Err(format!(
                        "until=timeout must not report an expiry: {:?}",
                        result.observation
                    ));
                }
                if result.observation.elapsed_ms < 150 {
                    return Err(format!(
                        "until=timeout resolved after {} ms, before its 150 ms horizon",
                        result.observation.elapsed_ms
                    ));
                }
                if result.image.is_some() {
                    return Err("include_image(false) still returned an image".to_owned());
                }
                Ok(())
            },
        );

        let changed = client
            .wait_for_change(WaitForChangeRequest::default().timeout_ms(150))
            .await;
        sweep.ok(
            "wait_for_change",
            "Ok(timed_out: true)",
            changed,
            |observation| assert_timed_out("wait_for_change", observation),
        );

        // `quiet_ms` defaults to 250 (§5.4), so a 150 ms timeout must expire.
        let quiet = client
            .wait_for_quiet(WaitForQuietRequest::default().timeout_ms(150))
            .await;
        sweep.ok(
            "wait_for_quiet",
            "Ok(timed_out: true)",
            quiet,
            |observation| assert_timed_out("wait_for_quiet", observation),
        );

        // §5.5 input — every method targets the unknown window, so every method
        // must answer `unknown_window` before touching the seat.
        let moved = client.pointer_move(missing_window, position).await;
        sweep.err("pointer_move", ErrorCode::UnknownWindow, moved);

        let clicked = client.click(ClickRequest::window(missing_window)).await;
        sweep.err("click", ErrorCode::UnknownWindow, clicked);

        let double = client
            .double_click(PointerButtonRequest::window(missing_window))
            .await;
        sweep.err("double_click", ErrorCode::UnknownWindow, double);

        let down = client
            .mouse_down(PointerButtonRequest::window(missing_window))
            .await;
        sweep.err("mouse_down", ErrorCode::UnknownWindow, down);

        let up = client
            .mouse_up(PointerButtonRequest::window(missing_window))
            .await;
        sweep.err("mouse_up", ErrorCode::UnknownWindow, up);

        let scrolled = client
            .scroll(ScrollRequest::new(missing_window, 0.0, 10.0))
            .await;
        sweep.err("scroll", ErrorCode::UnknownWindow, scrolled);

        let dragged = client
            .drag(DragRequest::new(
                missing_window,
                position,
                Position::pixels(16, 18),
            ))
            .await;
        sweep.err("drag", ErrorCode::UnknownWindow, dragged);

        let pressed = client.keypress("a", Some(missing_window)).await;
        sweep.err("keypress", ErrorCode::UnknownWindow, pressed);

        let key_down = client.key_down("a", Some(missing_window)).await;
        sweep.err("key_down", ErrorCode::UnknownWindow, key_down);

        let key_up = client.key_up("a", Some(missing_window)).await;
        sweep.err("key_up", ErrorCode::UnknownWindow, key_up);

        let typed = client.type_text("hi", Some(missing_window)).await;
        sweep.err("type_text", ErrorCode::UnknownWindow, typed);

        // §5.6 subscriptions
        let subscribed = client.subscribe_events(EventFilter::all()).await;
        sweep.ok(
            "subscribe_events",
            "Ok(subscription_id != 0)",
            subscribed,
            |stream| {
                if stream.subscription_id() == 0 {
                    Err("subscription_id is 0; ids are server-assigned and non-zero".to_owned())
                } else {
                    Ok(())
                }
            },
        );

        let unsubscribed = client.unsubscribe_events(12_345).await;
        sweep.ok(
            "unsubscribe_events",
            "Ok(()) — unknown ids are ignored",
            unsubscribed,
            |_| Ok(()),
        );

        // §5.7 human inspector
        let inspect = client
            .inspect_capture(InspectCaptureRequest::default())
            .await;
        sweep.ok("inspect_capture", "Ok(PNG 1280x720)", inspect, |image| {
            if image.format != adesk_proto::ImageFormat::Png {
                return Err(format!("format = {:?}, expected png", image.format));
            }
            if image.width != OUTPUT_WIDTH || image.height != OUTPUT_HEIGHT {
                return Err(format!(
                    "{}x{} != {OUTPUT_WIDTH}x{OUTPUT_HEIGHT}",
                    image.width, image.height
                ));
            }
            if image.data.is_empty() {
                return Err("empty PNG payload".to_owned());
            }
            Ok(())
        });

        let inspect_stream = client
            .inspect_subscribe(InspectSubscribeRequest::new(vec![OverlayKind::WindowIds]))
            .await;
        sweep.ok(
            "inspect_subscribe",
            "Ok(subscription_id != 0)",
            inspect_stream,
            |stream| {
                if stream.subscription_id() == 0 {
                    Err("subscription_id is 0; ids are server-assigned and non-zero".to_owned())
                } else {
                    Ok(())
                }
            },
        );

        // §6: no error above may have closed the connection.
        let final_ping = client.ping().await;
        sweep.ok(
            "ping (after the other 28 methods)",
            "Ok(PingInfo) — the connection survived every error",
            final_ping,
            |_| Ok(()),
        );
    });

    assert!(
        sweep.mismatches.is_empty(),
        "{} of 29 AGP methods did not answer as docs/protocol.md §5/§6 requires:\n  - {}",
        sweep.mismatches.len(),
        sweep.mismatches.join("\n  - ")
    );
}

// ---------------------------------------------------------------------------
// Scenario 3 — unknown method
// ---------------------------------------------------------------------------

#[test]
fn unknown_method_answers_unknown_method_and_keeps_the_connection_open() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    t.block_on(async {
        raw.send_json(&json!({
            "id": 77,
            "method": "definitely_not_a_method",
            "params": {},
        }))
        .await;

        // Protocol §1: an unknown method MUST produce `unknown_method`, never a
        // dropped connection.
        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(
            response["id"],
            json!(77),
            "the error response must carry the request id"
        );
        assert_eq!(
            response["error"]["code"],
            json!("unknown_method"),
            "unknown methods must answer `unknown_method` (§1), got {response}"
        );

        // The connection must still be usable afterwards.
        raw.send_json(&json!({ "id": 78, "method": "ping", "params": {} }))
            .await;
        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(response["id"], json!(78));
        assert_eq!(
            response["result"]["protocol_version"],
            json!(1),
            "the connection must stay open after `unknown_method`, got {response}"
        );
    });
}

// ---------------------------------------------------------------------------
// Scenario 3b — request params that fail validation
// ---------------------------------------------------------------------------

#[test]
fn invalid_params_answer_invalid_request_and_keep_the_connection_open() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    t.block_on(async {
        // A well-formed request frame whose `params` do not match the method's
        // schema decodes to `ProtoError::InvalidParams`. Protocol §6: the server
        // answers every request with exactly one response frame and only framing
        // corruption may close the connection — so this must be answered
        // `invalid_request` (`adesk-proto` maps `InvalidParams` there), not
        // dropped.
        raw.send_line(r#"{"id":43,"method":"click","params":{"window_id":"not-a-number"}}"#)
            .await;

        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(
            response["id"],
            json!(43),
            "the error response must carry the request id, got {response}"
        );
        assert_eq!(
            response["error"]["code"],
            json!("invalid_request"),
            "params that fail validation must answer `invalid_request` (§6), got {response}"
        );
        assert!(
            response.get("result").is_none(),
            "an `invalid_request` response must not carry a `result`, got {response}"
        );

        // The connection must still be usable afterwards (§6).
        raw.send_json(&json!({ "id": 44, "method": "ping", "params": {} }))
            .await;
        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(response["id"], json!(44));
        assert_eq!(
            response["result"]["protocol_version"],
            json!(1),
            "the connection must stay open after `invalid_request`, got {response}"
        );
    });
}

// ---------------------------------------------------------------------------
// Scenario 4 — registry error path keeps the connection open
// ---------------------------------------------------------------------------

#[test]
fn error_response_does_not_close_the_connection() {
    let t = TestRuntime::start();
    let client = t.connect();

    assert_error_code(
        t.block_on_timeout(client.get_app(&AppId::from("org.example.absent"))),
        ErrorCode::UnknownApp,
        "get_app(unknown app id)",
    );

    let info = expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after an unknown-app error response",
    );
    assert_eq!(
        info.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "the connection must survive an error response (§6)"
    );
}

// ---------------------------------------------------------------------------
// Scenario 5 — compositor error path keeps the connection open
// ---------------------------------------------------------------------------

#[test]
fn unknown_window_error_keeps_the_connection_open() {
    let t = TestRuntime::start();
    let client = t.connect();

    // `docs/protocol.md` §6 leaves the error `data` object free-form, so only the
    // code is asserted here.
    assert_error_code(
        t.block_on_timeout(client.get_window(WindowId(99))),
        ErrorCode::UnknownWindow,
        "get_window(99)",
    );

    let info = expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after an unknown-window error response",
    );
    assert_eq!(
        info.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "the connection must survive an error response (§6)"
    );
}

// ---------------------------------------------------------------------------
// Scenario 6 — a malformed line closes only that connection
// ---------------------------------------------------------------------------

#[test]
fn malformed_ndjson_closes_only_that_connection() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();
    // The two §6 boundary lines below each corrupt their own connection, so they
    // need one connection each. They are created here because the harness's
    // `connect_raw` blocks on the runtime and cannot be called from inside
    // `block_on`.
    let non_object = t.connect_raw();
    let no_id = t.connect_raw();

    t.block_on(async {
        raw.send_json(&json!({ "id": 1, "method": "ping", "params": {} }))
            .await;
        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(
            response["id"],
            json!(1),
            "the connection works before the malformed line, got {response}"
        );

        // A line that is not JSON at all: framing corruption closes this
        // connection and only this connection (§1).
        raw.send_line("this is not json").await;
        raw.expect_closed(Duration::from_secs(5)).await;

        // The §6 boundary: JSON alone is not a request. A line that carries no
        // `u64` id cannot be answered with an error response (there is no id to
        // put on it) and identifies no request, so it is framing corruption and
        // closes — unlike a request-shaped line, which is answered and kept
        // (scenario 3b).
        for (mut raw, line) in [
            (non_object, "[1, 2, 3]"),       // JSON, but not an object
            (no_id, r#"{"method":"ping"}"#), // object, but no `id`
        ] {
            raw.send_json(&json!({ "id": 2, "method": "ping", "params": {} }))
                .await;
            let response = raw.expect_json(REQUEST_TIMEOUT).await;
            assert_eq!(
                response["id"],
                json!(2),
                "the connection works before {line}, got {response}"
            );

            raw.send_line(line).await;
            raw.expect_closed(Duration::from_secs(5)).await;
        }
    });

    // A second, independent client is unaffected and the runtime still serves.
    let client = t.connect();
    let info = expect_ok(
        t.block_on_timeout(client.ping()),
        "ping from a second client after the first sent a malformed line",
    );
    assert_eq!(info.protocol_version, adesk_server::PROTOCOL_VERSION);
}

// ---------------------------------------------------------------------------
// Scenario 7 — concurrent requests on one connection
// ---------------------------------------------------------------------------

#[test]
fn concurrent_requests_on_one_connection_all_resolve() {
    let t = TestRuntime::start();
    let client = t.connect();
    let missing_app = AppId::from("org.example.absent");

    // Nine mixed requests in flight at once; each must resolve with its own
    // response (§1: requests multiplex on one connection, ids disambiguate).
    // The typed SDK has no per-request deadline, so the whole join is bounded
    // by the harness: a runtime that accepted but never answered would hang the
    // test binary forever instead of failing.
    let (ping, windows, focus, apps, app, observed, changed, inspect, quiet) =
        t.block_on_timeout(async {
            tokio::join!(
                client.ping(),
                client.list_windows(),
                client.get_focus(),
                client.list_apps(None, false),
                client.get_app(&missing_app),
                client.observe(
                    ObserveRequest::timeout()
                        .timeout_ms(150)
                        .include_image(false)
                ),
                client.wait_for_change(WaitForChangeRequest::default().timeout_ms(150)),
                client.inspect_capture(InspectCaptureRequest::default()),
                client.wait_for_quiet(WaitForQuietRequest::default().timeout_ms(150)),
            )
        });

    let ping = expect_ok(ping, "concurrent ping");
    assert_eq!(ping.protocol_version, adesk_server::PROTOCOL_VERSION);

    let windows = expect_ok(windows, "concurrent list_windows");
    assert!(
        windows.windows.is_empty(),
        "a fresh runtime has no windows, got {:?}",
        windows.windows
    );

    let focus = expect_ok(focus, "concurrent get_focus");
    assert!(
        focus.window_id.is_none(),
        "no window can hold focus yet, got {:?}",
        focus.window_id
    );
    assert!(!focus.surface_focus, "no surface holds keyboard focus yet");

    let apps = expect_ok(apps, "concurrent list_apps");
    assert!(
        apps.is_empty(),
        "an empty app-dir runtime lists no apps, got {}",
        apps.len()
    );

    assert_error_code(app, ErrorCode::UnknownApp, "concurrent get_app(unknown)");

    let observed = expect_ok(observed, "concurrent observe(until=timeout, 150ms)");
    assert!(
        !observed.observation.timed_out,
        "until=timeout reaches its horizon by definition and is not an expiry, got {:?}",
        observed.observation
    );
    assert!(
        observed.observation.elapsed_ms >= 150,
        "until=timeout must sample its full 150 ms horizon, got {} ms",
        observed.observation.elapsed_ms
    );

    let changed = expect_ok(changed, "concurrent wait_for_change(150ms)");
    assert!(
        changed.timed_out,
        "wait_for_change(150ms) with no commits must expire, got {changed:?}"
    );

    let inspect = expect_ok(inspect, "concurrent inspect_capture");
    assert_eq!(
        inspect.width, OUTPUT_WIDTH,
        "inspect_capture is the full output"
    );
    assert_eq!(
        inspect.height, OUTPUT_HEIGHT,
        "inspect_capture is the full output"
    );

    let quiet = expect_ok(
        quiet,
        "concurrent wait_for_quiet(quiet_ms=250, timeout=150ms)",
    );
    assert!(
        quiet.timed_out,
        "wait_for_quiet cannot be quiet before its 250ms quiet period, got {quiet:?}"
    );

    // The connection is still healthy after the burst.
    let after = expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after the concurrent burst",
    );
    assert_eq!(after.protocol_version, adesk_server::PROTOCOL_VERSION);
}

// ---------------------------------------------------------------------------
// Scenario 8 — pipelined requests get exactly one response each
// ---------------------------------------------------------------------------

#[test]
fn pipelined_requests_get_exactly_one_response_each() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    t.block_on(async {
        // Three requests written back-to-back without waiting for an answer.
        raw.send_line(r#"{"id":1,"method":"ping","params":{}}"#)
            .await;
        raw.send_line(r#"{"id":2,"method":"list_windows","params":{}}"#)
            .await;
        raw.send_line(r#"{"id":3,"method":"get_focus","params":{}}"#)
            .await;

        let mut ids = Vec::new();
        for index in 0..3 {
            let response = raw.expect_json(REQUEST_TIMEOUT).await;
            // All three pipelined methods — `ping`, `list_windows`, `get_focus` —
            // succeed on a fresh runtime, so every response must carry a `result`
            // and no `error`; accepting either would let a server that fails all
            // three still pass.
            assert!(
                response.get("result").is_some(),
                "pipelined response {index} must carry a `result`: {response}"
            );
            assert!(
                response.get("error").is_none(),
                "pipelined response {index} must not carry an `error`: {response}"
            );
            let id = response["id"]
                .as_u64()
                .unwrap_or_else(|| panic!("pipelined response {index} has no u64 id: {response}"));
            ids.push(id);
        }
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![1, 2, 3],
            "each pipelined request must be answered exactly once (§1)"
        );

        // No leftover frames: the next response belongs to the next request.
        raw.send_line(r#"{"id":4,"method":"ping","params":{}}"#)
            .await;
        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(
            response["id"],
            json!(4),
            "a stale/duplicate frame arrived before the id-4 response: {response}"
        );
        assert_eq!(
            response["result"]["protocol_version"],
            json!(1),
            "id 4 must be the ping result, got {response}"
        );
    });
}

// ---------------------------------------------------------------------------
// Scenario 9 — blank lines are ignored
// ---------------------------------------------------------------------------

#[test]
fn blank_lines_are_ignored() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    t.block_on(async {
        // Blank lines are not frames and must not close the connection.
        raw.send_line("\n\n").await;
        raw.send_json(&json!({ "id": 5, "method": "ping", "params": {} }))
            .await;

        let response = raw.expect_json(REQUEST_TIMEOUT).await;
        assert_eq!(
            response["id"],
            json!(5),
            "the ping after the blank lines must be answered, got {response}"
        );
        assert_eq!(response["result"]["protocol_version"], json!(1));
    });
}
