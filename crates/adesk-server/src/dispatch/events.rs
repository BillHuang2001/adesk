//! §5.6 event subscriptions and the §5.10 event wait.
//!
//! `subscribe_events` registers a filtered sink in
//! [`crate::subscriptions::SubscriptionRegistry`]; the event pump fans matching
//! `RuntimeEvent`s out as `EventFrame`s. `unsubscribe_events` is idempotent.
//! `wait_for_events` is the pull counterpart: one request that answers once a
//! matching event has been published (or the timeout elapsed), driven by the
//! notification service's event inbox.

use adesk_notify::EventWaitSpec;
use adesk_proto::{
    ProtoError, SubscribeEventsParams, SubscribeEventsResult, UnsubscribeEventsParams,
    UnsubscribeEventsResult, WaitForEventsParams, WaitForEventsResult,
};

use crate::dispatch::RequestContext;
use crate::error::{Result, ServerError};

/// `subscribe_events`: filter by kind and optional window, push `event` frames.
///
/// # Errors
///
/// [`ServerError::Proto`] with [`ProtoError::InvalidParams`] (AGP
/// `invalid_request`) when the filter names a kind that cannot be subscribed
/// (`inspect_frame`, which only `inspect_subscribe` pushes), and
/// [`ServerError::Internal`] when the connection has no outbound sink.
pub async fn subscribe_events(
    ctx: &RequestContext<'_>,
    params: SubscribeEventsParams,
) -> Result<SubscribeEventsResult> {
    if let Some(kind) = params
        .kinds
        .iter()
        .copied()
        .find(|kind| !kind.is_subscribable())
    {
        return Err(ServerError::Proto(ProtoError::InvalidParams {
            method: "subscribe_events".to_owned(),
            message: format!("`{kind:?}` is not a subscribable event kind (§5.6)"),
        }));
    }

    let sink = super::session_sink(ctx.session)?;
    let subscription_id =
        ctx.server
            .subscriptions
            .subscribe(ctx.session.id(), params.kinds, params.window_id, sink);
    ctx.session.add_subscription(subscription_id);
    Ok(SubscribeEventsResult { subscription_id })
}

/// `unsubscribe_events`: stop pushing for a subscription id.
///
/// Idempotent: an unknown or already-cancelled id is not an error, and the
/// subscription is removed from both registries (ids are unique across event and
/// inspector subscriptions) as well as from the session's bookkeeping.
pub async fn unsubscribe_events(
    ctx: &RequestContext<'_>,
    params: UnsubscribeEventsParams,
) -> Result<UnsubscribeEventsResult> {
    let id = params.subscription_id;
    // Both registries are consulted unconditionally (`|`, not `||`).
    let removed =
        ctx.server.subscriptions.unsubscribe(id) | ctx.server.inspect_subscriptions.unsubscribe(id);
    let owned = ctx.session.remove_subscription(id);
    if !removed && !owned {
        tracing::debug!(subscription_id = id, "unsubscribe of an unknown id ignored");
    }
    Ok(UnsubscribeEventsResult {})
}

/// `wait_for_events`: block until a matching event is published or the timeout
/// elapses, then answer with the collected `EventRecord`s and the watermark.
///
/// The filter point is `since_seq` when given, else the inbox watermark captured
/// when the wait began (§5.10), so an idling agent that passes the previous
/// result's `seq` never misses an event. The inbox records every emitted event
/// kind (window lifecycle, commits, `app_launched`, notifications, ...); an empty
/// `kinds` filter means "every emitted kind" and `window_id` restricts delivery to
/// events carrying that window. A timeout is a normal answer
/// (`timed_out: true`, empty `events`), never an error.
///
/// The inbox yields `RuntimeEvent`s, so each is bridged to the wire `EventRecord`
/// through `crate::translate::event_record` — the same `data` mapping the §5.6
/// fan-out uses.
pub async fn wait_for_events(
    ctx: &RequestContext<'_>,
    params: WaitForEventsParams,
) -> Result<WaitForEventsResult> {
    let spec = EventWaitSpec {
        // The inbox filters by `adesk_core::EventKind`; the wire params carry
        // `adesk_proto::EventKind`. The bridge drops the protocol-only kinds that
        // a wait can never observe (`quiet`, `inspect_frame`) — an empty result
        // then means "every emitted kind", the §5.10 default.
        kinds: params
            .kinds
            .iter()
            .filter_map(|kind| crate::translate::proto_event_kind(*kind))
            .collect(),
        window_id: params.window_id,
        timeout_ms: params.timeout_ms,
        max_events: params.max_events,
        since_seq: params.since_seq,
    };
    let batch = ctx.server.notify.wait_for_events(spec).await?;
    let events = batch
        .events
        .iter()
        .map(crate::translate::event_record)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(WaitForEventsResult {
        events,
        timed_out: batch.timed_out,
        elapsed_ms: batch.elapsed_ms,
        seq: batch.seq,
    })
}
