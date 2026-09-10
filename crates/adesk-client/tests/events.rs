//! Event streaming (§5.6): typed runtime events, local filtering, forward
//! compatibility, unsubscribe-on-drop, lag reporting and connection close.
//!
//! AGP event frames carry no subscription id, so the client demultiplexes
//! locally: every stream applies its own `EventFilter` on top of the server's
//! delivery (`CONTEXT.md` → "Design Decisions").

mod common;

use adesk_client::{
    AgpEvent, AgpEventStream, Client, ClientError, EventFilter, EventKind, EventStream,
    ImagePayload, QuietEvent,
};
use adesk_core::{Rect, RuntimeEvent, WindowId};
use common::{connect, MockServer, TIMEOUT};
use futures::StreamExt;
use serde_json::json;

/// One subscriber's event queue (`transport::EVENT_CHANNEL_CAPACITY`).
///
/// The fan-out drops events beyond this per-subscriber bound and reports the
/// count once as [`ClientError::Lagged`].
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// `subscribe_events` against the mock server, answering with `subscription_id`.
async fn subscribe(
    server: &mut MockServer,
    client: &Client,
    filter: EventFilter,
    subscription_id: u64,
) -> EventStream {
    let request = client.subscribe_events(filter);
    let (stream, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(request, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "subscribe_events");
            server
                .respond(id, json!({ "subscription_id": subscription_id }))
                .await;
        })
    })
    .await
    .expect("subscribe_events answers instead of hanging");
    stream.expect("subscribe_events succeeds")
}

/// Await the next typed stream item, failing the test (instead of hanging) on
/// timeout and treating the end of the stream as a failure.
async fn next_event(stream: &mut EventStream) -> Result<RuntimeEvent, ClientError> {
    tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("the stream yields an item instead of hanging")
        .expect("the stream is still open")
}

/// `subscribe_frames` against the mock server, answering with `subscription_id`.
async fn subscribe_frames(
    server: &mut MockServer,
    client: &Client,
    filter: EventFilter,
    subscription_id: u64,
) -> AgpEventStream {
    let request = client.subscribe_frames(filter);
    let (stream, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(request, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "subscribe_events");
            server
                .respond(id, json!({ "subscription_id": subscription_id }))
                .await;
        })
    })
    .await
    .expect("subscribe_frames answers instead of hanging");
    stream.expect("subscribe_frames succeeds")
}

/// Await the next `AgpEvent` frame, failing the test (instead of hanging) on
/// timeout and treating the end of the stream as a failure.
async fn next_frame(stream: &mut AgpEventStream) -> Result<AgpEvent, ClientError> {
    tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("the stream yields an item instead of hanging")
        .expect("the stream is still open")
}

/// `subscribe_events` yields typed `RuntimeEvent`s.
///
/// Answer 'subscribe_events' with 'subscription_id', then emit a
/// 'surface_commit' frame (data carrying 'window_id', 'commit_seq', 'damage')
/// and assert the stream's next item is `RuntimeEvent::SurfaceCommit` with the
/// same fields; emit 'focus_changed' and assert `RuntimeEvent::FocusChanged`.
#[tokio::test]
async fn subscribe_yields_typed_runtime_events() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(&mut server, &client, EventFilter::all(), 1).await;
    assert_eq!(
        stream.subscription_id(),
        1,
        "the server-assigned id is kept"
    );

    // Envelope carries seq/ts_ms; `data` carries the variant fields (§5.6).
    server
        .emit_event(
            "surface_commit",
            41,
            1200,
            json!({
                "window_id": 17,
                "commit_seq": 8291,
                "damage": [{ "x": 0, "y": 0, "w": 640, "h": 400 }],
            }),
        )
        .await;

    match next_event(&mut stream)
        .await
        .expect("the surface_commit event is typed")
    {
        RuntimeEvent::SurfaceCommit {
            seq,
            ts_ms,
            window_id,
            commit_seq,
            damage,
        } => {
            assert_eq!(seq, 41, "the envelope seq is preserved");
            assert_eq!(ts_ms, 1200, "the envelope ts_ms is preserved");
            assert_eq!(window_id, WindowId(17));
            assert_eq!(commit_seq, 8291);
            assert_eq!(damage.rects(), &[Rect::new(0, 0, 640, 400)]);
        }
        other => panic!("expected RuntimeEvent::SurfaceCommit, got {other:?}"),
    }

    server
        .emit_event("focus_changed", 42, 1300, json!({ "window_id": 17 }))
        .await;

    match next_event(&mut stream)
        .await
        .expect("the focus_changed event is typed")
    {
        RuntimeEvent::FocusChanged {
            seq,
            ts_ms,
            window_id,
        } => {
            assert_eq!(seq, 42);
            assert_eq!(ts_ms, 1300);
            assert_eq!(window_id, Some(WindowId(17)));
        }
        other => panic!("expected RuntimeEvent::FocusChanged, got {other:?}"),
    }
}

