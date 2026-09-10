//! AGP §1 — one global monotonic `seq` domain for server-synthesized events.
//!
//! §1 gives every event frame a `seq` that is monotonic across the runtime's
//! lifetime, with gaps allowed and reuse forbidden. Most events are allocated by
//! the compositor's own emitter, but two kinds are synthesized by the server and
//! must still draw from the *same* counter (a server-private counter could
//! collide with a compositor event):
//!
//! - `app_launched` — emitted by the `launch_app` handler (§5.2);
//! - `inspect_frame` — pushed once per throttled refresh by an
//!   `inspect_subscribe` stream (§5.7).
//!
//! Both call `RuntimeCommand::ReserveSeq`, which advances the compositor's
//! central counter and publishes nothing: a reserved number may be skipped but
//! can never be reused by a later event, and the compositor's `QueryState`
//! watermark moves with it. These tests drive a real runtime with no Wayland
//! client and no fixtures that emit events, so nothing else allocates `seq`
//! while they run — which is what makes the strict relations below
//! deterministic rather than lucky.
//!
//! | Test | Protocol claim |
//! |---|---|
//! | `app_launched_seqs_are_strictly_above_the_observed_watermark` | the first launch's `app_launched` `seq` is strictly above the watermark sampled before the launch; the second launch is strictly above the first (no reuse) and above the watermark that was visible before it. |
//! | `inspect_frame_seqs_are_strictly_above_the_observed_watermark` | consecutive `inspect_frame` pushes carry strictly increasing `seq`s, every one of them above the watermark sampled before the subscription existed. |
//!
//! ## How the watermark is sampled
//!
//! `observe(until = timeout)` waits its horizon and reports the observer's
//! global watermark in `result.observation.seq` — the highest `seq` published
//! when the wait resolved (`docs/protocol.md` §4/§5.4; the response shape is
//! pinned by `tests/observation.rs::observation_reports_the_watermark_fields`).
//! Sampling emits nothing, so it never moves the counter it measures. Every
//! sample and every event frame is read on the wire through the harness's
//! [`RawClient`], because `futures` is not a dependency and the typed
//! `EventStream` cannot be polled here.
//!
//! ## Deliberately absent: compositor-emitted events
//!
//! A third test would interleave a compositor-emitted event with the
//! server-synthesized ones. Nothing available to these suites can produce one
//! without a Wayland client (window, commit and focus events all need a
//! surface), and injecting a hand-made `RuntimeEvent` into the broadcast would
//! fabricate the very `seq` relation under test, so that case is left for a
//! suite that can connect a real client.

use adesk_core::{AppId, LaunchId};
use serde_json::{json, Value};

mod common;

use common::{expect_ok, write_desktop_entry, RawClient, TestRuntime, REQUEST_TIMEOUT};

/// The horizon of a watermark sample, in milliseconds.
///
/// `until = timeout` resolves *at* the horizon by definition (§5.4), so a short
/// one keeps the suite fast without changing what is reported.
const WATERMARK_TIMEOUT_MS: u64 = 150;

/// The launch fixture: `Exec` spawns `true` from `PATH` (the Nix dev shell has
/// no `/bin`), so the launch succeeds without an installed application — the
/// registry only has to record it and the server to announce it.
const LAUNCH_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Sequence Fixture
Exec=true
";

/// Sends one raw request and returns the response carrying the same `id`.
///
/// Event frames interleaved on the connection are skipped by
/// [`RawClient::expect_json_matching`].
///
/// # Panics
///
/// Panics if the connection closes or no matching response arrives within
/// [`REQUEST_TIMEOUT`].
fn raw_request(
    t: &TestRuntime,
    raw: &mut RawClient,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    t.block_on_timeout(async {
        raw.send_json(&json!({ "id": id, "method": method, "params": params }))
            .await;
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| value.get("id") == Some(&json!(id)))
            .await
    })
}

/// Samples the runtime's global `seq` watermark on the wire.
///
/// Returns `result.observation.seq` of an `observe(until = timeout)` call: the
/// highest `seq` published when that wait resolved. The sample is a pure read —
/// it neither emits an event nor reserves a number.
///
/// # Panics
///
/// Panics when the request answers an error frame or the observation carries no
/// numeric `seq`.
fn observe_watermark(t: &TestRuntime, raw: &mut RawClient, id: u64) -> u64 {
    let response = raw_request(
        t,
        raw,
        id,
        "observe",
        json!({
            "until": { "type": "timeout" },
            "timeout_ms": WATERMARK_TIMEOUT_MS,
            "include_image": false,
        }),
    );
    assert!(
        response.get("error").is_none(),
        "observe(until = timeout) answered with an error frame: {response}"
    );
    response
        .pointer("/result/observation/seq")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("result.observation.seq is missing or not a number: {response}"))
}

