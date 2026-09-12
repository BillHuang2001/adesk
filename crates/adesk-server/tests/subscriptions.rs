//! AGP §5.6 — event subscriptions (`subscribe_events` / `unsubscribe_events`).
//!
//! ## Coverage
//!
//! | Test | Protocol claim |
//! |---|---|
//! | `subscribe_events_returns_a_subscription_id` | `subscribe_events` answers a non-zero `subscription_id` and registers exactly one server-side subscription; `unsubscribe_events` removes it. |
//! | `subscribe_with_surface_damage_filter_is_accepted` | the `surface_damage` filter alias is accepted. |
//! | `subscribe_with_window_filter_is_accepted` | a `window_id` filter is legal even for a window that does not exist yet — filtering is a delivery optimisation, not a lookup. |
//! | `subscribe_with_inspect_frame_kind_is_invalid_request` | `inspect_frame` is not a subscribable kind (§5.7 pushes it through `inspect_subscribe`); the runtime answers `invalid_request` and keeps the connection open. |
//! | `unsubscribe_events_is_idempotent_for_unknown_ids` | cancelling an unknown/already-cancelled id is a success, not an error. |
//! | `disconnect_removes_subscriptions` | a dropped connection takes its subscriptions out of both registries. |
//! | `two_subscriptions_get_distinct_ids` | ids are unique per registry and survive repeated unsubscribes. |
//! | `window_created_event_carries_the_correlated_launch_id` | the event pump stamps `launch_id` on a `WindowCreated` the correlator matched, so the fanned-out frame carries it even though the compositor's own broadcast does not. |
//!
//! ## How the assertions read server state
//!
//! The typed `EventStream` cannot be polled here (`futures` is not a
//! dependency), so delivery itself is out of scope: the tests assert the
//! server-side truth in `ServerContext::subscriptions` /
//! `ServerContext::inspect_subscriptions` plus the wire-visible ids. The typed
//! stream **unsubscribes on drop**, so it is kept alive across the registry
//! assertion. `disconnect_removes_subscriptions` uses [`RawClient`] instead of a
//! stream precisely because a stream's drop-time unsubscribe would make the
//! registry assertion vacuous.

use std::time::Duration;

use adesk_client::{EventFilter, EventKind};
use adesk_core::{AppId, RuntimeEvent, WindowId};
use adesk_proto::EventFrame;
use serde_json::{json, Value};

mod common;

use common::{
    eventually, expect_ok, raw_request, subscription_id, write_desktop_entry, TestRuntime,
    REQUEST_TIMEOUT,
};
/// How long a test waits for a server-side registry effect to settle.
///
/// Registry mutation happens before the response is written, so this only
/// exists so a broken runtime fails with a clear message instead of hanging.
const SETTLE: Duration = Duration::from_secs(5);

#[test]
fn subscribe_events_returns_a_subscription_id() {
    let t = TestRuntime::start();
    let client = t.connect();

    // `EventStream` is not `Debug`, so unwrap by hand to keep the failure text
    // (the full `ClientError`, including the server's error code).
    let stream = match t.block_on_timeout(client.subscribe_events(EventFilter::all())) {
        Ok(stream) => stream,
        Err(error) => panic!("subscribe_events(EventFilter::all()): expected Ok, got {error:?}"),
    };

    let id = stream.subscription_id();
    assert_ne!(id, 0, "§5.6: `subscription_id` is a non-zero u64");
    assert_eq!(
        t.context().subscriptions.len(),
        1,
        "subscribe_events must register exactly one server-side subscription"
    );

    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(id)),
        "unsubscribe_events(subscription_id)",
    );
    assert!(
        eventually(SETTLE, || t.context().subscriptions.is_empty()),
        "unsubscribe_events must remove the subscription (still {} registered)",
        t.context().subscriptions.len()
    );

    // Dropped last: the stream's own best-effort unsubscribe is idempotent.
    drop(stream);
}

#[test]
fn subscribe_with_surface_damage_filter_is_accepted() {
    let t = TestRuntime::start();
    let client = t.connect();

    let filter = EventFilter::kinds(vec![EventKind::SurfaceDamage]);
    let stream = match t.block_on_timeout(client.subscribe_events(filter)) {
        Ok(stream) => stream,
        Err(error) => {
            panic!("subscribe_events(kinds=[surface_damage]): expected Ok, got {error:?}")
        }
    };

    let id = stream.subscription_id();
    assert_ne!(id, 0, "§5.6: `subscription_id` is a non-zero u64");
    assert_eq!(
        t.context().subscriptions.len(),
        1,
        "a surface_damage filter registers exactly one subscription"
    );

    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(id)),
        "unsubscribe_events(surface_damage subscription)",
    );
    assert!(
        eventually(SETTLE, || t.context().subscriptions.is_empty()),
        "unsubscribe_events must remove the surface_damage subscription"
    );

    drop(stream);
}