/// Kind filtering is applied locally.
///
/// Subscribe with `EventFilter::kinds([EventKind::WindowCreated])`; the server
/// (deliberately ignoring the filter) emits 'surface_commit' then
/// 'window_created'; assert the stream skips the former and yields only
/// `RuntimeEvent::WindowCreated`, because local filtering is a superset of the
/// server's.
#[tokio::test]
async fn local_kind_filter_is_applied() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(
        &mut server,
        &client,
        EventFilter::kinds([EventKind::WindowCreated]),
        2,
    )
    .await;

    // The server ignores the filter and sends a commit the client never asked
    // for; only the window_created frame may reach the stream.
    server
        .emit_event(
            "surface_commit",
            7,
            100,
            json!({ "window_id": 17, "commit_seq": 1, "damage": [] }),
        )
        .await;
    server
        .emit_event(
            "window_created",
            8,
            110,
            json!({
                "window_id": 17,
                "app_id": "org.example.Editor",
                "pid": 4242,
                "launch_id": 3,
                "title": "Untitled",
            }),
        )
        .await;

    match next_event(&mut stream)
        .await
        .expect("the window_created event is typed")
    {
        RuntimeEvent::WindowCreated {
            seq,
            ts_ms,
            window_id,
            ..
        } => {
            assert_eq!(
                seq, 8,
                "the earlier surface_commit was filtered out locally"
            );
            assert_eq!(ts_ms, 110);
            assert_eq!(window_id, WindowId(17));
        }
        other => panic!("expected RuntimeEvent::WindowCreated, got {other:?}"),
    }
}

/// Window filtering is applied locally.
///
/// Subscribe with `EventFilter::all().window(WindowId 17)`; emit
/// 'surface_commit' events for windows 17 and 18; assert only window 17's
/// event is yielded and the event for 18 is skipped.
#[tokio::test]
async fn local_window_filter_is_applied() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(
        &mut server,
        &client,
        EventFilter::all().window(WindowId(17)),
        4,
    )
    .await;

    // Window 18 is emitted first, so yielding 17 first proves it was skipped.
    server
        .emit_event(
            "surface_commit",
            11,
            200,
            json!({ "window_id": 18, "commit_seq": 5, "damage": [] }),
        )
        .await;
    server
        .emit_event(
            "surface_commit",
            12,
            210,
            json!({ "window_id": 17, "commit_seq": 6, "damage": [] }),
        )
        .await;

    let event = next_event(&mut stream)
        .await
        .expect("the window-17 event is typed");
    assert_eq!(
        event.window_id(),
        Some(WindowId(17)),
        "only window 17 is delivered"
    );
    match event {
        RuntimeEvent::SurfaceCommit {
            seq,
            window_id,
            commit_seq,
            ..
        } => {
            assert_eq!(seq, 12, "the window-18 event was filtered out locally");
            assert_eq!(window_id, WindowId(17));
            assert_eq!(commit_seq, 6);
        }
        other => panic!("expected RuntimeEvent::SurfaceCommit for window 17, got {other:?}"),
    }
}

