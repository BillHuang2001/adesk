//! AGP §5.7 human inspector — end-to-end suite.
//!
//! `inspect_capture` and `inspect_subscribe` are the debug-only view of the
//! runtime: a full-output composition with overlays, never the agent-facing
//! `capture_*` path (§5.7). This suite pins the wire contract against a live
//! pixman runtime:
//!
//! - the captured frame is the whole 1280x720 output, PNG-encoded, decodable by
//!   the SDK, and `max_dimension` bounds the longer edge *after* overlays are
//!   composited (downscale only, never upscale);
//! - an empty overlay set is legal and renders the plain composition;
//! - the subscription answers a non-zero `subscription_id`, pushes
//!   `inspect_frame` events carrying that id plus a PNG of the output, and
//!   `unsubscribe_events` stops the stream;
//! - a push loop that can no longer render (the compositor is gone) deregisters
//!   its own stream instead of leaving a stale registry entry behind.
//!
//! Overlay *drawing* is unit-tested inside `adesk-inspector`; with no windows on
//! the output, overlays draw nothing, so this suite deliberately does not
//! compare overlay and plain images.

mod common;

use std::time::{Duration, Instant};

use adesk_client::{decode_image, ImageFormat, ImagePayload, InspectCaptureRequest};
use adesk_core::OverlayKind;
use serde_json::json;

use common::{eventually, expect_ok, TestRuntime, OUTPUT_HEIGHT, OUTPUT_WIDTH};

/// Asserts the §4 `ImagePayload` envelope and that the SDK can decode it.
///
/// `ImagePayload::format` is the wire enum (`adesk_proto::ImageFormat`), a
/// distinct type from the request-side [`ImageFormat`]; both use the same
/// `"png"`/`"rgba8"` names, so the format assertion compares the serialized
/// wire value.
fn assert_png_payload(payload: &ImagePayload, width: u32, height: u32, what: &str) {
    assert_eq!(payload.width, width, "{what}: image width");
    assert_eq!(payload.height, height, "{what}: image height");
    assert_eq!(
        serde_json::to_value(payload.format).expect("a wire image format serializes"),
        serde_json::to_value(ImageFormat::Png).expect("a client image format serializes"),
        "{what}: unexpected image format"
    );
    assert!(
        payload.stride.is_none(),
        "{what}: a PNG payload carries no stride, got {:?}",
        payload.stride
    );
    assert!(
        !payload.data.is_empty(),
        "{what}: the encoded image must not be empty"
    );
    assert!(
        payload.scale.is_finite() && payload.scale > 0.0,
        "{what}: scale must be a positive finite factor, got {}",
        payload.scale
    );

    // A payload the SDK cannot decode is not a usable capture.
    let decoded = expect_ok(decode_image(payload), &format!("{what}: decode_image"));
    assert_eq!(decoded.width, width, "{what}: decoded width");
    assert_eq!(decoded.height, height, "{what}: decoded height");
    assert!(
        !decoded.data.is_empty(),
        "{what}: decoded pixels must not be empty"
    );
}

/// Default overlays over the whole output: a 1280x720 PNG at full resolution.
#[test]
fn inspect_capture_with_default_overlays_returns_output_sized_png() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let what = "inspect_capture(default overlays)";
    let payload = expect_ok(
        runtime.block_on_timeout(client.inspect_capture(InspectCaptureRequest::default())),
        what,
    );

    assert_png_payload(&payload, OUTPUT_WIDTH, OUTPUT_HEIGHT, what);
    assert!(
        (payload.scale - 1.0).abs() < 0.01,
        "{what}: a whole-output capture is not scaled, got scale {}",
        payload.scale
    );
}

/// An empty overlay set is legal (§5.7) and returns the same output-sized PNG.
///
/// `InspectCaptureRequest` is `#[non_exhaustive]`, so the request is built
/// through its constructor: `overlays([])` is `Default` minus the overlay list
/// (no region, no `max_dimension`). With no windows on the output, overlays draw
/// nothing, so the two images are not required to differ — only the envelope is.
#[test]
fn inspect_capture_with_empty_overlays_returns_output_sized_png() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let what = "inspect_capture(overlays: [])";
    let request = InspectCaptureRequest::overlays(Vec::<OverlayKind>::new());
    let payload = expect_ok(
        runtime.block_on_timeout(client.inspect_capture(request)),
        what,
    );

    assert_png_payload(&payload, OUTPUT_WIDTH, OUTPUT_HEIGHT, what);
    assert!(
        (payload.scale - 1.0).abs() < 0.01,
        "{what}: a whole-output capture is not scaled, got scale {}",
        payload.scale
    );
}