/// The `seq` of a delivered event frame (§1: the frame-level counter).
///
/// # Panics
///
/// Panics with the whole frame when it carries no numeric `seq`.
fn frame_seq(frame: &Value, what: &str) -> u64 {
    frame
        .get("seq")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{what}: the event frame carries no numeric `seq`: {frame}"))
}

/// The `subscription_id` of a raw subscribe response.
///
/// # Panics
///
/// Panics with the whole response when it carries no id.
fn subscription_id(response: &Value, what: &str) -> u64 {
    response
        .pointer("/result/subscription_id")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{what}: expected a subscription_id, got {response}"))
}

/// Predicate selecting the `app_launched` frame of one specific launch.
///
/// Matching on `data.launch_id` ties each read to its own `launch_app` response,
/// so the two launches of the test cannot be confused even though both frames
/// arrive on the same connection.
fn launch_frame(launch_id: LaunchId) -> impl FnMut(&Value) -> bool {
    move |value| {
        value.get("event") == Some(&json!("app_launched"))
            && value.pointer("/data/launch_id").and_then(Value::as_u64) == Some(launch_id.0)
    }
}

/// Reads the next `inspect_frame` push of `subscription` and returns its `seq`.
///
/// Frames belong to a subscription id, and each read consumes one line, so
/// consecutive calls return consecutive pushes of that stream.
///
/// # Panics
///
/// Panics if the connection closes or no such frame arrives within
/// [`REQUEST_TIMEOUT`].
fn next_inspect_frame_seq(
    t: &TestRuntime,
    raw: &mut RawClient,
    subscription: u64,
    what: &str,
) -> u64 {
    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("inspect_frame"))
                && value
                    .pointer("/data/subscription_id")
                    .and_then(Value::as_u64)
                    == Some(subscription)
        })
        .await
    });
    assert_eq!(
        frame
            .pointer("/data/subscription_id")
            .and_then(Value::as_u64),
        Some(subscription),
        "{what}: the frame must belong to the inspected subscription: {frame}"
    );
    frame_seq(&frame, what)
}

/// `app_launched` (server-synthesized, §5.2) draws from the compositor's counter.
///
/// The `launch_app` handler reserves the event's `seq` before spawning, so a
/// launch can never announce a number at or below the watermark a client had
/// already observed (§1), and a second launch can never repeat the first's
/// number. The second sample of the watermark makes the second assertion strict
/// about a *visible* watermark rather than about the initial `0`: the delivered
/// frame proves the observer already saw it, because the event pump feeds the
/// observer before it fans a frame out.
#[test]
fn app_launched_seqs_are_strictly_above_the_observed_watermark() {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(dir.path(), "org.example.sequence.desktop", LAUNCH_ENTRY);
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();
    let mut raw = t.connect_raw();

    // Sampled before the first launch: `app_launched` must be allocated above
    // everything the runtime published so far, never at or below it.
    let watermark = observe_watermark(&t, &mut raw, 1);

    // Only `app_launched`, so the connection carries nothing else while the
    // launches below run.
    let response = raw_request(
        &t,
        &mut raw,
        2,
        "subscribe_events",
        json!({ "kinds": ["app_launched"] }),
    );
    let subscription = subscription_id(&response, "subscribe_events(kinds=[app_launched])");
    assert_ne!(subscription, 0, "§5.6: `subscription_id` is a non-zero u64");
    assert_eq!(
        t.context().subscriptions.len(),
        1,
        "the subscription must be registered before the launch"
    );

    // ------------------------------------------------------------ first launch

    let app_id = AppId::from("org.example.sequence");
    let first = expect_ok(
        t.block_on_timeout(client.launch_app(&app_id, &[])),
        "launch_app(org.example.sequence), first launch",
    );
    assert_eq!(first.app_id, app_id, "the launch reports the requested app");
    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, launch_frame(first.launch_id))
            .await
    });
    assert_eq!(
        frame.pointer("/data/app_id").and_then(Value::as_str),
        Some(app_id.0.as_str()),
        "the `app_launched` frame must describe the launch that produced it: {frame}"
    );
    let first_seq = frame_seq(&frame, "app_launched #1");
    assert!(
        first_seq > watermark,
        "§1: the first `app_launched` seq ({first_seq}) must be strictly greater \
         than the watermark sampled before the launch ({watermark}); reusing a \
         published number would be a protocol violation"
    );

    // The pump hands the event to the observer *before* the fan-out delivers the
    // frame, so reading it above already proves the observer saw `first_seq`.
    let watermark_after_first = observe_watermark(&t, &mut raw, 3);
    assert!(
        watermark_after_first >= first_seq,
        "the delivered frame proves the observer processed seq {first_seq}, but \
         the sampled watermark is {watermark_after_first}"
    );

    // ----------------------------------------------------------- second launch

    let second = expect_ok(
        t.block_on_timeout(client.launch_app(&app_id, &[])),
        "launch_app(org.example.sequence), second launch",
    );
    assert_ne!(
        first.launch_id, second.launch_id,
        "§5.2: every `launch_app` allocates its own launch id"
    );
    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, launch_frame(second.launch_id))
            .await
    });
    let second_seq = frame_seq(&frame, "app_launched #2");
    assert!(
        second_seq > first_seq,
        "§1: `seq` is strictly increasing across server-synthesized events; the \
         second `app_launched` ({second_seq}) must be above the first ({first_seq})"
    );
    assert!(
        second_seq > watermark_after_first,
        "§1: the second reservation ({second_seq}) must stay above the watermark \
         visible before it ({watermark_after_first})"
    );
}