/// `subscribe_frames` surfaces `inspect_frame` and unknown kinds.
///
/// Subscribe with `subscribe_frames(EventFilter::all())`; emit an
/// 'inspect_frame' whose data is an object with 'image' (an `ImagePayload`),
/// then an unknown event name (for example 'future_kind') with arbitrary data;
/// assert the items are `AgpEvent::InspectFrame` with the seq/ts_ms/image
/// preserved and `AgpEvent::Other` with name/seq/ts_ms/data preserved verbatim
/// (protocol §7 forward compatibility).
#[tokio::test]
async fn subscribe_frames_yields_inspect_and_other() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    // `subscribe_frames` is a client extension over `subscribe_events`.
    let mut stream = subscribe_frames(&mut server, &client, EventFilter::all(), 9).await;

    // A 1x1 RGBA8 payload: small, valid, and exactly comparable.
    let image = ImagePayload::from_rgba8(1, 1, &[0x11, 0x22, 0x33, 0xff], 1.0)
        .expect("build a 1x1 rgba8 payload");
    server
        .emit_event(
            "inspect_frame",
            21,
            300,
            json!({ "subscription_id": 9, "image": image.clone() }),
        )
        .await;

    let event = next_frame(&mut stream)
        .await
        .expect("the inspect_frame event is typed");
    match event {
        AgpEvent::InspectFrame(frame) => {
            assert_eq!(frame.seq, 21);
            assert_eq!(frame.ts_ms, 300);
            assert_eq!(frame.image, image, "the image payload is preserved");
        }
        other => panic!("expected AgpEvent::InspectFrame, got {other:?}"),
    }

    // Unknown kind: the lenient wire path must preserve name/seq/ts_ms/data.
    let data = json!({ "future_field": [1, 2, 3], "nested": { "ok": true } });
    server
        .emit_event("future_kind", 22, 310, data.clone())
        .await;

    let event = next_frame(&mut stream)
        .await
        .expect("an unknown kind is not an error");
    match event {
        AgpEvent::Other {
            name,
            seq,
            ts_ms,
            data: got,
        } => {
            assert_eq!(name, "future_kind");
            assert_eq!(seq, 22);
            assert_eq!(ts_ms, 310);
            assert_eq!(got, data, "unknown event data is preserved verbatim");
        }
        other => panic!("expected AgpEvent::Other, got {other:?}"),
    }
}

/// Dropping a stream sends `unsubscribe_events`.
///
/// Drop the `EventStream` and assert the server reads an 'unsubscribe_events'
/// request whose params are exactly 'subscription_id' equal to the id returned
/// by 'subscribe_events'. The send is best-effort but must happen on a live
/// connection.
#[tokio::test]
async fn dropping_stream_sends_unsubscribe() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let stream = subscribe(&mut server, &client, EventFilter::all(), 3).await;

    drop(stream);

    // The drop enqueues the request; awaiting the server's read lets the
    // writer task flush it on the still-live connection.
    let (_, method, params) = tokio::time::timeout(TIMEOUT, server.next_request())
        .await
        .expect("the dropped stream sends unsubscribe_events instead of hanging");
    assert_eq!(method, "unsubscribe_events");
    assert_eq!(
        params,
        json!({ "subscription_id": 3 }),
        "the dropped stream cancels exactly its own subscription"
    );
    assert!(
        !client.is_closed(),
        "unsubscribe-on-drop happens on a live connection"
    );
}

