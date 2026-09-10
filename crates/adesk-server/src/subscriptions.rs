//! Event subscriptions (`docs/protocol.md` §5.6) and inspector streams (§5.7).
//!
//! Both registries are shared by all connections of one runtime and allocate
//! ids from a private monotonic counter: ids are unique and never reused
//! *within* a registry, but the event and inspector registries are independent
//! sequences (`ServerContext` creates one of each), so an id from
//! `subscribe_events` is not comparable with an id from `inspect_subscribe`.
//!
//! An empty `kinds` filter means "all kinds" (the §5.6 default `kinds = all`);
//! per-kind semantics — notably `surface_damage` counting only commits with a
//! non-empty damage region — live in [`adesk_proto::EventKind::matches`] and are
//! not reimplemented here.
//!
//! Event fan-out is non-blocking: a slow consumer loses event frames (never
//! responses) and its subscription is removed once the queue closes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use adesk_core::{OverlayKind, RuntimeEvent, WindowId};
use adesk_proto::{EventFrame, EventKind, Frame};
use tokio::sync::mpsc;

use crate::session::SessionId;

/// Subscription id, unique within its registry.
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

impl Subscription {
    /// Whether `event` passes both the kind and the window filter.
    ///
    /// An empty `kinds` filter matches every event; a `window_id` filter
    /// matches only events carrying that window, so window-less events
    /// (`app_launched`, `focus_changed` to `null`) are not delivered to a
    /// window-scoped subscription.
    fn matches(&self, event: &RuntimeEvent) -> bool {
        if !self.kinds.is_empty() && !self.kinds.iter().any(|kind| kind.matches(event)) {
            return false;
        }
        match self.window_id {
            None => true,
            Some(window_id) => event.window_id() == Some(window_id),
        }
    }
}

/// Monotonic id source shared by every clone of one registry.
///
/// The counter counts allocated ids, so the id handed out is the count plus
/// one: the first subscription of a registry is always `1`, and ids are never
/// reused even after `unsubscribe`.
#[derive(Clone, Default)]
struct IdAllocator {
    allocated: Arc<AtomicU64>,
}

impl IdAllocator {
    /// Allocates the next id (monotonic, never reused).
    fn next(&self) -> SubscriptionId {
        self.allocated.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Behavior a [`Registry`] expects from the entries it stores: an id for lookup
/// and removal, the owning connection, and the outbound sink.
trait RegistryEntry {
    /// Id handed to the client.
    fn entry_id(&self) -> SubscriptionId;
    /// Owning connection.
    fn entry_connection(&self) -> SessionId;
    /// Destination queue.
    fn entry_sink(&self) -> &EventSink;
}

/// Registry of id-stamped, connection-owned entries.
///
/// [`SubscriptionRegistry`] and [`InspectRegistry`] differ only in their payload
/// type, so the storage, the id allocation and the lock discipline live here
/// once; each wrapper adds just its `subscribe` constructor and its payload
/// accessor.
struct Registry<T> {
    inner: Arc<Mutex<Vec<T>>>,
    ids: IdAllocator,
}

impl<T> Clone for Registry<T> {
    fn clone(&self) -> Registry<T> {
        Registry {
            inner: Arc::clone(&self.inner),
            ids: self.ids.clone(),
        }
    }
}

impl<T> Default for Registry<T> {
    fn default() -> Registry<T> {
        Registry {
            inner: Arc::new(Mutex::new(Vec::new())),
            ids: IdAllocator::default(),
        }
    }
}

impl<T: RegistryEntry> Registry<T> {
    /// Stores an entry built from a freshly allocated id; returns that id.
    fn insert(&self, build: impl FnOnce(SubscriptionId) -> T) -> SubscriptionId {
        let id = self.ids.next();
        self.lock().push(build(id));
        id
    }

    /// Removes one entry; returns whether it existed.
    fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|entry| entry.entry_id() != id);
        entries.len() != before
    }

    /// Removes every entry of a connection; returns their ids.
    fn remove_connection(&self, connection: SessionId) -> Vec<SubscriptionId> {
        let mut entries = self.lock();
        let mut removed = Vec::new();
        entries.retain(|entry| {
            if entry.entry_connection() == connection {
                removed.push(entry.entry_id());
                false
            } else {
                true
            }
        });
        removed
    }

