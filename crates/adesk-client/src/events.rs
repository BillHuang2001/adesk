//! Event streaming: subscription handles, filters, and the three stream views.
//!
//! AGP event frames carry no subscription id, so the client demultiplexes
//! **locally**: the reader task publishes every inbound event to a per-connection
//! broadcast channel, and each stream applies its own [`EventFilter`]. The
//! server-side filter of `subscribe_events` (protocol §5.6) is a delivery
//! optimisation, not a correctness requirement — local filtering is a superset.
//!
//! Three views are offered:
//!
//! | Type | Item | Use |
//! |---|---|---|
//! | [`EventStream`] | `Result<RuntimeEvent, ClientError>` | agent-facing runtime events |
//! | [`AgpEventStream`] | `Result<AgpEvent, ClientError>` | every event kind, incl. forward-compatible `Other` |
//! | [`InspectStream`] | `Result<InspectFrame, ClientError>` | `inspect_subscribe` frames |
//!
//! Dropping any stream cancels its subscription: a best-effort
//! `unsubscribe_events` is enqueued on the connection so the server stops
//! filtering for it. Streams are `Send + Unpin`, so they can be moved into
//! `tokio::spawn`ed tasks.

use std::pin::Pin;
use std::task::{Context, Poll};

use adesk_core::{EventKind as CoreEventKind, RuntimeEvent, WindowId};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ImagePayload, Result};

/// AGP event kind — the 9 `adesk_core::EventKind` values plus the two
/// subscription-only kinds (`surface_damage`, `quiet`) from protocol §5.6.
///
/// Used both as the `subscribe_events` filter (`kinds`) and for local
/// filtering of delivered frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventKind {
    /// A new xdg-toplevel was mapped.
    WindowCreated,
    /// A toplevel was unmapped/destroyed.
    WindowDestroyed,
    /// The active window changed.
    WindowActivated,
    /// A toplevel title changed.
    TitleChanged,
    /// Any commit on the window's surface tree.
    SurfaceCommit,
    /// Subscription-only: damage accumulated for a window (protocol §5.6).
    SurfaceDamage,
    /// Keyboard focus moved (possibly to `None`).
    FocusChanged,
    /// An xdg-popup was mapped.
    PopupAppeared,
    /// An xdg-popup was unmapped.
    PopupDisappeared,
    /// Subscription-only: the observer decided a window went quiet (§5.6).
    Quiet,
    /// The app registry spawned a process.
    AppLaunched,
}

impl EventKind {
    /// The AGP wire name (`snake_case`).
    pub fn as_str(&self) -> &'static str {
        todo!("map each variant to its snake_case wire name")
    }
}

impl From<CoreEventKind> for EventKind {
    fn from(kind: CoreEventKind) -> Self {
        todo!("map the 9 core event kinds onto the matching variants")
    }
}

/// Server-side subscription filter (`subscribe_events` params, protocol §5.6).
///
/// `None` fields are omitted from the params object, which the protocol defines
/// as "all kinds" / "all windows".
#[derive(Debug, Clone, Default, Serialize)]
#[non_exhaustive]
pub struct EventFilter {
    /// Event kinds to deliver; `None` means every kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<EventKind>>,
    /// Restrict delivery to one window; `None` means every window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

impl EventFilter {
    /// Deliver every event kind for every window.
    pub fn all() -> Self {
        Self { kinds: None, window_id: None }
    }

    /// Deliver only the given event kinds (for every window).
    pub fn kinds(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        Self { kinds: Some(kinds.into_iter().collect()), window_id: None }
    }