/// A lagging subscriber gets `ClientError::Lagged`.
///
/// Subscribe and then emit more events than the connection's broadcast
/// capacity without polling the stream; assert the next item is
/// `Err(ClientError::Lagged)` with a non-zero 'skipped' count, and that the
/// stream stays usable afterwards (a later event is still delivered) instead of
/// stalling the reader.
#[tokio::test]
async fn lag_reports_skipped_events() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(&mut server, &client, EventFilter::all(), 5).await;

    // Never poll while emitting: the per-subscriber queue must overflow.
    let overflow = 300;
    let emitted = EVENT_CHANNEL_CAPACITY + overflow;
    for index in 0..emitted {
        let seq = index as u64 + 1;
        server
            .emit_event(
                "surface_commit",
                seq,
                1000 + seq,
                json!({ "window_id": 17, "commit_seq": seq, "damage": [] }),
            )
            .await;
    }

    // Synchronise: the response is written after every event, and the reader
    // dispatches frames in order, so once it resolves the reader has processed
    // all of them (and dropped the overflow).
    let sync = client.list_windows();
    let (result, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(sync, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "list_windows");
            server
                .respond(id, json!({ "windows": [], "active_window_id": null }))
                .await;
        })
    })
    .await
    .expect("the sync request answers instead of hanging");
    result.expect("the sync request succeeds");

    match next_event(&mut stream).await {
        Err(ClientError::Lagged { skipped }) => {
            assert!(
                skipped > 0,
                "the overflow is counted, not hidden: {skipped}"
            );
        }
        other => panic!("expected ClientError::Lagged as the first item, got {other:?}"),
    }

    // The stream stays usable: one buffered item frees a queue slot, so the next
    // event is delivered rather than dropped.
    let buffered = next_event(&mut stream)
        .await
        .expect("the buffered backlog is still delivered");
    assert!(
        buffered.seq() <= emitted as u64,
        "the backlog is the pre-overflow events"
    );

    let marker_seq = 999_999;
    server
        .emit_event(
            "surface_commit",
            marker_seq,
            1_000_000,
            json!({ "window_id": 17, "commit_seq": marker_seq, "damage": [] }),
        )
        .await;

    // Drain the buffered backlog until the marker proves the stream still
    // delivers new events.
    let mut drained = 0usize;
    loop {
        match next_event(&mut stream).await {
            Ok(RuntimeEvent::SurfaceCommit { commit_seq, .. }) if commit_seq == marker_seq => break,
            Ok(_) => {
                drained += 1;
                assert!(
                    drained <= emitted,
                    "the marker event is delivered, not lost"
                );
            }
            Err(error) => panic!("unexpected error while draining the backlog: {error:?}"),
        }
    }
}

/// Connection close ends the stream with `ClientError::Closed`.
///
/// Close the server side while a stream is subscribed; assert the stream
/// yields `Err(ClientError::Closed)` once (after any buffered items) and then
/// `None`, rather than hanging forever.
#[tokio::test]
async fn connection_close_ends_stream_with_closed() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(&mut server, &client, EventFilter::all(), 6).await;

    // `MockServer::close` consumes the server; the stream outlives it.
    server.close().await;

    match next_event(&mut stream).await {
        Err(ClientError::Closed) => {}
        other => panic!("expected ClientError::Closed, got {other:?}"),
    }

    let end = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("the stream ends instead of hanging");
    assert!(
        end.is_none(),
        "the stream ends after reporting Closed once, got {end:?}"
    );
    assert!(client.is_closed(), "the connection reports the close");
}