    /// Whether an entry with `id` is still registered (no payload clone).
    fn contains(&self, id: SubscriptionId) -> bool {
        self.lock().iter().any(|entry| entry.entry_id() == id)
    }

    /// Drops every entry whose sink is closed; returns how many were dropped.
    fn prune_closed(&self) -> usize {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|entry| !entry.entry_sink().is_closed());
        before - entries.len()
    }

    /// A snapshot of the live entries, for callers that need the payloads.
    fn list(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.lock().clone()
    }

    /// Number of live entries.
    fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether there are no entries.
    fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Locks the entry list, ignoring poisoning: a panic in another thread must
    /// not turn every later operation into a panic.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<T>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl RegistryEntry for Subscription {
    fn entry_id(&self) -> SubscriptionId {
        self.id
    }

    fn entry_connection(&self) -> SessionId {
        self.connection
    }

    fn entry_sink(&self) -> &EventSink {
        &self.sink
    }
}

/// Registry of event subscriptions shared by all connections.
#[derive(Clone, Default)]
pub struct SubscriptionRegistry {
    registry: Registry<Subscription>,
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
        self.registry.insert(|id| Subscription {
            id,
            connection,
            kinds,
            window_id,
            sink,
        })
    }

    /// Removes one subscription; returns whether it existed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        self.registry.unsubscribe(id)
    }

    /// Removes every subscription of a connection; returns their ids.
    pub fn remove_connection(&self, connection: SessionId) -> Vec<SubscriptionId> {
        self.registry.remove_connection(connection)
    }

    /// Delivers `event` to every matching subscription as an `EventFrame`.
    ///
    /// Uses `EventKind::matches` for filtering and non-blocking sends; a full
    /// queue drops the frame (the client observes a gap, `adesk-client` reports
    /// `Lagged`), a closed queue removes the subscription.
    pub fn fan_out(&self, event: &RuntimeEvent) {
        let mut subscriptions = self.registry.lock();
        if subscriptions.is_empty() {
            return;
        }
        // Built once, lazily, and cloned per matching subscriber.
        let mut frame: Option<Frame> = None;
        subscriptions.retain(|subscription| {
            if !subscription.matches(event) {
                return true;
            }
            let frame = frame.get_or_insert_with(|| Frame::Event(EventFrame::from_runtime(event)));
            match subscription.sink.try_send(frame.clone()) {
                // Full: the consumer is behind — drop the frame, keep the
                // subscription (the client sees a gap, not a silent disconnect).
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                // Closed: the connection is gone — forget the subscription.
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }

    /// Number of live subscriptions.
    pub fn len(&self) -> usize {
        self.registry.len()
    }

    /// Whether there are no subscriptions.
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
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

impl RegistryEntry for InspectSubscription {
    fn entry_id(&self) -> SubscriptionId {
        self.id
    }

    fn entry_connection(&self) -> SessionId {
        self.connection
    }

    fn entry_sink(&self) -> &EventSink {
        &self.sink
    }
}

/// Registry of inspector streams shared by all connections.
#[derive(Clone, Default)]
pub struct InspectRegistry {
    registry: Registry<InspectSubscription>,
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
        self.registry.insert(|id| InspectSubscription {
            id,
            connection,
            overlays,
            min_interval_ms,
            sink,
        })
    }

    /// Removes one stream; returns whether it existed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        self.registry.unsubscribe(id)
    }

    /// Removes every stream of a connection; returns their ids.
    pub fn remove_connection(&self, connection: SessionId) -> Vec<SubscriptionId> {
        self.registry.remove_connection(connection)
    }

    /// Whether a stream with `id` is still registered (no payload clone).
    pub(crate) fn contains(&self, id: SubscriptionId) -> bool {
        self.registry.contains(id)
    }

    /// Drops every stream whose sink is closed; returns how many were dropped.
    pub(crate) fn prune_closed(&self) -> usize {
        self.registry.prune_closed()
    }

    /// A snapshot of the live streams, for the throttled push loop.
    pub fn list(&self) -> Vec<InspectSubscription> {
        self.registry.list()
    }

    /// Number of live streams.
    pub fn len(&self) -> usize {
        self.registry.len()
    }

    /// Whether there are no streams.
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{Rect, Region};

    fn created(seq: u64, window: u64) -> RuntimeEvent {
        RuntimeEvent::WindowCreated {
            seq,
            ts_ms: seq * 10,
            window_id: WindowId(window),
            app_id: None,
            pid: None,
            launch_id: None,
            title: Some("t".to_owned()),
        }
    }

    fn commit(seq: u64, window: u64, damage: Region) -> RuntimeEvent {
        RuntimeEvent::SurfaceCommit {
            seq,
            ts_ms: seq * 10,
            window_id: WindowId(window),
            commit_seq: seq,
            damage,
        }
    }

    fn launched(seq: u64) -> RuntimeEvent {
        RuntimeEvent::AppLaunched {
            seq,
            ts_ms: seq * 10,
            launch_id: adesk_core::LaunchId(1),
            app_id: adesk_core::AppId::from("org.example.app"),
            pid: Some(42),
        }
    }

    fn focus_cleared(seq: u64) -> RuntimeEvent {
        RuntimeEvent::FocusChanged {
            seq,
            ts_ms: seq * 10,
            window_id: None,
        }
    }

    fn damaged() -> Region {
        Region::from_rect(Rect::new(0, 0, 8, 4))
    }

    fn event_frame(event: &RuntimeEvent) -> Frame {
        Frame::Event(EventFrame::from_runtime(event))
    }

    fn drain(rx: &mut mpsc::Receiver<Frame>) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            frames.push(frame);
        }
        frames
    }

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        let registry = SubscriptionRegistry::new();
        let (sink_a, _rx_a) = mpsc::channel(1);
        let (sink_b, _rx_b) = mpsc::channel(1);
        let (sink_c, _rx_c) = mpsc::channel(1);
        let (sink_d, _rx_d) = mpsc::channel(1);

        let first = registry.subscribe(1, Vec::new(), None, sink_a);
        let second = registry.subscribe(1, Vec::new(), None, sink_b);
        let third = registry.subscribe(2, Vec::new(), None, sink_c);
        assert_eq!((first, second, third), (1, 2, 3));

        assert!(registry.unsubscribe(second));
        assert_eq!(registry.subscribe(1, Vec::new(), None, sink_d), 4);
        assert!(!registry.unsubscribe(second), "ids are not reused");
    }

    #[test]
    fn clones_share_state_and_the_id_counter() {
        let registry = SubscriptionRegistry::new();
        let clone = registry.clone();
        let (sink_a, mut rx_a) = mpsc::channel(4);
        let (sink_b, _rx_b) = mpsc::channel(4);

        let first = registry.subscribe(1, Vec::new(), None, sink_a);
        let second = clone.subscribe(2, Vec::new(), None, sink_b);
        assert_eq!((first, second), (1, 2));
        assert_eq!(clone.len(), 2);

        clone.fan_out(&created(1, 1));
        assert_eq!(drain(&mut rx_a).len(), 1);
        assert_eq!(registry.remove_connection(1), vec![first]);
        assert_eq!(clone.len(), 1);
    }

    #[test]
    fn empty_kinds_match_every_event() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, Vec::new(), None, sink);

        let event = created(1, 1);
        registry.fan_out(&event);
        assert_eq!(drain(&mut rx), vec![event_frame(&event)]);

        let event = launched(2);
        registry.fan_out(&event);
        assert_eq!(drain(&mut rx), vec![event_frame(&event)]);
    }

    #[test]
    fn kind_filter_selects_only_matching_events() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, vec![EventKind::WindowCreated], None, sink);

        registry.fan_out(&commit(1, 1, damaged()));
        registry.fan_out(&launched(2));
        assert!(drain(&mut rx).is_empty());

        let event = created(3, 1);
        registry.fan_out(&event);
        assert_eq!(drain(&mut rx), vec![event_frame(&event)]);
    }

    #[test]
    fn surface_damage_matches_only_non_empty_damage() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, vec![EventKind::SurfaceDamage], None, sink);

        registry.fan_out(&commit(1, 1, Region::empty()));
        assert!(drain(&mut rx).is_empty());

        let event = commit(2, 1, damaged());
        registry.fan_out(&event);
        assert_eq!(drain(&mut rx), vec![event_frame(&event)]);
    }

    #[test]
    fn surface_commit_matches_every_commit() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, vec![EventKind::SurfaceCommit], None, sink);

        let empty = commit(1, 1, Region::empty());
        let damaged = commit(2, 1, damaged());
        registry.fan_out(&empty);
        registry.fan_out(&damaged);
        assert_eq!(
            drain(&mut rx),
            vec![event_frame(&empty), event_frame(&damaged)]
        );
    }

    #[test]
    fn protocol_only_kinds_never_match_runtime_events() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(
            1,
            vec![EventKind::Quiet, EventKind::InspectFrame],
            None,
            sink,
        );

        registry.fan_out(&created(1, 1));
        registry.fan_out(&commit(2, 1, damaged()));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn window_filter_scopes_to_one_window() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, Vec::new(), Some(WindowId(7)), sink);

        registry.fan_out(&created(1, 8));
        registry.fan_out(&commit(2, 8, damaged()));
        assert!(drain(&mut rx).is_empty());

        let event = commit(3, 7, damaged());
        registry.fan_out(&event);
        assert_eq!(drain(&mut rx), vec![event_frame(&event)]);
    }

    #[test]
    fn window_filter_excludes_windowless_events() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, Vec::new(), Some(WindowId(7)), sink);

        registry.fan_out(&launched(1));
        registry.fan_out(&focus_cleared(2));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn unscoped_subscription_receives_every_window() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(4);
        registry.subscribe(1, Vec::new(), None, sink);

        let first = created(1, 1);
        let second = created(2, 2);
        registry.fan_out(&first);
        registry.fan_out(&second);
        assert_eq!(
            drain(&mut rx),
            vec![event_frame(&first), event_frame(&second)]
        );
    }

    #[test]
    fn remove_connection_returns_its_ids_and_spares_other_connections() {
        let registry = SubscriptionRegistry::new();
        let (sink_a, mut rx_a) = mpsc::channel(4);
        let (sink_b, mut rx_b) = mpsc::channel(4);
        let (sink_c, mut rx_c) = mpsc::channel(4);

        let a = registry.subscribe(1, Vec::new(), None, sink_a);
        let b = registry.subscribe(1, Vec::new(), None, sink_b);
        let c = registry.subscribe(2, Vec::new(), None, sink_c);

        assert_eq!(registry.remove_connection(1), vec![a, b]);
        assert_eq!(registry.len(), 1);

        registry.fan_out(&created(1, 1));
        assert!(drain(&mut rx_a).is_empty());
        assert!(drain(&mut rx_b).is_empty());
        assert_eq!(drain(&mut rx_c).len(), 1);
        assert_eq!(registry.remove_connection(2), vec![c]);
        assert!(registry.remove_connection(2).is_empty());
    }

    #[test]
    fn full_queue_drops_the_frame_and_keeps_the_subscription() {
        let registry = SubscriptionRegistry::new();
        let (sink, mut rx) = mpsc::channel(1);
        let id = registry.subscribe(1, Vec::new(), None, sink);

        let first = created(1, 1);
        let second = created(2, 1);
        registry.fan_out(&first);
        registry.fan_out(&second); // capacity 1, receiver alive → dropped

        assert_eq!(
            registry.len(),
            1,
            "a full queue never removes the subscription"
        );
        assert_eq!(drain(&mut rx), vec![event_frame(&first)]);
        assert!(registry.unsubscribe(id));
    }

    #[test]
    fn closed_sink_removes_the_subscription() {
        let registry = SubscriptionRegistry::new();
        let (sink, rx) = mpsc::channel(4);
        let id = registry.subscribe(1, Vec::new(), None, sink);
        drop(rx);

        registry.fan_out(&created(1, 1));
        assert!(registry.is_empty());
        assert!(!registry.unsubscribe(id));
        assert!(registry.remove_connection(1).is_empty());
    }

    #[test]
    fn len_and_is_empty_track_live_subscriptions() {
        let registry = SubscriptionRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let (sink, _rx) = mpsc::channel(1);
        let id = registry.subscribe(1, Vec::new(), None, sink);
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);

        assert!(registry.unsubscribe(id));
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(!registry.unsubscribe(id), "unsubscribing twice is a no-op");
    }

    #[test]
    fn inspect_ids_are_per_registry_sequences() {
        let first = InspectRegistry::new();
        let second = InspectRegistry::new();
        let (sink_a, _rx_a) = mpsc::channel(1);
        let (sink_b, _rx_b) = mpsc::channel(1);

        assert_eq!(
            first.subscribe(1, vec![OverlayKind::Focus], 0, sink_a),
            1,
            "each registry allocates its own ids from 1"
        );
        assert_eq!(second.subscribe(1, vec![OverlayKind::Focus], 0, sink_b), 1);
        assert_eq!(
            first.subscribe(1, vec![OverlayKind::Damage], 100, {
                let (sink, _rx) = mpsc::channel(1);
                sink
            }),
            2
        );
    }

    #[test]
    fn inspect_list_is_a_snapshot_of_live_streams() {
        let registry = InspectRegistry::new();
        let (sink_a, _rx_a) = mpsc::channel(1);
        let (sink_b, _rx_b) = mpsc::channel(1);
        let a = registry.subscribe(1, vec![OverlayKind::WindowIds], 0, sink_a);
        let b = registry.subscribe(
            2,
            vec![OverlayKind::Cursor, OverlayKind::Focus],
            250,
            sink_b,
        );

        let snapshot = registry.list();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].id, a);
        assert_eq!(snapshot[0].connection, 1);
        assert_eq!(snapshot[0].overlays, vec![OverlayKind::WindowIds]);
        assert_eq!(snapshot[0].min_interval_ms, 0);
        assert_eq!(snapshot[1].id, b);
        assert_eq!(snapshot[1].connection, 2);
        assert_eq!(snapshot[1].min_interval_ms, 250);

        registry.unsubscribe(a);
        assert_eq!(registry.list().len(), 1);
        assert_eq!(snapshot.len(), 2, "list() returns a snapshot, not a view");
    }

    #[test]
    fn inspect_unsubscribe_and_remove_connection() {
        let registry = InspectRegistry::new();
        let (sink_a, _rx_a) = mpsc::channel(1);
        let (sink_b, _rx_b) = mpsc::channel(1);
        let (sink_c, _rx_c) = mpsc::channel(1);
        let a = registry.subscribe(1, Vec::new(), 0, sink_a);
        let b = registry.subscribe(1, Vec::new(), 0, sink_b);
        let c = registry.subscribe(2, Vec::new(), 0, sink_c);

        assert!(registry.unsubscribe(b));
        assert!(!registry.unsubscribe(b));
        assert_eq!(registry.remove_connection(1), vec![a]);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.remove_connection(3), Vec::<SubscriptionId>::new());
        assert!(registry.unsubscribe(c));
        assert!(registry.is_empty());
    }

    #[test]
    fn inspect_len_and_is_empty_track_live_streams() {
        let registry = InspectRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let (sink, _rx) = mpsc::channel(1);
        let id = registry.subscribe(1, Vec::new(), 0, sink);
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);

        assert!(registry.unsubscribe(id));
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn inspect_contains_reports_only_registered_ids() {
        let registry = InspectRegistry::new();
        let (sink, _rx) = mpsc::channel(1);
        let id = registry.subscribe(1, vec![OverlayKind::Focus], 0, sink);

        assert!(registry.contains(id));
        assert!(
            !registry.contains(id + 1),
            "an id that was never registered is not live"
        );

        registry.unsubscribe(id);
        assert!(
            !registry.contains(id),
            "an unsubscribed id is no longer live"
        );
    }

    #[test]
    fn prune_closed_drops_only_streams_whose_sink_is_gone() {
        let registry = InspectRegistry::new();
        let (live, _live_rx) = mpsc::channel(1);
        let (dead, dead_rx) = mpsc::channel(1);
        let live_id = registry.subscribe(1, Vec::new(), 0, live);
        let dead_id = registry.subscribe(2, Vec::new(), 0, dead);
        drop(dead_rx);

        assert_eq!(registry.prune_closed(), 1);
        assert!(registry.contains(live_id));
        assert!(!registry.contains(dead_id));
        assert_eq!(registry.len(), 1);

        assert_eq!(
            registry.prune_closed(),
            0,
            "pruning an all-live registry is a no-op"
        );
        assert_eq!(registry.len(), 1);
    }
}
