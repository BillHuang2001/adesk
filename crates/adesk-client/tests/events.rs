//! Event streaming (§5.6): typed runtime events, local filtering, forward
//! compatibility, unsubscribe-on-drop, lag reporting and connection close.
//!
//! AGP event frames carry no subscription id, so the client demultiplexes
//! locally: every stream applies its own `EventFilter` on top of the server's
//! delivery (`CONTEXT.md` → "Design Decisions").

mod common;

/// `subscribe_events` yields typed `RuntimeEvent`s.
///
/// Answer 'subscribe_events' with 'subscription_id', then emit a
/// 'surface_commit' frame (data carrying 'window_id', 'commit_seq', 'damage')
/// and assert the stream's next item is `RuntimeEvent::SurfaceCommit` with the
/// same fields; emit 'focus_changed' and assert `RuntimeEvent::FocusChanged`.
#[tokio::test]
async fn subscribe_yields_typed_runtime_events() {
    todo!(
        "subscribe with EventFilter::all(); answer with subscription_id; \
         emit_event 'surface_commit' with window_id 17, commit_seq 8291 and a damage rect; \
         assert the stream yields Ok(RuntimeEvent::SurfaceCommit) with those fields; \
         then emit_event 'focus_changed' and assert Ok(RuntimeEvent::FocusChanged)"
    );
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
    todo!(
        "subscribe with EventFilter::kinds([EventKind::WindowCreated]); \
         emit_event 'surface_commit' for window 17, then emit_event 'window_created' for window 17; \
         assert the first stream item is the WindowCreated event and no SurfaceCommit event is yielded"
    );
}

/// Window filtering is applied locally.
///
/// Subscribe with `EventFilter::all().window(WindowId 17)`; emit
/// 'surface_commit' events for windows 17 and 18; assert only window 17's
/// event is yielded and the event for 18 is skipped.
#[tokio::test]
async fn local_window_filter_is_applied() {
    todo!(
        "subscribe with EventFilter::all().window(WindowId 17); \
         emit_event 'surface_commit' for window 18, then for window 17; \
         assert the first yielded item belongs to window 17 and the window-18 event never appears"
    );
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
    todo!(
        "call subscribe_frames(EventFilter::all()); \
         emit_event 'inspect_frame' with data holding an 'image' ImagePayload; \
         emit_event 'future_kind' with arbitrary data; \
         assert the items are AgpEvent::InspectFrame with matching seq/ts_ms/image and AgpEvent::Other with name 'future_kind', seq, ts_ms and data preserved"
    );
}

/// Dropping a stream sends `unsubscribe_events`.
///
/// Drop the `EventStream` and assert the server reads an 'unsubscribe_events'
/// request whose params are exactly 'subscription_id' equal to the id returned
/// by 'subscribe_events'. The send is best-effort but must happen on a live
/// connection.
#[tokio::test]
async fn dropping_stream_sends_unsubscribe() {
    todo!(
        "subscribe, answer with subscription_id 3, then drop the stream; \
         assert next_request() yields method 'unsubscribe_events' with params 'subscription_id' 3"
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
    todo!(
        "subscribe, then emit_event many more frames than the broadcast capacity while never polling; \
         assert the next item is Err(ClientError::Lagged) with skipped greater than zero; \
         emit one more event and assert the stream still yields it"
    );
}

/// Connection close ends the stream with `ClientError::Closed`.
///
/// Close the server side while a stream is subscribed; assert the stream
/// yields `Err(ClientError::Closed)` once (after any buffered items) and then
/// `None`, rather than hanging forever.
#[tokio::test]
async fn connection_close_ends_stream_with_closed() {
    todo!(
        "subscribe, answer with subscription_id, then MockServer::close; \
         assert the stream yields Err(ClientError::Closed) once and then None"
    );
}
