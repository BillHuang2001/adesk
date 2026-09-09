//! §5.6 event subscriptions.
//!
//! `subscribe_events` registers a filtered sink in
//! [`crate::subscriptions::SubscriptionRegistry`]; the event pump fans matching
//! `RuntimeEvent`s out as `EventFrame`s. `unsubscribe_events` is idempotent.

use adesk_proto::{
    ProtoError, SubscribeEventsParams, SubscribeEventsResult, UnsubscribeEventsParams,
    UnsubscribeEventsResult,
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
