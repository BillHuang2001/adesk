//! AGP §5.9 notifications and §5.10 event waits — the runtime's programmable
//! event source and the agent's idle primitive.
//!
//! ## Coverage
//!
//! | Test | Protocol claim |
//! |---|---|
//! | `post_allocates_ids_and_list_is_newest_first` | `post_notification` answers a freshly allocated, monotonic `notification_id` and a `seq`; the stored notification fills the §5.9 defaults and appears in `list_notifications`, newest first. |
//! | `post_with_an_empty_title_is_invalid_request` | an empty `title` fails with `invalid_request`, stores nothing and leaves the connection usable. |
//! | `close_dismisses_and_a_second_close_publishes_nothing` | `close_notification` dismisses (gone from the default list, present with `include_dismissed` + its `close_reason`/`closed_seq`) and a second close is a successful no-op that publishes **nothing**: no `notification_closed` frame reaches a subscriber and a `wait_for_events` above its gap `seq` times out. |
//! | `invoke_action_succeeds_without_dismissing_and_validates_keys` | a known `action_key` succeeds and publishes a `notification_action` event without dismissing; an unknown key fails `invalid_request`; an unknown id fails `unknown_notification`. |
//! | `unknown_ids_answer_unknown_notification_with_the_id_in_data` | `close_notification`/`invoke_notification_action` on an unknown id answer `unknown_notification` with `ErrorPayload.data = {"notification_id": id}` and keep the connection open. |
//! | `each_notification_kind_reaches_its_subscriber` | a `subscribe_events` subscriber for each of the three notification kinds receives its matching frame with the exact `data` payload and the reserved `seq`. |
//! | `a_disjoint_subscriber_receives_nothing` | a subscription that filters to a disjoint kind receives neither the `notification` nor the `notification_action` frame. |
//! | `wait_for_events_resolves_on_a_notification_posted_while_waiting` | a `wait_for_events` outstanding while a notification is posted resolves with that event, typed through `adesk-client`. |
//! | `wait_for_events_reports_a_timeout_as_a_result` | a wait that sees nothing within `timeout_ms` answers `timed_out: true` with an empty `events` — a result, never an error. |
//! | `wait_for_events_honours_kinds_window_id_max_events_and_since_seq` | the wait's `window_id` excludes window-less events, `since_seq` sets the filter point and `max_events` caps the batch. |
//! | `notification_seqs_interleave_with_the_compositor_counter` | §1: the `notification`/`notification_action`/`notification_closed` events draw their `seq` from the compositor's one counter, bracketed by out-of-band `ReserveSeq` probes exactly as `sequence.rs` does for `app_launched`/`inspect_frame`. |
//!
//! ## How the assertions read the runtime
//!
//! Notifications are runtime-scoped, so one connection posts/closes/invokes
//! (the typed [`adesk_client::Client`]) while dedicated [`RawClient`]s hold the
//! §5.6 subscriptions that observe the published frames. A subscription is a
//! push stream, so "receives nothing" is asserted by reading a bounded
//! [`GRACE`]-long window (`RawClient::read_json`) after the mutating request has
//! answered; the pump fans the frame out on the compositor broadcast, so a frame
//! that was genuinely published arrives well inside it.
//!
//! No display, GPU, network or installed application is required: the runtime is
//! an empty pixman runtime and every event under test is server-synthesized.

use std::time::Duration;