/// `max_dimension` bounds the longer edge of the composed frame, never upscales.
///
/// Overlays are composited at full resolution and the downscale is applied to
/// the result (§5.7 handler), so the reported dimensions are exactly the
/// bounded output size: 1280x720 → 640x360 → 64x36, and an unbounded request
/// stays 1280x720.
#[test]
fn inspect_capture_max_dimension_downscales_after_overlays() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    for (max_dimension, width, height) in [
        (640, 640, 360),
        (64, 64, 36),
        (4096, OUTPUT_WIDTH, OUTPUT_HEIGHT),
    ] {
        let what = format!("inspect_capture(max_dimension={max_dimension})");
        let request = InspectCaptureRequest::default().max_dimension(max_dimension);
        let payload = expect_ok(
            runtime.block_on_timeout(client.inspect_capture(request)),
            &what,
        );
        assert_png_payload(&payload, width, height, &what);

        match max_dimension {
            640 => {
                // The reported factor must be a real downscale. `1.0` is the §4
                // default meaning "not reported"; when the server reports the
                // applied factor it must be the true 640/1280 = 0.5.
                assert!(
                    payload.scale > 0.0 && payload.scale <= 1.0,
                    "{what}: scale {} is outside (0, 1]",
                    payload.scale
                );
                if (payload.scale - 1.0).abs() >= f64::EPSILON {
                    assert!(
                        (payload.scale - 0.5).abs() < 0.01,
                        "{what}: 1280 -> 640 is a 0.5 downscale, got scale {}",
                        payload.scale
                    );
                }
            }
            4096 => assert!(
                (payload.scale - 1.0).abs() < 0.01,
                "{what}: a capture below the output size is not scaled, got scale {}",
                payload.scale
            ),
            _ => {}
        }
    }
}

