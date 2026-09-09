//! AGP §5.6 — event subscriptions.

use serde::{Deserialize, Serialize};

use crate::api::Empty;
use crate::events::{AgpEventStream, EventFilter, EventStream};
use crate::{Client, Result};

/// `subscribe_events` / `inspect_subscribe` result.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SubscribeResult {
    pub(crate) subscription_id: u64,
}

/// `unsubscribe_events` params.
#[derive(Debug, Clone, Serialize)]
struct UnsubscribeParams {
    subscription_id: u64,
}

impl Client {
    /// `subscribe_events` — stream typed [`RuntimeEvent`](adesk_core::RuntimeEvent)s.
    ///
    /// `filter` is sent as the params object (`EventFilter` serialises to
    /// `{"kinds": [...], "window_id": N}`) and is *also* applied locally, so a
    /// stream only ever yields events it asked for. The returned [`EventStream`]
    /// cancels the server subscription when dropped. Items are
    /// [`ClientError::Lagged`](crate::ClientError::Lagged) if this subscriber
    /// falls behind; the stream ends once the connection closes.
    pub async fn subscribe_events(&self, filter: EventFilter) -> Result<EventStream> {
        let result: SubscribeResult = self.request("subscribe_events", &filter).await?;
        Ok(EventStream::new(
            self.inner.subscribe(),
            filter,
            result.subscription_id,
            self.inner.clone(),
        ))
    }

    /// `subscribe_events`, but yielding every AGP event frame — including
    /// `inspect_frame` and event kinds this client version does not model
    /// ([`AgpEvent::Other`](crate::AgpEvent::Other)).
    ///
    /// This is a client-side extension over the same protocol method; use it
    /// when forward compatibility matters more than the typed core vocabulary.
    pub async fn subscribe_frames(&self, filter: EventFilter) -> Result<AgpEventStream> {
        let result: SubscribeResult = self.request("subscribe_events", &filter).await?;
        Ok(AgpEventStream::new(
            self.inner.subscribe(),
            filter,
            result.subscription_id,
            self.inner.clone(),
        ))
    }

    /// `unsubscribe_events` — cancel a subscription by server-assigned id.
    ///
    /// Streams do this automatically on drop; call it explicitly only when the
    /// id was obtained elsewhere (or to stop delivery before dropping a
    /// stream).
    pub async fn unsubscribe_events(&self, subscription_id: u64) -> Result<()> {
        let _: Empty = self
            .request("unsubscribe_events", &UnsubscribeParams { subscription_id })
            .await?;
        Ok(())
    }
}