use adesk_client::{AgpEvent, Client, EventKind, PostNotificationRequest, WaitForEventsRequest};
use adesk_compositor::RuntimeCommand;
use adesk_core::{
    ErrorCode, NotificationCloseReason, NotificationId, NotificationUrgency, RuntimeEvent, WindowId,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;

mod common;

use common::{
    assert_error_code, expect_ok, raw_request, subscription_id, TestRuntime, REQUEST_TIMEOUT,
    SHORT_TIMEOUT_MS,
};

/// How long a negative delivery assertion waits before concluding "nothing
/// arrived".
///
/// The §5.9 handlers publish on the compositor broadcast and the event pump fans
/// the frame out asynchronously, so a frame that *was* published could arrive a
/// few milliseconds late. Half a second is comfortably beyond that jitter while
/// keeping the suite fast; a broken runtime answers every such test in `GRACE`
/// rather than hanging.
const GRACE: Duration = Duration::from_millis(500);

/// The `timeout_ms` of a `wait_for_events` that a published event must wake.
///
/// Generous: a correct runtime answers the moment the event lands (well inside
/// this), so the long bound only exists so a broken filter fails with a clear
/// `timed_out` instead of hanging until [`REQUEST_TIMEOUT`].
const WAKE_TIMEOUT_MS: u64 = 5_000;

/// Posts a notification and unwraps the result.
///
/// # Panics
///
/// Panics with `what` and the full error if the runtime rejects the post.
fn post_ok(
    t: &TestRuntime,
    client: &Client,
    request: PostNotificationRequest,
    what: &str,
) -> adesk_client::PostNotificationResult {
    expect_ok(t.block_on_timeout(client.post_notification(request)), what)
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

/// Reserves the next event sequence number from the compositor's counter.
///
/// This is the production command, not a hand-picked number: it is exactly what
/// `reserve_seq` (`src/dispatch/windows.rs`) sends for every server-synthesized
/// event, only issued out of band through the public
/// [`adesk_compositor::CompositorHandle`]. [`RuntimeCommand::ReserveSeq`]
/// advances the shared counter and publishes nothing, which is what lets a test
/// bracket a synthesized event between two probes.
///
/// # Panics
///
/// Panics if the compositor refuses the command or drops its reply — both mean
/// the runtime is not serving, so the assertions would be meaningless.
fn probe_reserve(t: &TestRuntime) -> u64 {
    t.block_on_timeout(async {
        let (reply, answer) = oneshot::channel();
        t.context()
            .compositor
            .send(RuntimeCommand::ReserveSeq { reply })
            .expect("the compositor must accept a ReserveSeq probe");
        answer
            .await
            .expect("the compositor must answer every ReserveSeq it accepts")
    })
}

/// Subscribes a raw connection to exactly `kinds` and returns its id.
///
/// # Panics
///
/// Panics if `subscribe_events` answers anything but a subscription id.
fn subscribe_kinds(t: &TestRuntime, raw: &mut common::RawClient, id: u64, kinds: &[&str]) -> u64 {
    let response = raw_request(t, raw, id, "subscribe_events", json!({ "kinds": kinds }));
    subscription_id(&response, "subscribe_events")
}

#[test]
fn post_allocates_ids_and_list_is_newest_first() {
    let t = TestRuntime::start();
    let client = t.connect();

    let first = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("First")
            .body("first body")
            .source("tests")
            .urgency(NotificationUrgency::Low),
        "post_notification(First)",
    );
    let second = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Second"),
        "post_notification(Second)",
    );

    assert_ne!(
        first.notification_id, second.notification_id,
        "§5.9: every post allocates its own notification id"
    );
    assert!(
        second.notification_id > first.notification_id,
        "§5.9: notification ids are monotonic and never reused: {} then {}",
        first.notification_id,
        second.notification_id
    );
    assert!(
        second.seq > first.seq,
        "§1: each post's event reserves a strictly greater seq, got {} then {}",
        first.seq,
        second.seq
    );

    let list = expect_ok(
        t.block_on_timeout(client.list_notifications(false)),
        "list_notifications(false)",
    );
    assert_eq!(list.len(), 2, "both posted notifications must be listed");
    assert_eq!(
        list[0].id, second.notification_id,
        "§5.9: the list is newest first (`posted_seq` descending)"
    );
    assert_eq!(
        list[1].id, first.notification_id,
        "§5.9: the list is newest first (`posted_seq` descending)"
    );

    // The stored notification echoes the posted fields and fills the §5.9
    // defaults, so a consumer sees a complete record.
    let stored = &list[1];
    assert_eq!(stored.title, "First");
    assert_eq!(stored.body, "first body");
    assert_eq!(stored.source.as_deref(), Some("tests"));
    assert_eq!(stored.urgency, NotificationUrgency::Low);
    assert_eq!(
        stored.posted_seq, first.seq,
        "§5.9: `posted_seq` is the `notification` event's `seq`"
    );
    assert_eq!(stored.category, None, "§5.9: `category` defaults to `null`");
    assert!(stored.actions.is_empty(), "§5.9: `actions` default to `[]`");
    assert!(stored.hints.is_empty(), "§5.9: `hints` default to `{{}}`");
    assert_eq!(stored.timeout_ms, None);
    assert!(!stored.dismissed);
    assert_eq!(stored.closed_seq, None);
    assert_eq!(stored.close_reason, None);
    assert_eq!(
        list[0].urgency,
        NotificationUrgency::Normal,
        "§5.9: `urgency` defaults to `normal`"
    );
}