/// `inspect_frame` (server-synthesized, §5.7) draws from the compositor's counter.
///
/// Each pushed frame reserves one number from the same global counter, so the
/// stream is strictly increasing and never dips to or below a watermark a client
/// had already seen. The push loop renders, reserves and enqueues one frame at a
/// time, so frame order and `seq` order agree.
///
/// Note: an idle runtime publishes no event before the subscription exists, so
/// the sampled watermark is `0` here and the weight of the test is on the two
/// consecutive pushes — a reused or fabricated number would make the second
/// frame repeat the first instead of exceeding it.
#[test]
fn inspect_frame_seqs_are_strictly_above_the_observed_watermark() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    // Sampled before the stream exists: every frame it pushes is allocated after
    // this point.
    let watermark = observe_watermark(&t, &mut raw, 1);

    // The push loop is spawned before the handler returns, so the first pushed
    // frame may precede the response; `raw_request` matches on the request id and
    // skips it, and the assertions below only need the frames read after it.
    let response = raw_request(
        &t,
        &mut raw,
        2,
        "inspect_subscribe",
        json!({ "overlays": ["window_ids"], "min_interval_ms": 50 }),
    );
    let subscription = subscription_id(&response, "inspect_subscribe");
    assert_ne!(subscription, 0, "§5.7: `subscription_id` is a non-zero u64");

    // Two consecutive pushes of that stream.
    let first_seq = next_inspect_frame_seq(&t, &mut raw, subscription, "inspect_frame #1");
    let second_seq = next_inspect_frame_seq(&t, &mut raw, subscription, "inspect_frame #2");

    assert!(
        first_seq > watermark,
        "§1: the first `inspect_frame` seq ({first_seq}) must be strictly greater \
         than the watermark sampled before the subscription ({watermark})"
    );
    assert!(
        second_seq > first_seq,
        "§1: consecutive `inspect_frame` pushes must be strictly increasing; the \
         second ({second_seq}) must be above the first ({first_seq}), never reused"
    );
    assert!(
        second_seq > watermark,
        "§1: every server-synthesized seq stays above the watermark a client had \
         already observed (second frame: {second_seq}, watermark: {watermark})"
    );

    // §5.6/§5.7 pin the `{}` result, not post-response silence: the sequential
    // push loop may still deliver one frame that was already in flight, so
    // nothing is asserted about frames after this response.
    let response = raw_request(
        &t,
        &mut raw,
        3,
        "unsubscribe_events",
        json!({ "subscription_id": subscription }),
    );
    assert_eq!(
        response.get("result"),
        Some(&json!({})),
        "§5.6: unsubscribe_events answers an empty result, got {response}"
    );
}
