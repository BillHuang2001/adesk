//! Event streaming: subscription handles, filters, and the three stream views.
//!
//! AGP event frames carry no subscription id, so the client demultiplexes
//! **locally**: the reader task publishes every inbound event to a per-connection
//! fan-out, and each stream applies its own [`EventFilter`]. The server-side
//! filter of `subscribe_events` (protocol §5.6) is a delivery optimisation, not
//! a correctness requirement — local filtering is a superset.
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

use crate::transport::EventReceiver;
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
        match self {
            EventKind::WindowCreated => "window_created",
            EventKind::WindowDestroyed => "window_destroyed",
            EventKind::WindowActivated => "window_activated",
            EventKind::TitleChanged => "title_changed",
            EventKind::SurfaceCommit => "surface_commit",
            EventKind::SurfaceDamage => "surface_damage",
            EventKind::FocusChanged => "focus_changed",
            EventKind::PopupAppeared => "popup_appeared",
            EventKind::PopupDisappeared => "popup_disappeared",
            EventKind::Quiet => "quiet",
            EventKind::AppLaunched => "app_launched",
        }
    }
}

impl From<CoreEventKind> for EventKind {
    fn from(kind: CoreEventKind) -> Self {
        match kind {
            CoreEventKind::WindowCreated => EventKind::WindowCreated,
            CoreEventKind::WindowDestroyed => EventKind::WindowDestroyed,
            CoreEventKind::WindowActivated => EventKind::WindowActivated,
            CoreEventKind::TitleChanged => EventKind::TitleChanged,
            CoreEventKind::SurfaceCommit => EventKind::SurfaceCommit,
            CoreEventKind::FocusChanged => EventKind::FocusChanged,
            CoreEventKind::PopupAppeared => EventKind::PopupAppeared,
            CoreEventKind::PopupDisappeared => EventKind::PopupDisappeared,
            CoreEventKind::AppLaunched => EventKind::AppLaunched,
        }
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
    ///
    /// Both halves must match: the kind must be selected (`None` = all kinds,
    /// and a kind the client cannot name never matches a `Some` filter), and an
    /// event carrying a window id must carry *this* window id. An event without
    /// a window id (for example `app_launched` or an `inspect_frame`) passes
    /// only when no window filter is set.
    pub(crate) fn matches(&self, event: &AgpEvent) -> bool {
        if let Some(wanted) = self.window_id {
            if event_window_id(event) != Some(wanted) {
                return false;
            }
        }
        match &self.kinds {
            None => true,
            Some(kinds) => event_kind(event).is_some_and(|kind| kinds.contains(&kind)),
        }
    }
}

/// The client-side kind of an event, when it has one.
fn event_kind(event: &AgpEvent) -> Option<EventKind> {
    match event {
        AgpEvent::Runtime(runtime) => Some(EventKind::from(runtime.kind())),
        // Known-but-untyped frames (`quiet`, `surface_damage`, future kinds):
        // the wire name is the only kind information available.
        AgpEvent::Other { name, .. } => {
            serde_json::from_value(Value::String(name.clone())).ok()
        }
        AgpEvent::InspectFrame(_) => None,
    }
}

/// The window an event belongs to, when it carries one.
fn event_window_id(event: &AgpEvent) -> Option<WindowId> {
    match event {
        AgpEvent::Runtime(runtime) => runtime.window_id(),
        AgpEvent::Other { data, .. } => {
            data.get("window_id").and_then(Value::as_u64).map(WindowId)
        }
        AgpEvent::InspectFrame(_) => None,
    }
}

/// Map one decoded wire event onto the crate's event vocabulary.
///
/// Called by the reader task. The nine core kinds become
/// [`AgpEvent::Runtime`]; `inspect_frame` becomes [`AgpEvent::InspectFrame`]
/// when its `data.image` fits [`ImagePayload`]; everything else — the
/// subscription-only kinds `surface_damage`/`quiet` and any kind this client
/// version does not know (protocol §7) — is preserved verbatim as
/// [`AgpEvent::Other`].
pub(crate) fn agp_event_from_raw(raw: crate::wire::RawEvent) -> AgpEvent {
    let crate::wire::RawEvent { name, seq, ts_ms, data } = raw;
    match name.as_str() {
        "inspect_frame" => {
            match data.get("image").cloned().map(serde_json::from_value::<ImagePayload>) {
                Some(Ok(image)) => AgpEvent::InspectFrame(InspectFrame { seq, ts_ms, image }),
                _ => AgpEvent::Other { name, seq, ts_ms, data },
            }
        }
        // Protocol-only kinds: no `RuntimeEvent` counterpart (see CONTEXT.md).
        "surface_damage" | "quiet" => AgpEvent::Other { name, seq, ts_ms, data },
        _ => match runtime_event(&name, seq, ts_ms, &data) {
            Some(event) => AgpEvent::Runtime(event),
            None => AgpEvent::Other { name, seq, ts_ms, data },
        },
    }
}

/// Rebuild a typed [`RuntimeEvent`] from an event envelope plus its `data`.
///
/// The wire frame hoists `seq`/`ts_ms` and names the kind in `event`; the core
/// event is internally tagged (`type`), so the three fields are merged into the
/// data object before deserialising. `None` when the data does not match any
/// core kind — the caller then keeps the raw frame.
fn runtime_event(name: &str, seq: u64, ts_ms: u64, data: &Value) -> Option<RuntimeEvent> {
    let Value::Object(fields) = data else {
        return None;
    };
    let mut object = fields.clone();
    object.insert("type".to_owned(), Value::String(name.to_owned()));
    object.insert("seq".to_owned(), Value::from(seq));
    object.insert("ts_ms".to_owned(), Value::from(ts_ms));
    serde_json::from_value(Value::Object(object)).ok()
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
    /// Per-connection event fan-out (shared with all streams).
    events: EventReceiver,
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
        events: EventReceiver,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { events, filter, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for EventStream {
    type Item = Result<RuntimeEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.events.poll_event(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(event))) => {
                    if !this.filter.matches(&event) {
                        continue;
                    }
                    if let AgpEvent::Runtime(runtime) = event {
                        return Poll::Ready(Some(Ok(runtime)));
                    }
                    // `inspect_frame` / unknown kinds have no typed core event.
                }
            }
        }
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
    /// Per-connection event fan-out.
    events: EventReceiver,
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
        events: EventReceiver,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { events, filter, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for AgpEventStream {
    type Item = Result<AgpEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.events.poll_event(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(event))) => {
                    if this.filter.matches(&event) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
            }
        }
    }
}

impl Drop for AgpEventStream {
    fn drop(&mut self) {
        self.connection.unsubscribe_fire_and_forget(self.subscription_id);
    }
}

/// Stream of `inspect_frame` events produced by `inspect_subscribe`.
pub struct InspectStream {
    /// Per-connection event fan-out.
    events: EventReceiver,
    /// Server-assigned subscription id.
    subscription_id: u64,
    /// Connection handle used to cancel the subscription on drop.
    connection: std::sync::Arc<crate::transport::Connection>,
}

impl InspectStream {
    /// Build a stream (crate-internal).
    pub(crate) fn new(
        events: EventReceiver,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self { events, subscription_id, connection }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.subscription_id
    }
}

impl Stream for InspectStream {
    type Item = Result<InspectFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.events.poll_event(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(event))) => {
                    if let AgpEvent::InspectFrame(frame) = event {
                        return Poll::Ready(Some(Ok(frame)));
                    }
                }
            }
        }
    }
}

impl Drop for InspectStream {
    fn drop(&mut self) {
        self.connection.unsubscribe_fire_and_forget(self.subscription_id);
    }
}