#[test]
fn post_with_an_empty_title_is_invalid_request() {
    let t = TestRuntime::start();
    let client = t.connect();

    assert_error_code(
        t.block_on_timeout(client.post_notification(PostNotificationRequest::new(""))),
        ErrorCode::InvalidRequest,
        "post_notification(empty title)",
    );

    // §6: an error never closes the connection.
    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after the rejected post",
    );

    let list = expect_ok(
        t.block_on_timeout(client.list_notifications(false)),
        "list_notifications(false)",
    );
    assert!(
        list.is_empty(),
        "§5.9: a rejected post stores nothing, got {list:?}"
    );
}

#[test]
fn close_dismisses_and_a_second_close_publishes_nothing() {
    let t = TestRuntime::start();
    let client = t.connect();

    let mut sub = t.connect_raw();
    let _subscription = subscribe_kinds(&t, &mut sub, 1, &["notification_closed"]);

    let posted = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Close me"),
        "post_notification(Close me)",
    );

    let closed = expect_ok(
        t.block_on_timeout(
            client.close_notification(posted.notification_id, NotificationCloseReason::Dismissed),
        ),
        "close_notification",
    );
    assert_eq!(closed.notification_id, posted.notification_id);
    assert!(
        closed.seq > posted.seq,
        "§1: the close reserves its own seq above the post, got {} then {}",
        posted.seq,
        closed.seq
    );

    // The dismissal is published exactly once, as a `notification_closed` frame.
    let frame = t.block_on_timeout(async {
        sub.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("notification_closed"))
        })
        .await
    });
    assert_eq!(
        frame
            .pointer("/data/notification_id")
            .and_then(Value::as_u64),
        Some(posted.notification_id.0),
        "§5.9: the close frame carries the notification id: {frame}"
    );
    assert_eq!(
        frame.pointer("/data/reason"),
        Some(&json!("dismissed")),
        "§5.9: the close frame carries the requested reason: {frame}"
    );
    assert_eq!(
        frame_seq(&frame, "notification_closed"),
        closed.seq,
        "§1: the frame's `seq` is the close result's `seq`"
    );

    // Default list: dismissed notifications are gone.
    let live = expect_ok(
        t.block_on_timeout(client.list_notifications(false)),
        "list_notifications(false)",
    );
    assert!(
        live.iter().all(|n| n.id != posted.notification_id),
        "§5.9: a dismissed notification is absent from the default list"
    );

    // `include_dismissed` reports it with its close metadata.
    let all = expect_ok(
        t.block_on_timeout(client.list_notifications(true)),
        "list_notifications(true)",
    );
    let dismissed = all
        .iter()
        .find(|n| n.id == posted.notification_id)
        .unwrap_or_else(|| panic!("§5.9: `include_dismissed` must report the closed one: {all:?}"));
    assert!(dismissed.dismissed);
    assert_eq!(
        dismissed.close_reason,
        Some(NotificationCloseReason::Dismissed)
    );
    assert_eq!(dismissed.closed_seq, Some(closed.seq));

    // A second close is a successful no-op that still reserves a (gap) seq.
    let again = expect_ok(
        t.block_on_timeout(
            client.close_notification(posted.notification_id, NotificationCloseReason::Dismissed),
        ),
        "second close_notification",
    );
    assert_eq!(again.notification_id, posted.notification_id);
    assert!(
        again.seq > closed.seq,
        "§5.9: the no-op close still reserves a seq (`newly_dismissed` gates publication, not \
         reservation), got {} then {}",
        closed.seq,
        again.seq
    );

    // Nothing was published: the subscriber sees no second frame.
    let stray = t.block_on_timeout(async { sub.read_json(GRACE).await });
    assert!(
        stray.is_none(),
        "§5.9: a no-op close publishes nothing, but a frame arrived: {stray:?}"
    );

    // And a wait above the no-op close's gap seq is never woken.
    let waited = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .kinds([EventKind::NotificationClosed])
                    .since_seq(again.seq)
                    .timeout_ms(SHORT_TIMEOUT_MS),
            ),
        ),
        "wait_for_events after the no-op close",
    );
    assert!(
        waited.timed_out,
        "§5.10: nothing is published above the no-op close's gap seq"
    );
    assert!(waited.events.is_empty());
}