/// `inspect_subscribe` streams `inspect_frame` events; unsubscribing stops them.
///
/// The typed SDK hides the frames behind a `futures::Stream`, which the harness
/// deliberately does not depend on, so this test speaks the wire directly:
/// subscribe, read one frame, unsubscribe, then watch the connection.
///
/// §5.6/§5.7 pin the `{}` unsubscribe result, not strict post-response silence.
/// The push loop (`src/dispatch/inspect.rs`) is sequential: it checks liveness,
/// then awaits a full-output render plus PNG encode, then enqueues without
/// re-checking, so at most one frame that was already in flight may still be
/// delivered after the response. The contract asserted here is therefore:
/// unsubscribing stops the stream — at most one already-in-flight frame may
/// follow the response, and nothing after that.
#[test]
fn inspect_subscribe_streams_frames_and_unsubscribe_stops_them() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_raw();

    runtime.block_on(async {
        raw.send_json(&json!({
            "id": 1,
            "method": "inspect_subscribe",
            "params": { "overlays": ["window_ids"], "min_interval_ms": 50 },
        }))
        .await;

        // The push loop is spawned before the handler returns, so the first
        // pushed frame may precede the response; match on the request id.
        let response = raw
            .expect_json_matching(Duration::from_secs(5), |value| value["id"] == json!(1))
            .await;
        let subscription_id = response["result"]["subscription_id"]
            .as_u64()
            .unwrap_or_else(|| {
                panic!("inspect_subscribe must answer result.subscription_id, got {response}")
            });
        assert_ne!(
            subscription_id, 0,
            "subscription ids are non-zero, got {response}"
        );

        let frame = raw
            .expect_json_matching(Duration::from_secs(5), |value| {
                value["event"] == json!("inspect_frame")
            })
            .await;
        assert_eq!(
            frame["data"]["subscription_id"].as_u64(),
            Some(subscription_id),
            "an inspect_frame carries the subscription that produced it: {frame}"
        );
        assert_eq!(
            frame["data"]["image"]["format"].as_str(),
            Some("png"),
            "§5.7 frames are PNG payloads: {frame}"
        );
        assert_eq!(
            frame["data"]["image"]["width"].as_u64(),
            Some(u64::from(OUTPUT_WIDTH)),
            "a pushed frame is the whole output: {frame}"
        );
        assert_eq!(
            frame["data"]["image"]["height"].as_u64(),
            Some(u64::from(OUTPUT_HEIGHT)),
            "a pushed frame is the whole output: {frame}"
        );
        assert!(
            frame["seq"].is_u64(),
            "§1: every event frame carries a numeric `seq`: {frame}"
        );
        assert!(
            frame["ts_ms"].is_u64(),
            "§1: every event frame carries a numeric `ts_ms`: {frame}"
        );

        raw.send_json(&json!({
            "id": 2,
            "method": "unsubscribe_events",
            "params": { "subscription_id": subscription_id },
        }))
        .await;
        let response = raw
            .expect_json_matching(Duration::from_secs(5), |value| value["id"] == json!(2))
            .await;
        assert_eq!(
            response["result"],
            json!({}),
            "§5.6: unsubscribe_events answers an empty result, got {response}"
        );

        // Grace window: the sequential push loop may have started an expensive
        // render before the unsubscribe landed and enqueue that frame without
        // re-checking liveness. 500 ms comfortably exceeds `min_interval_ms=50`
        // plus a full-output pixman render.
        let grace_deadline = Instant::now() + Duration::from_millis(500);
        let mut strays = Vec::new();
        while Instant::now() < grace_deadline {
            match raw.read_json(Duration::from_millis(100)).await {
                Some(value) => {
                    if value["event"] == json!("inspect_frame")
                        && value["data"]["subscription_id"].as_u64() == Some(subscription_id)
                    {
                        strays.push(value);
                    }
                }
                // `read_json` returns `None` for both timeout and EOF, so yield
                // rather than spin at 100% CPU once the connection is closed.
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        assert!(
            strays.len() <= 1,
            "the sequential push loop permits at most one in-flight inspect_frame after \
             unsubscribe, but {} arrived: {strays:?}",
            strays.len()
        );
        if let Some(frame) = strays.first() {
            assert_eq!(
                frame["data"]["subscription_id"].as_u64(),
                Some(subscription_id),
                "an in-flight frame must belong to the unsubscribed subscription, not a \
                 leak from another: {frame}"
            );
        }

        // Quiet window: a broken or never-stopping loop keeps producing frames
        // here, so the regression signal is preserved and stronger than a bare
        // zero-count over a single window.
        let quiet_deadline = Instant::now() + Duration::from_millis(1_500);
        let mut late = Vec::new();
        while Instant::now() < quiet_deadline {
            match raw.read_json(Duration::from_millis(100)).await {
                Some(value) => {
                    if value["event"] == json!("inspect_frame")
                        && value["data"]["subscription_id"].as_u64() == Some(subscription_id)
                    {
                        late.push(value);
                    }
                }
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        assert!(
            late.is_empty(),
            "unsubscribe_events must stop inspect_frame events, but {} arrived after the \
             grace window: {late:?}",
            late.len()
        );
    });
}

/// A push loop that can no longer render deregisters its stream.
///
/// The compositor going away is the loop's terminal failure: every later render
/// answers `shutting_down`. The loop must then remove its own
/// `InspectRegistry` entry, because that registry is the truth
/// `unsubscribe_events` and `ServerContext::inspect_subscriptions` read — a
/// stopped stream that stays listed is state the client cannot see. The AGP
/// connection is deliberately kept open here (only the runtime's own shutdown
/// closes it), so the closed-sink short-circuit and
/// `event_pump::prune_inspect_streams` cannot be what removes the entry.
#[test]
fn a_stream_that_can_no_longer_render_is_deregistered() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_raw();

    let subscription_id = runtime.block_on(async {
        raw.send_json(&json!({
            "id": 1,
            "method": "inspect_subscribe",
            "params": { "overlays": ["window_ids"], "min_interval_ms": 50 },
        }))
        .await;
        let response = raw
            .expect_json_matching(Duration::from_secs(5), |value| value["id"] == json!(1))
            .await;
        let subscription_id = response["result"]["subscription_id"]
            .as_u64()
            .unwrap_or_else(|| {
                panic!("inspect_subscribe must answer result.subscription_id, got {response}")
            });

        // One pushed frame proves the loop is live and rendering before the
        // compositor is taken away.
        let frame = raw
            .expect_json_matching(Duration::from_secs(5), |value| {
                value["event"] == json!("inspect_frame")
                    && value["data"]["subscription_id"].as_u64() == Some(subscription_id)
            })
            .await;
        assert!(
            frame["seq"].is_u64(),
            "§1: a pushed frame carries a numeric `seq`: {frame}"
        );
        assert!(
            runtime
                .context()
                .inspect_subscriptions
                .list()
                .iter()
                .any(|stream| stream.id == subscription_id),
            "a streaming subscription must be registered: {subscription_id}"
        );

        subscription_id
    });

    // Kill the compositor: nothing can render any more, so the loop's next
    // iteration takes its error path.
    runtime.block_on(async {
        runtime
            .context()
            .compositor
            .shutdown()
            .await
            .expect("the compositor must stop cleanly while the runtime is up");
    });

    assert!(
        eventually(Duration::from_secs(10), || {
            runtime
                .context()
                .inspect_subscriptions
                .list()
                .iter()
                .all(|stream| stream.id != subscription_id)
        }),
        "a push loop whose renders fail must deregister its subscription instead of \
         leaving a stale `InspectRegistry` entry behind"
    );

    // The connection itself is untouched by the loop's failure: the stream ended
    // because it could not render, never because this client went away.
    let response = runtime.block_on(async {
        raw.send_json(&json!({ "id": 2, "method": "ping", "params": {} }))
            .await;
        raw.expect_json_matching(Duration::from_secs(5), |value| value["id"] == json!(2))
            .await
    });
    assert_eq!(
        response["result"]["protocol_version"],
        json!(1),
        "the connection must survive a dead inspect stream, got {response}"
    );
}