/// A wire `quiet` frame is typed as `AgpEvent::Quiet`.
///
/// Subscribe with `subscribe_frames(EventFilter::all())`; emit a 'quiet' frame
/// whose data is `{"window_id": 17, "quiet_ms": 250}` and assert
/// `AgpEvent::Quiet` carries both fields plus the frame's exact `seq`/`ts_ms`;
/// emit a runtime-wide quiet (`window_id: null`) and assert `window_id: None`
/// with its own envelope; then emit a 'surface_damage' frame and assert it still
/// arrives as `AgpEvent::Other` with name/seq/ts_ms/data preserved (it is a
/// subscription-only filter alias that is never emitted as a typed frame).
///
/// The `Quiet` variant retains the envelope, so quiet frames are causally
/// orderable like `RuntimeEvent`/`InspectFrame` frames.
#[tokio::test]
async fn quiet_frame_is_typed() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe_frames(&mut server, &client, EventFilter::all(), 10).await;

    server
        .emit_event(
            "quiet",
            71,
            4000,
            json!({ "window_id": 17, "quiet_ms": 250 }),
        )
        .await;

    match next_frame(&mut stream)
        .await
        .expect("the quiet event is typed")
    {
        AgpEvent::Quiet { seq, ts_ms, event } => {
            assert_eq!(seq, 71, "the frame envelope is retained");
            assert_eq!(ts_ms, 4000, "the frame envelope is retained");
            assert_eq!(
                event,
                QuietEvent {
                    window_id: Some(WindowId(17)),
                    quiet_ms: 250,
                }
            );
        }
        other => panic!("expected AgpEvent::Quiet, got {other:?}"),
    }

    // Runtime-wide quiet: no window id.
    server
        .emit_event(
            "quiet",
            73,
            4200,
            json!({ "window_id": null, "quiet_ms": 100 }),
        )
        .await;

    match next_frame(&mut stream)
        .await
        .expect("the runtime-wide quiet event is typed")
    {
        AgpEvent::Quiet { seq, ts_ms, event } => {
            assert_eq!(seq, 73);
            assert_eq!(ts_ms, 4200);
            assert_eq!(event.window_id, None, "null window_id means the runtime");
            assert_eq!(event.quiet_ms, 100);
        }
        other => panic!("expected AgpEvent::Quiet, got {other:?}"),
    }
    // `surface_damage` stays untyped: a filter alias, not a frame kind.
    let damage = json!({
        "window_id": 17,
        "damage": [{ "x": 1, "y": 2, "w": 3, "h": 4 }],
    });
    server
        .emit_event("surface_damage", 74, 4300, damage.clone())
        .await;

    match next_frame(&mut stream)
        .await
        .expect("a surface_damage frame is not an error")
    {
        AgpEvent::Other {
            name,
            seq,
            ts_ms,
            data,
        } => {
            assert_eq!(name, "surface_damage");
            assert_eq!(seq, 74);
            assert_eq!(ts_ms, 4300);
            assert_eq!(data, damage, "the raw payload is preserved");
        }
        other => panic!("expected AgpEvent::Other for surface_damage, got {other:?}"),
    }
}

/// Typed `quiet` events participate in local kind and window filtering.
///
/// Subscribe with `EventFilter::kinds([EventKind::Quiet])`; emit a
/// 'surface_commit' (a non-matching kind) then a 'quiet'; assert only the quiet
/// is yielded. Then open a second subscription with
/// `EventFilter::all().window(WindowId 17)`; emit a quiet for window 18
/// (skipped) then one for window 17; assert the first item is the window-17
/// quiet, proving the other was filtered locally.
#[tokio::test]
async fn typed_quiet_is_locally_filtered() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let mut kinds = subscribe_frames(
        &mut server,
        &client,
        EventFilter::kinds([EventKind::Quiet]),
        11,
    )
    .await;

    server
        .emit_event(
            "surface_commit",
            81,
            5000,
            json!({ "window_id": 17, "commit_seq": 3, "damage": [] }),
        )
        .await;
    server
        .emit_event(
            "quiet",
            82,
            5010,
            json!({ "window_id": 17, "quiet_ms": 250 }),
        )
        .await;

    match next_frame(&mut kinds)
        .await
        .expect("the quiet event is typed")
    {
        AgpEvent::Quiet { event, .. } => {
            assert_eq!(event.window_id, Some(WindowId(17)));
            assert_eq!(event.quiet_ms, 250);
        }
        other => panic!("expected only AgpEvent::Quiet, got {other:?}"),
    }

    // A fresh subscription only sees events published after it exists.
    let mut window = subscribe_frames(
        &mut server,
        &client,
        EventFilter::all().window(WindowId(17)),
        12,
    )
    .await;

    server
        .emit_event(
            "quiet",
            83,
            5020,
            json!({ "window_id": 18, "quiet_ms": 250 }),
        )
        .await;
    server
        .emit_event(
            "quiet",
            84,
            5030,
            json!({ "window_id": 17, "quiet_ms": 250 }),
        )
        .await;

    match next_frame(&mut window)
        .await
        .expect("the window-17 quiet event is typed")
    {
        AgpEvent::Quiet { event, .. } => {
            assert_eq!(
                event.window_id,
                Some(WindowId(17)),
                "the window-18 quiet was filtered out locally"
            );
        }
        other => panic!("expected AgpEvent::Quiet for window 17, got {other:?}"),
    }
}