#[test]
fn invoke_action_succeeds_without_dismissing_and_validates_keys() {
    let t = TestRuntime::start();
    let client = t.connect();

    let mut sub = t.connect_raw();
    let _subscription = subscribe_kinds(&t, &mut sub, 1, &["notification_action"]);

    let posted = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Act on me")
            .action("view", "View")
            .action("dismiss", "Dismiss"),
        "post_notification(Act on me)",
    );

    let invoked = expect_ok(
        t.block_on_timeout(client.invoke_notification_action(posted.notification_id, "view")),
        "invoke_notification_action(view)",
    );
    assert_eq!(invoked.notification_id, posted.notification_id);
    assert_eq!(invoked.action_key, "view");
    assert!(
        invoked.seq > posted.seq,
        "§1: the invocation reserves its own seq above the post"
    );

    let frame = t.block_on_timeout(async {
        sub.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("notification_action"))
        })
        .await
    });
    assert_eq!(
        frame
            .pointer("/data/notification_id")
            .and_then(Value::as_u64),
        Some(posted.notification_id.0),
        "§5.9: the action frame carries the notification id: {frame}"
    );
    assert_eq!(
        frame.pointer("/data/action_key"),
        Some(&json!("view")),
        "§5.9: the action frame carries the invoked key: {frame}"
    );
    assert_eq!(
        frame_seq(&frame, "notification_action"),
        invoked.seq,
        "§1: the frame's `seq` is the invocation result's `seq`"
    );

    // §5.9: an invocation never dismisses the notification.
    let list = expect_ok(
        t.block_on_timeout(client.list_notifications(false)),
        "list_notifications(false)",
    );
    assert_eq!(list.len(), 1, "the notification stays in the default list");
    assert_eq!(list[0].id, posted.notification_id);
    assert!(!list[0].dismissed, "§5.9: invocation does not dismiss");

    // An action key the notification does not declare is `invalid_request`.
    assert_error_code(
        t.block_on_timeout(
            client.invoke_notification_action(posted.notification_id, "not-a-button"),
        ),
        ErrorCode::InvalidRequest,
        "invoke_notification_action(unknown key)",
    );

    // An unknown id is `unknown_notification`.
    assert_error_code(
        t.block_on_timeout(
            client.invoke_notification_action(NotificationId::from(987_654), "view"),
        ),
        ErrorCode::UnknownNotification,
        "invoke_notification_action(unknown id)",
    );
}

#[test]
fn unknown_ids_answer_unknown_notification_with_the_id_in_data() {
    let t = TestRuntime::start();
    let mut raw = t.connect_raw();

    let response = raw_request(
        &t,
        &mut raw,
        1,
        "close_notification",
        json!({ "notification_id": 999 }),
    );
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("unknown_notification")),
        "§5.9/§6: an unknown id fails `unknown_notification`: {response}"
    );
    assert_eq!(
        response.pointer("/error/data/notification_id"),
        Some(&json!(999)),
        "§5.9: the unknown-notification error names the id it looked up: {response}"
    );

    let response = raw_request(
        &t,
        &mut raw,
        2,
        "invoke_notification_action",
        json!({ "notification_id": 1000, "action_key": "view" }),
    );
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("unknown_notification")),
        "§5.9: an unknown id fails `unknown_notification`: {response}"
    );
    assert_eq!(
        response.pointer("/error/data/notification_id"),
        Some(&json!(1000)),
        "§5.9: the unknown-notification error names the id it looked up: {response}"
    );

    // §6: both errors kept the connection open and answered a later request.
    let response = raw_request(&t, &mut raw, 3, "ping", json!({}));
    assert!(
        response.get("error").is_none(),
        "§6: an error response must not break the connection: {response}"
    );
}

