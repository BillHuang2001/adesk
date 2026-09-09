//! §5.6 event subscriptions.
//!
//! `subscribe_events` registers a filtered sink in
//! [`crate::subscriptions::SubscriptionRegistry`]; the event pump fans matching
//! `RuntimeEvent`s out as `EventFrame`s. `unsubscribe_events` is idempotent.

use adesk_proto::{
    SubscribeEventsParams, SubscribeEventsResult, UnsubscribeEventsParams, UnsubscribeEventsResult,
};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `subscribe_events`: filter by kind and optional window, push `event` frames.
pub async fn subscribe_events(
    ctx: &RequestContext<'_>,
    params: SubscribeEventsParams,
) -> Result<SubscribeEventsResult> {
    todo!()
}

/// `unsubscribe_events`: stop pushing for a subscription id.
pub async fn unsubscribe_events(
    ctx: &RequestContext<'_>,
    params: UnsubscribeEventsParams,
) -> Result<UnsubscribeEventsResult> {
    todo!()
}
