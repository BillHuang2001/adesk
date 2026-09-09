//! Event subscriptions (`docs/protocol.md` §5.6) and inspector streams (§5.7).
//!
//! Both registries are shared by all connections and keyed by a monotonic
//! server-wide id. Event fan-out is non-blocking: a slow consumer loses event
//! frames (never responses) and its subscription is removed when the queue
//! closes.

use std::sync::{Arc, Mutex};

use adesk_core::{OverlayKind, RuntimeEvent, WindowId};
use adesk_proto::{EventKind, Frame};
use tokio::sync::mpsc;

use crate::session::SessionId;

/// Server-wide subscription id.
pub type SubscriptionId = u64;

/// Queue receiving a connection's outbound frames.
pub type EventSink = mpsc::Sender<Frame>;

/// One `subscribe_events` subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Id returned to the client.
    pub id: SubscriptionId,
    /// Owning connection.
    pub connection: SessionId,
    /// Event kinds the client asked for (default: all subscribable kinds).
    pub kinds: Vec<EventKind>,
    /// Optional window filter.
    pub window_id: Option<WindowId>,
    /// Destination queue.
    pub sink: EventSink,
}

/// Registry of event subscriptions shared by all connections.
#[derive(Clone, Default)]
pub struct SubscriptionRegistry {
    inner: Arc<Mutex<Vec<Subscription>>>,
}

impl SubscriptionRegistry {
    /// An empty registry.
    pub fn new() -> SubscriptionRegistry {
        SubscriptionRegistry::default()
    }

    /// Registers a subscription and returns its id.
    pub fn subscribe(
        &self,
        connection: SessionId,
        kinds: Vec<EventKind>,
        window_id: Option<WindowId>,
        sink: EventSink,
    ) -> SubscriptionId {
        todo!()
    }

    /// Removes one subscription; returns whether it existed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        todo!()
    }

    /// Removes every subscription of a connection; returns their ids.
    pub fn remove_connection(&self, connection: SessionId) -> Vec<SubscriptionId> {
        todo!()
    }

    /// Delivers `event` to every matching subscription as an `EventFrame`.
    ///
    /// Uses `EventKind::matches` for filtering and non-blocking sends; a full
    /// queue drops the frame (the client observes a gap, `adesk-client` reports
    /// `Lagged`), a closed queue removes the subscription.
    pub fn fan_out(&self, event: &RuntimeEvent) {
        todo!()
    }

    /// Number of live subscriptions.
    pub fn len(&self) -> usize {
        todo!()
    }

    /// Whether there are no subscriptions.
    pub fn is_empty(&self) -> bool {
        todo!()
    }
}

/// One `inspect_subscribe` stream.
#[derive(Debug, Clone)]
pub struct InspectSubscription {
    /// Id returned to the client.
    pub id: SubscriptionId,
    /// Owning connection.
    pub connection: SessionId,
    /// Overlays to paint (canonicalized by `Inspector::new`).
    pub overlays: Vec<OverlayKind>,
    /// Minimum milliseconds between pushed frames (0 = every refresh).
    pub min_interval_ms: u64,
    /// Destination queue.
    pub sink: EventSink,
}

/// Registry of inspector streams shared by all connections.
#[derive(Clone, Default)]
pub struct InspectRegistry {
    inner: Arc<Mutex<Vec<InspectSubscription>>>,
}

impl InspectRegistry {
    /// An empty registry.
    pub fn new() -> InspectRegistry {
        InspectRegistry::default()
    }

    /// Registers a stream and returns its id.
    pub fn subscribe(
        &self,
        connection: SessionId,
        overlays: Vec<OverlayKind>,
        min_interval_ms: u64,
        sink: EventSink,
    ) -> SubscriptionId {
        todo!()
    }

    /// Removes one stream; returns whether it existed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        todo!()
    }

    /// Removes every stream of a connection; returns their ids.
    pub fn remove_connection(&self, connection: SessionId) -> Vec<SubscriptionId> {
        todo!()
    }

    /// A snapshot of the live streams, for the throttled push loop.
    pub fn list(&self) -> Vec<InspectSubscription> {
        todo!()
    }

    /// Number of live streams.
    pub fn len(&self) -> usize {
        todo!()
    }

    /// Whether there are no streams.
    pub fn is_empty(&self) -> bool {
        todo!()
    }
}