#[test]
fn each_notification_kind_reaches_its_subscriber() {
    let t = TestRuntime::start();
    let client = t.connect();

    let mut notif_sub = t.connect_raw();
    let mut closed_sub = t.connect_raw();
    let mut action_sub = t.connect_raw();
    let _n = subscribe_kinds(&t, &mut notif_sub, 1, &["notification"]);
    let _c = subscribe_kinds(&t, &mut closed_sub, 1, &["notification_closed"]);
    let _a = subscribe_kinds(&t, &mut action_sub, 1, &["notification_action"]);

    let posted = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Ping").action("open", "Open"),
        "post_notification(Ping)",
    );

    // A subscriber to `notification` receives the frame with the §5.9 payload.
    let frame = t.block_on_timeout(async {
        notif_sub
            .expect_json_matching(REQUEST_TIMEOUT, |value| {
                value.get("event") == Some(&json!("notification"))
            })
            .await
    });
    assert_eq!(
        frame
            .pointer("/data/notification/id")
            .and_then(Value::as_u64),
        Some(posted.notification_id.0),
        "§5.9: the `notification` frame carries the stored notification: {frame}"
    );
    assert_eq!(
        frame.pointer("/data/notification/title"),
        Some(&json!("Ping")),
        "§5.9: the `notification` frame carries the title: {frame}"
    );
    assert_eq!(
        frame_seq(&frame, "notification"),
        posted.seq,
        "§1: the frame's `seq` is the post result's `seq`"
    );

    // A subscriber to `notification_action` receives the invocation.
    let invoked = expect_ok(
        t.block_on_timeout(client.invoke_notification_action(posted.notification_id, "open")),
        "invoke_notification_action(open)",
    );
    let frame = t.block_on_timeout(async {
        action_sub
            .expect_json_matching(REQUEST_TIMEOUT, |value| {
                value.get("event") == Some(&json!("notification_action"))
            })
            .await
    });
    assert_eq!(
        frame.pointer("/data/action_key"),
        Some(&json!("open")),
        "§5.9: the `notification_action` frame carries the invoked key: {frame}"
    );
    assert_eq!(frame_seq(&frame, "notification_action"), invoked.seq);

    // A subscriber to `notification_closed` receives the dismissal.
    let closed = expect_ok(
        t.block_on_timeout(
            client.close_notification(posted.notification_id, NotificationCloseReason::Closed),
        ),
        "close_notification(closed)",
    );
    let frame = t.block_on_timeout(async {
        closed_sub
            .expect_json_matching(REQUEST_TIMEOUT, |value| {
                value.get("event") == Some(&json!("notification_closed"))
            })
            .await
    });
    assert_eq!(
        frame.pointer("/data/reason"),
        Some(&json!("closed")),
        "§5.9: the `notification_closed` frame echoes the requested reason: {frame}"
    );
    assert_eq!(frame_seq(&frame, "notification_closed"), closed.seq);
}

#[test]
fn a_disjoint_subscriber_receives_nothing() {
    let t = TestRuntime::start();
    let client = t.connect();

    // A subscriber that only wants `notification_closed` must never see the
    // `notification` or `notification_action` frames published below.
    let mut disjoint = t.connect_raw();
    let _subscription = subscribe_kinds(&t, &mut disjoint, 1, &["notification_closed"]);

    let posted = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Filtered").action("open", "Open"),
        "post_notification(Filtered)",
    );
    expect_ok(
        t.block_on_timeout(client.invoke_notification_action(posted.notification_id, "open")),
        "invoke_notification_action(open)",
    );

    let stray = t.block_on_timeout(async { disjoint.read_json(GRACE).await });
    assert!(
        stray.is_none(),
        "§5.6: a `notification_closed`-only subscriber must not receive `notification` or \
         `notification_action` frames, but got: {stray:?}"
    );
}

#[test]
fn wait_for_events_resolves_on_a_notification_posted_while_waiting() {
    let t = TestRuntime::start();
    let waiter = t.connect();
    let poster = t.connect();

    let (wake, result) = t.block_on_timeout(async move {
        // A floor post fixes the filter point: the wait only counts events with
        // `seq` strictly above this, so it cannot resolve on anything already
        // published and must block until the wake-up post arrives.
        let floor = poster
            .post_notification(PostNotificationRequest::new("floor"))
            .await
            .expect("post_notification(floor) must succeed");

        let request = WaitForEventsRequest::new()
            .kinds([EventKind::Notification])
            .since_seq(floor.seq)
            .timeout_ms(WAKE_TIMEOUT_MS);
        let wait = tokio::spawn(async move { waiter.wait_for_events(request).await });

        // Give the wait task a chance to send its request and park before the
        // wake-up is posted. The assertion below holds regardless of scheduling
        // (`since_seq` fixes the filter point), but this makes the common path
        // exercise the genuinely-outstanding case.
        tokio::task::yield_now().await;

        let wake = poster
            .post_notification(PostNotificationRequest::new("wake-up"))
            .await
            .expect("post_notification(wake-up) must succeed");

        let result = wait
            .await
            .expect("the wait task must not panic")
            .expect("wait_for_events must answer");
        (wake, result)
    });

    assert!(
        !result.timed_out,
        "§5.10: the notification posted while the wait was outstanding must wake it"
    );
    assert_eq!(
        result.events.len(),
        1,
        "only the wake-up notification qualifies above the floor: {:?}",
        result.events
    );
    match &result.events[0] {
        AgpEvent::Runtime(RuntimeEvent::Notification {
            seq, notification, ..
        }) => {
            assert_eq!(
                *seq, wake.seq,
                "§5.10: the record's `seq` is the waking event's `seq`"
            );
            assert_eq!(notification.title, "wake-up");
            assert_eq!(notification.id, wake.notification_id);
        }
        other => panic!("expected a typed `notification` event, got {other:?}"),
    }
    assert!(
        result.seq >= wake.seq,
        "§5.10: the answered watermark is at least the waking event's `seq`"
    );
}