#[test]
fn subscribe_with_window_filter_is_accepted() {
    let t = TestRuntime::start();
    let client = t.connect();

    // Window 1 does not exist yet: §5.6 makes `window_id` a delivery filter,
    // not a lookup, so subscribing ahead of a window's creation is legal.
    let filter = EventFilter::all().window(WindowId(1));
    let stream = match t.block_on_timeout(client.subscribe_events(filter)) {
        Ok(stream) => stream,
        Err(error) => panic!("subscribe_events(window_id=1): expected Ok, got {error:?}"),
    };

    let id = stream.subscription_id();
    assert_ne!(id, 0, "§5.6: `subscription_id` is a non-zero u64");
    assert_eq!(
        t.context().subscriptions.len(),
        1,
        "a window filter registers exactly one subscription"
    );

    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(id)),
        "unsubscribe_events(window-filtered subscription)",
    );
    assert!(
        eventually(SETTLE, || t.context().subscriptions.is_empty()),
        "unsubscribe_events must remove the window-filtered subscription"
    );

    drop(stream);
}

#[test]
fn subscribe_with_inspect_frame_kind_is_invalid_request() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    // `inspect_frame` is pushed only to `inspect_subscribe` subscribers (§5.7);
    // §5.6 accepts the 11 filterable kinds and rejects anything else.
    let response = raw_request(
        &t,
        &mut raw,
        1,
        "subscribe_events",
        json!({ "kinds": ["inspect_frame"] }),
    );
    let code = response.pointer("/error/code").and_then(Value::as_str);
    assert_eq!(
        code,
        Some("invalid_request"),
        "§5.6: a non-subscribable kind must answer invalid_request, got {response}"
    );
    assert!(
        t.context().subscriptions.is_empty(),
        "a rejected filter must not register a subscription"
    );

    // §6: an error response never closes the connection.
    let pong = raw_request(&t, &mut raw, 2, "ping", json!({}));
    assert!(
        pong.get("result").is_some(),
        "the connection must stay open after invalid_request, got {pong}"
    );
}

#[test]
fn unsubscribe_events_is_idempotent_for_unknown_ids() {
    let t = TestRuntime::start();
    let client = t.connect();

    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(999_999)),
        "unsubscribe_events(999_999)",
    );
    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(u64::MAX)),
        "unsubscribe_events(u64::MAX)",
    );
    expect_ok(
        t.block_on_timeout(client.unsubscribe_events(999_999)),
        "repeated unsubscribe_events(999_999)",
    );

    // The connection is still healthy and nothing was registered.
    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after unknown unsubscribes",
    );
    assert!(
        t.context().subscriptions.is_empty(),
        "unknown ids must not register anything"
    );
    assert!(
        t.context().inspect_subscriptions.is_empty(),
        "unknown ids must not register an inspector stream"
    );
}

#[test]
fn disconnect_removes_subscriptions() {
    let t = TestRuntime::start();
    // A raw client, not a typed `EventStream`: the stream unsubscribes on drop,
    // which would make the registry assertion vacuous.
    let mut raw = t.connect_raw();

    let response = raw_request(&t, &mut raw, 1, "subscribe_events", json!({}));
    let id = subscription_id(&response, "subscribe_events({})");
    assert_ne!(id, 0, "§5.6: `subscription_id` is a non-zero u64");
    assert_eq!(
        t.context().subscriptions.len(),
        1,
        "the subscription must be registered before the disconnect"
    );

    // Closing the socket ends the read loop, which removes the session's
    // subscriptions from both registries (`Connection::run`).
    drop(raw);

    assert!(
        eventually(SETTLE, || {
            t.context().subscriptions.is_empty() && t.context().inspect_subscriptions.is_empty()
        }),
        "disconnect must drop the connection's subscriptions (events {}, inspector {})",
        t.context().subscriptions.len(),
        t.context().inspect_subscriptions.len()
    );
}