/// `EventStream` remains a `RuntimeEvent`-only view.
///
/// Subscribe with `subscribe_events(EventFilter::all())`; emit a 'quiet' frame
/// and then a 'surface_commit'; assert the first yielded item is the
/// `RuntimeEvent::SurfaceCommit` — the typed quiet frame is skipped, not
/// delivered (use `subscribe_frames` to observe it).
#[tokio::test]
async fn event_stream_skips_quiet_frames() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe(&mut server, &client, EventFilter::all(), 7).await;

    server
        .emit_event(
            "quiet",
            91,
            6000,
            json!({ "window_id": 17, "quiet_ms": 250 }),
        )
        .await;
    server
        .emit_event(
            "surface_commit",
            92,
            6010,
            json!({ "window_id": 17, "commit_seq": 9, "damage": [] }),
        )
        .await;

    match next_event(&mut stream)
        .await
        .expect("the surface_commit event is typed")
    {
        RuntimeEvent::SurfaceCommit {
            seq,
            ts_ms,
            commit_seq,
            ..
        } => {
            assert_eq!(seq, 92, "the quiet frame was skipped by EventStream");
            assert_eq!(ts_ms, 6010);
            assert_eq!(commit_seq, 9);
        }
        other => panic!("expected RuntimeEvent::SurfaceCommit, got {other:?}"),
    }
}

/// A malformed `quiet` payload falls back to `AgpEvent::Other`.
///
/// Emit a 'quiet' frame whose data cannot deserialise into `QuietEvent`
/// (`quiet_ms` is a string); assert `AgpEvent::Other` preserves the wire name
/// and the raw data verbatim instead of failing the event path (protocol §7
/// forward compatibility).
#[tokio::test]
async fn malformed_quiet_payload_falls_back_to_other() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let mut stream = subscribe_frames(&mut server, &client, EventFilter::all(), 13).await;

    let data = json!({ "window_id": 17, "quiet_ms": "soon" });
    server.emit_event("quiet", 95, 7000, data.clone()).await;

    match next_frame(&mut stream)
        .await
        .expect("a malformed quiet payload is not an error")
    {
        AgpEvent::Other {
            name,
            seq,
            ts_ms,
            data: got,
        } => {
            assert_eq!(name, "quiet");
            assert_eq!(seq, 95);
            assert_eq!(ts_ms, 7000);
            assert_eq!(got, data, "the raw payload is preserved");
        }
        other => panic!("expected AgpEvent::Other for a malformed quiet, got {other:?}"),
    }
}

/// `QuietEvent` is serde round-trippable through the client re-export.
///
/// Serialise a window quiet and a runtime-wide quiet; assert the exact JSON
/// shape (`{"window_id": <n|null>, "quiet_ms": <n>}`) and that deserialising
/// it back yields an equal value.
#[test]
fn quiet_event_serde_round_trip() {
    let window = QuietEvent {
        window_id: Some(WindowId(17)),
        quiet_ms: 250,
    };
    let value = serde_json::to_value(&window).expect("serialise a window quiet");
    assert_eq!(value, json!({ "window_id": 17, "quiet_ms": 250 }));
    let back: QuietEvent = serde_json::from_value(value).expect("deserialise a window quiet");
    assert_eq!(back, window);

    let runtime = QuietEvent {
        window_id: None,
        quiet_ms: 100,
    };
    let value = serde_json::to_value(&runtime).expect("serialise a runtime-wide quiet");
    assert_eq!(value, json!({ "window_id": null, "quiet_ms": 100 }));
    let back: QuietEvent = serde_json::from_value(value).expect("deserialise a runtime-wide quiet");
    assert_eq!(back, runtime);
}