#[test]
fn wait_for_events_reports_a_timeout_as_a_result() {
    let t = TestRuntime::start();
    let client = t.connect();

    // Nothing is ever published on this empty runtime, so the wait must reach
    // its horizon and report that as a result, never an error.
    let result = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .kinds([EventKind::Notification])
                    .timeout_ms(SHORT_TIMEOUT_MS),
            ),
        ),
        "wait_for_events (no events)",
    );
    assert!(result.timed_out, "§5.10: the horizon was reached");
    assert!(
        result.events.is_empty(),
        "§5.10: a timed-out wait answers with no events, got {:?}",
        result.events
    );

    // The same on the wire: an empty `events` array and `timed_out: true`.
    let mut raw = t.connect_raw();
    let response = raw_request(
        &t,
        &mut raw,
        1,
        "wait_for_events",
        json!({ "kinds": ["notification"], "timeout_ms": SHORT_TIMEOUT_MS }),
    );
    assert!(
        response.get("error").is_none(),
        "§5.10: a timeout is a result, not an error: {response}"
    );
    assert_eq!(
        response.pointer("/result/timed_out"),
        Some(&json!(true)),
        "{response}"
    );
    assert_eq!(
        response.pointer("/result/events"),
        Some(&json!([])),
        "{response}"
    );
}

#[test]
fn wait_for_events_honours_kinds_window_id_max_events_and_since_seq() {
    let t = TestRuntime::start();
    let client = t.connect();

    let first = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("one"),
        "post_notification(one)",
    );
    let second = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("two"),
        "post_notification(two)",
    );
    assert!(second.seq > first.seq);

    // `window_id` restricts delivery to events carrying that window; the
    // notifications are window-less, so the filter excludes them and the wait
    // times out even though matching `kinds` exist in the journal.
    let scoped = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .window(WindowId(7))
                    .since_seq(0)
                    .timeout_ms(SHORT_TIMEOUT_MS),
            ),
        ),
        "wait_for_events(window_id)",
    );
    assert!(
        scoped.timed_out,
        "§5.10: `window_id` excludes window-less events (notifications must not be delivered)"
    );
    assert!(scoped.events.is_empty());

    // `since_seq = first.seq` moves the filter point past the first event, so
    // only the second qualifies.
    let only_second = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .kinds([EventKind::Notification])
                    .since_seq(first.seq)
                    .timeout_ms(WAKE_TIMEOUT_MS),
            ),
        ),
        "wait_for_events(since_seq = first)",
    );
    assert!(
        !only_second.timed_out,
        "§5.10: the second event is above the filter point"
    );
    assert_eq!(
        only_second.events.len(),
        1,
        "§5.10: only events with `seq > since_seq` count: {:?}",
        only_second.events
    );
    match &only_second.events[0] {
        AgpEvent::Runtime(RuntimeEvent::Notification { seq, .. }) => {
            assert_eq!(
                *seq, second.seq,
                "the surviving event is the newer notification"
            );
        }
        other => panic!("expected the newer `notification` event, got {other:?}"),
    }

    // `max_events` caps the batch: from the `0` filter point both notifications
    // qualify, but the cap answers with the oldest `max_events` of them.
    let capped = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .kinds([EventKind::Notification])
                    .since_seq(0)
                    .max_events(1)
                    .timeout_ms(WAKE_TIMEOUT_MS),
            ),
        ),
        "wait_for_events(max_events = 1)",
    );
    assert!(!capped.timed_out);
    assert_eq!(
        capped.events.len(),
        1,
        "§5.10: `max_events` caps the batch, got {:?}",
        capped.events
    );
    match &capped.events[0] {
        AgpEvent::Runtime(RuntimeEvent::Notification { seq, .. }) => {
            assert_eq!(
                *seq, first.seq,
                "§5.10: the batch is oldest-first, capped at the front"
            );
        }
        other => panic!("expected the oldest `notification` event, got {other:?}"),
    }

    // `since_seq` at the newest event's `seq` excludes everything: nothing is
    // strictly above the newest event.
    let drained = expect_ok(
        t.block_on_timeout(
            client.wait_for_events(
                WaitForEventsRequest::new()
                    .kinds([EventKind::Notification])
                    .since_seq(second.seq)
                    .timeout_ms(SHORT_TIMEOUT_MS),
            ),
        ),
        "wait_for_events(since_seq = newest)",
    );
    assert!(
        drained.timed_out,
        "§5.10: `since_seq` is exclusive, so the newest event does not wake the wait"
    );
    assert!(drained.events.is_empty());
}