#[test]
fn two_subscriptions_get_distinct_ids() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    let first = raw_request(&t, &mut raw, 1, "subscribe_events", json!({}));
    let second = raw_request(
        &t,
        &mut raw,
        2,
        "subscribe_events",
        json!({ "kinds": ["surface_commit"] }),
    );
    let a = subscription_id(&first, "subscribe_events #1");
    let b = subscription_id(&second, "subscribe_events #2");
    assert_ne!(a, 0, "§5.6: ids are non-zero");
    assert_ne!(b, 0, "§5.6: ids are non-zero");
    assert_ne!(a, b, "ids must be unique within a registry");
    assert_eq!(
        t.context().subscriptions.len(),
        2,
        "two subscribe_events must register two subscriptions"
    );

    for (id, request_id) in [(a, 3_u64), (b, 4)] {
        let response = raw_request(
            &t,
            &mut raw,
            request_id,
            "unsubscribe_events",
            json!({ "subscription_id": id }),
        );
        assert!(
            response.get("result").is_some(),
            "unsubscribe_events({id}) failed: {response}"
        );
    }
    assert!(
        eventually(SETTLE, || t.context().subscriptions.is_empty()),
        "both subscriptions must be gone (still {} registered)",
        t.context().subscriptions.len()
    );

    // Idempotent: the same ids may be cancelled again.
    for (id, request_id) in [(a, 5_u64), (b, 6)] {
        let response = raw_request(
            &t,
            &mut raw,
            request_id,
            "unsubscribe_events",
            json!({ "subscription_id": id }),
        );
        assert!(
            response.get("result").is_some(),
            "repeated unsubscribe_events({id}) must succeed: {response}"
        );
    }
}

/// A `.desktop` entry for the launch fixture. `Exec` spawns `true` from `PATH`
/// (there is no `/bin` on the Nix dev shell), so the test needs no installed
/// application — only a recorded `LaunchRecord`.
const CORRELATION_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Correlation Fixture
Exec=true
StartupWMClass=CorrelationFixture
";

#[test]
fn window_created_event_carries_the_correlated_launch_id() {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(
        dir.path(),
        "org.example.correlation.desktop",
        CORRELATION_ENTRY,
    );
    let t = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = t.connect();

    // A tap on the compositor's raw broadcast: `launch_app` emits `AppLaunched`
    // there, and the compositor publishes its own `WindowCreated` there. Waiting
    // for the pump's subscription makes the injected event deterministic — a
    // broadcast channel does not replay, so an early send would be lost.
    let events = t.context().compositor.events().clone();
    let mut compositor_tap = events.subscribe();
    assert!(
        eventually(SETTLE, || events.receiver_count() >= 2),
        "the event pump must be subscribed before the event is sent ({} subscriber(s))",
        events.receiver_count()
    );

    // The AGP subscriber receives the server's projection of the same event.
    let mut raw = t.connect_raw();
    let response = raw_request(&t, &mut raw, 1, "subscribe_events", json!({}));
    let _subscription = subscription_id(&response, "subscribe_events({})");

    let app_id = AppId::from("org.example.correlation");
    let launched = expect_ok(
        t.block_on_timeout(client.launch_app(&app_id, &[])),
        "launch_app(org.example.correlation)",
    );
    assert_eq!(launched.app_id, app_id);

    // A `WindowCreated` with `launch_id: None`, exactly as the compositor emits
    // it (no AGP command feeds its local `WmBridge::note_launch`). The pid
    // matches the recorded launch, so the correlator's first tier applies.
    let window_id = WindowId(4242);
    let event = RuntimeEvent::WindowCreated {
        seq: 1_000_000,
        ts_ms: t.context().now_ms(),
        window_id,
        app_id: Some(app_id.clone()),
        pid: launched.pid,
        launch_id: None,
        title: Some("Correlation Fixture".into()),
    };
    events
        .send(event)
        .expect("the pump and the tap are subscribed");

    // 1. The compositor's own broadcast is not rewritten: the server cannot
    //    change what the compositor already published. `launch_app`'s
    //    `AppLaunched` is skipped.
    let published = t.block_on_timeout(async {
        loop {
            match compositor_tap.recv().await {
                Ok(RuntimeEvent::WindowCreated {
                    window_id: id,
                    launch_id,
                    ..
                }) if id == window_id => break launch_id,
                Ok(_) => continue,
                Err(error) => panic!("compositor tap closed before the event: {error}"),
            }
        }
    });
    assert_eq!(
        published, None,
        "the compositor's raw broadcast carries no launch_id; only the server projection does"
    );

    // 2. The fanned-out frame carries the correlator's verdict, so the observer,
    //    the inspection cache and the AGP subscriber all agree on `launch_id`.
    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("window_created"))
                && value.pointer("/data/window_id") == Some(&json!(4242))
        })
        .await
    });
    let frame: EventFrame = serde_json::from_value(frame)
        .unwrap_or_else(|error| panic!("window_created frame must decode: {error}"));
    let runtime = frame
        .to_runtime()
        .expect("window_created has a RuntimeEvent counterpart");
    let RuntimeEvent::WindowCreated {
        launch_id,
        app_id: reported,
        pid,
        ..
    } = runtime
    else {
        panic!("expected a window_created event, got {runtime:?}");
    };
    assert_eq!(launch_id, Some(launched.launch_id));
    assert_eq!(reported, Some(app_id));
    assert_eq!(pid, launched.pid);
}