    /// Restrict an existing filter to one window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Whether a locally observed event satisfies this filter.
    pub(crate) fn matches(&self, event: &AgpEvent) -> bool {
        let _ = event;
        todo!("kinds + window_id predicate used by the stream implementations")
    }
}

/// One `inspect_frame` event (protocol §5.7).
///
/// The protocol does not spell out the data shape of `inspect_frame`; the
/// client assumes `{"image": ImagePayload}` (see `CONTEXT.md` → Known Issues).
#[derive(Debug, Clone)]
pub struct InspectFrame {
    /// Global monotonic event sequence.
    pub seq: u64,
    /// Monotonic milliseconds since runtime start.
    pub ts_ms: u64,
    /// Composed output image with the requested debug overlays.
    pub image: ImagePayload,
}

/// Every inbound event frame, including kinds with no `adesk_core::RuntimeEvent`
/// counterpart.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgpEvent {
    /// One of the 9 runtime events, typed by `adesk-core`.
    Runtime(RuntimeEvent),
    /// An `inspect_frame` produced by `inspect_subscribe`.
    InspectFrame(InspectFrame),
    /// An event the client does not model (forward compatibility, protocol §7).
    /// The raw frame is preserved so callers can still react to it.
    Other {
        /// Wire event name.
        name: String,
        /// Global monotonic event sequence.
        seq: u64,
        /// Monotonic milliseconds since runtime start.
        ts_ms: u64,
        /// Raw event payload.
        data: Value,
    },
}

/// Stream of agent-facing runtime events: `subscribe_events` with a filter that
/// selects the 9 core kinds.
///
/// Items are `Err(ClientError::Lagged { .. })` if this subscriber fell behind,
/// then `Err(ClientError::Closed)` (or `Protocol`) once when the connection
/// ends, then `None`. Non-core frames (e.g. `inspect_frame`) are skipped — use
/// [`AgpEventStream`] to see them.
pub struct EventStream {
    /// Per-connection broadcast receiver (shared with all streams).
    receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
    /// Local filter applied to every event.
    filter: EventFilter,
    /// Server-assigned subscription id, used for `unsubscribe_events`.
    subscription_id: u64,
    /// Connection handle used to cancel the subscription on drop.
    connection: std::sync::Arc<crate::transport::Connection>,
}

impl EventStream {
    /// Build a stream (crate-internal; consumers obtain one from
    /// [`Client::subscribe_events`](crate::Client::subscribe_events)).
    pub(crate) fn new(
        receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { receiver, filter, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for EventStream {
    type Item = Result<RuntimeEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let _ = cx;
        todo!("poll the broadcast receiver, filter, map to RuntimeEvent")
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // Best-effort: enqueue `unsubscribe_events`; a dead connection is fine.
        self.connection.unsubscribe_fire_and_forget(self.subscription_id);
    }
}

/// Stream of **all** AGP event frames (`subscribe_frames`), including
/// `inspect_frame` and unknown future kinds.
pub struct AgpEventStream {
    /// Per-connection broadcast receiver.
    receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
    /// Local filter applied to every event.
    filter: EventFilter,
    /// Server-assigned subscription id.
    subscription_id: u64,
    /// Connection handle used to cancel the subscription on drop.
    connection: std::sync::Arc<crate::transport::Connection>,
}

impl AgpEventStream {
    /// Build a stream (crate-internal).
    pub(crate) fn new(
        receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { receiver, filter, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for AgpEventStream {
    type Item = Result<AgpEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let _ = cx;
        todo!("poll the broadcast receiver and apply the filter")
    }
}

impl Drop for AgpEventStream {
    fn drop(&mut self) {
        self.connection.unsubscribe_fire_and_forget(self.subscription_id);
    }
}

/// Stream of `inspect_frame` events produced by `inspect_subscribe`.
pub struct InspectStream {
    /// Per-connection broadcast receiver.
    receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
    /// Server-assigned subscription id.
    subscription_id: u64,
    /// Connection handle used to cancel the subscription on drop.
    connection: std::sync::Arc<crate::transport::Connection>,
}

impl InspectStream {
    /// Build a stream (crate-internal).
    pub(crate) fn new(
        receiver: tokio::sync::broadcast::Receiver<AgpEvent>,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { receiver, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for InspectStream {
    type Item = Result<InspectFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let _ = cx;
        todo!("poll the broadcast receiver, keep only inspect_frame events")
    }
}

impl Drop for InspectStream {
    fn drop(&mut self) {
        self.connection.unsubscribe_fire_and_forget(self.subscription_id);
    }
}