#[test]
fn notification_seqs_interleave_with_the_compositor_counter() {
    let t = TestRuntime::start();
    let client = t.connect();

    let mut raw = t.connect_raw();
    let _subscription = subscribe_kinds(
        &t,
        &mut raw,
        1,
        &["notification", "notification_action", "notification_closed"],
    );

    // --- post -----------------------------------------------------------------

    let before_post = probe_reserve(&t);
    let posted = post_ok(
        &t,
        &client,
        PostNotificationRequest::new("Bracket").action("open", "Open"),
        "post_notification(Bracket)",
    );
    let after_post = probe_reserve(&t);
    assert!(
        before_post < posted.seq && posted.seq < after_post,
        "§1: the `notification` event's seq ({}) must sit strictly between the probes \
         ({before_post}, {after_post}) — it is reserved from the compositor's one counter",
        posted.seq
    );

    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("notification"))
        })
        .await
    });
    assert_eq!(
        frame_seq(&frame, "notification"),
        posted.seq,
        "§1: the fanned-out frame carries the reserved seq"
    );
    assert_eq!(
        frame
            .pointer("/data/notification/posted_seq")
            .and_then(Value::as_u64),
        Some(posted.seq),
        "§5.9: the stored `posted_seq` is the same reserved seq"
    );

    // --- invoke ---------------------------------------------------------------

    let before_action = probe_reserve(&t);
    let invoked = expect_ok(
        t.block_on_timeout(client.invoke_notification_action(posted.notification_id, "open")),
        "invoke_notification_action(open)",
    );
    let after_action = probe_reserve(&t);
    assert!(
        before_action < invoked.seq && invoked.seq < after_action,
        "§1: the `notification_action` event's seq ({}) must sit strictly between the probes \
         ({before_action}, {after_action})",
        invoked.seq
    );

    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("notification_action"))
        })
        .await
    });
    assert_eq!(frame_seq(&frame, "notification_action"), invoked.seq);

    // --- close ----------------------------------------------------------------

    let before_close = probe_reserve(&t);
    let closed = expect_ok(
        t.block_on_timeout(
            client.close_notification(posted.notification_id, NotificationCloseReason::Dismissed),
        ),
        "close_notification",
    );
    let after_close = probe_reserve(&t);
    assert!(
        before_close < closed.seq && closed.seq < after_close,
        "§1: the `notification_closed` event's seq ({}) must sit strictly between the probes \
         ({before_close}, {after_close})",
        closed.seq
    );

    let frame = t.block_on_timeout(async {
        raw.expect_json_matching(REQUEST_TIMEOUT, |value| {
            value.get("event") == Some(&json!("notification_closed"))
        })
        .await
    });
    assert_eq!(frame_seq(&frame, "notification_closed"), closed.seq);

    // The pairwise relations above are the requirement; the chain states them in
    // one place, in the order the runtime handed the numbers out. Gaps (a
    // no-op close, a rejected post) are allowed — reuse is not.
    assert!(
        before_post < posted.seq
            && posted.seq < after_post
            && after_post < before_action
            && before_action < invoked.seq
            && invoked.seq < after_action
            && after_action < before_close
            && before_close < closed.seq
            && closed.seq < after_close,
        "§1: the whole reservation order must be monotonic: probes {before_post} < {after_post} \
         < {before_action} < {after_action} < {before_close} < {after_close}, with the three \
         notification events interleaved strictly between them"
    );
}
