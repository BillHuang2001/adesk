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
//! | [`AgpEventStream`] | `Result<AgpEvent, ClientError>` | every event kind, incl. typed `quiet` and forward-compatible `Other` |
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
use crate::{ImagePayload, QuietEvent, Result};

/// AGP event kind — the 9 `adesk_core::EventKind` values plus the two
/// protocol-only kinds from §5.6: `surface_damage` (a filter alias that is
/// never emitted as a frame) and `quiet` (emitted with a typed payload).
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
    /// The observer decided a window (or the whole runtime) went quiet (§5.6).
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
        Self {
            kinds: None,
            window_id: None,
        }
    }

    /// Deliver only the given event kinds (for every window).
    pub fn kinds(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        Self {
            kinds: Some(kinds.into_iter().collect()),
            window_id: None,
        }
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
        AgpEvent::Quiet { .. } => Some(EventKind::Quiet),
        // Known-but-untyped frames (`surface_damage`, future kinds): the wire
        // name is the only kind information available.
        AgpEvent::Other { name, .. } => kind_from_name(name),
        AgpEvent::InspectFrame(_) => None,
    }
}

/// Map a wire event name onto the client [`EventKind`], allocation-free.
///
/// The strings mirror [`EventKind`]'s `snake_case` serde representation. A name
/// the client does not model (`inspect_frame`, or any future kind, protocol §7)
/// yields `None`, so such a frame never matches a `Some` filter.
fn kind_from_name(name: &str) -> Option<EventKind> {
    match name {
        "window_created" => Some(EventKind::WindowCreated),
        "window_destroyed" => Some(EventKind::WindowDestroyed),
        "window_activated" => Some(EventKind::WindowActivated),
        "title_changed" => Some(EventKind::TitleChanged),
        "surface_commit" => Some(EventKind::SurfaceCommit),
        "surface_damage" => Some(EventKind::SurfaceDamage),
        "focus_changed" => Some(EventKind::FocusChanged),
        "popup_appeared" => Some(EventKind::PopupAppeared),
        "popup_disappeared" => Some(EventKind::PopupDisappeared),
        "quiet" => Some(EventKind::Quiet),
        "app_launched" => Some(EventKind::AppLaunched),
        _ => None,
    }
}

/// The window an event belongs to, when it carries one.
fn event_window_id(event: &AgpEvent) -> Option<WindowId> {
    match event {
        AgpEvent::Runtime(runtime) => runtime.window_id(),
        AgpEvent::Quiet { event, .. } => event.window_id,
        AgpEvent::Other { data, .. } => data.get("window_id").and_then(Value::as_u64).map(WindowId),
        AgpEvent::InspectFrame(_) => None,
    }
}

/// Map one decoded wire event onto the crate's event vocabulary.
///
/// Called by the reader task. The nine core kinds become
/// [`AgpEvent::Runtime`]; `quiet` becomes [`AgpEvent::Quiet`] when its `data`
/// fits [`QuietEvent`]; `inspect_frame` becomes [`AgpEvent::InspectFrame`] when
/// its `data.image` fits [`ImagePayload`]; everything else — the
/// subscription-only alias `surface_damage`, a payload that does not fit its
/// typed variant, and any kind this client version does not know (protocol §7)
/// — is preserved verbatim as [`AgpEvent::Other`].
pub(crate) fn agp_event_from_raw(raw: crate::wire::RawEvent) -> AgpEvent {
    let crate::wire::RawEvent {
        name,
        seq,
        ts_ms,
        mut data,
    } = raw;
    match name.as_str() {
        "inspect_frame" => {
            // Move `data.image` out rather than cloning it: an `inspect_frame`
            // image is multi-MiB of base64. A payload that does not fit leaves
            // a `null` image slot in the fallback below, but the frame is still
            // handed to `AgpEvent::Other` with its name, sequence and remaining
            // fields intact (protocol §7 forward compatibility).
            match data.get_mut("image").map(Value::take) {
                Some(image) => match serde_json::from_value::<ImagePayload>(image) {
                    Ok(image) => AgpEvent::InspectFrame(InspectFrame { seq, ts_ms, image }),
                    Err(_) => AgpEvent::Other {
                        name,
                        seq,
                        ts_ms,
                        data,
                    },
                },
                None => AgpEvent::Other {
                    name,
                    seq,
                    ts_ms,
                    data,
                },
            }
        }
        // `quiet` is emitted with a typed payload (§5.6); the frame envelope is
        // retained so the typed variant stays causally orderable. A payload this
        // client cannot read falls back to the raw frame (protocol §7).
        "quiet" => match serde_json::from_value::<QuietEvent>(data.clone()) {
            Ok(event) => AgpEvent::Quiet { seq, ts_ms, event },
            Err(_) => AgpEvent::Other {
                name,
                seq,
                ts_ms,
                data,
            },
        },
        // A subscription-only filter alias: no frame carries it.
        "surface_damage" => AgpEvent::Other {
            name,
            seq,
            ts_ms,
            data,
        },
        _ => match runtime_event(&name, seq, ts_ms, &data) {
            Some(event) => AgpEvent::Runtime(event),
            None => AgpEvent::Other {
                name,
                seq,
                ts_ms,
                data,
            },
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
/// The wire payload is `{"subscription_id": u64, "image": ImagePayload}`; this
/// type keeps the image and the frame envelope `seq`/`ts_ms`. The wire
/// `subscription_id` is not retained: the connection's event fan-out delivers
/// every frame to every stream, so it could not be used to demultiplex two
/// overlay sets anyway. A frame whose `image` does not fit becomes
/// [`AgpEvent::Other`].
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
    /// A typed `quiet` event (protocol §5.6): the observer saw no counted
    /// surface commit for the window (or the whole runtime) for `quiet_ms`.
    ///
    /// The frame envelope (`seq`/`ts_ms`) is retained alongside the shared wire
    /// payload ([`QuietEvent`]), so quiet frames are causally orderable with
    /// [`RuntimeEvent`] and [`InspectFrame`] frames.
    Quiet {
        /// Global monotonic event sequence.
        seq: u64,
        /// Monotonic milliseconds since runtime start.
        ts_ms: u64,
        /// The observer's quiet decision (window-specific or runtime-wide).
        event: QuietEvent,
    },
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

/// Shared plumbing for the three stream views: the per-connection event
/// fan-out, the local [`EventFilter`], the server-assigned subscription id, and
/// the best-effort `unsubscribe_events` on drop.
///
/// Each public stream ([`EventStream`], [`AgpEventStream`], [`InspectStream`])
/// is a thin wrapper that owns one of these and only maps the delivered event
/// onto its own item type.
struct Subscription {
    /// Per-connection event fan-out (shared with all streams).
    events: EventReceiver,
    /// Local filter applied to every event.
    filter: EventFilter,
    /// Server-assigned subscription id, used for `unsubscribe_events`.
    subscription_id: u64,
    /// Connection handle used to cancel the subscription on drop.
    connection: std::sync::Arc<crate::transport::Connection>,
}

impl Subscription {
    /// Build the shared subscription state.
    fn new(
        events: EventReceiver,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self {
            events,
            filter,
            subscription_id,
            connection,
        }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    fn subscription_id(&self) -> u64 {
        self.subscription_id
    }

    /// Poll until the next event that passes the local filter, discarding the
    /// rest. The subscription-level outcomes (lag/close/end) pass through
    /// unchanged, so every call stops at the first *filtered* event.
    fn poll_matching(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<AgpEvent>>> {
        loop {
            match self.events.poll_event(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(event))) => {
                    if self.filter.matches(&event) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Best-effort: enqueue `unsubscribe_events`; a dead connection is fine.
        self.connection
            .unsubscribe_fire_and_forget(self.subscription_id);
    }
}

/// Stream of agent-facing runtime events: `subscribe_events` with a filter that
/// selects the 9 core kinds.
///
/// Items are `Err(ClientError::Lagged { .. })` if this subscriber fell behind,
/// then `Err(ClientError::Closed)` (or `Protocol`) once when the connection
/// ends, then `None`. This is a [`RuntimeEvent`]-only view: frames without a
/// typed core event (`quiet`, `inspect_frame`, unknown future kinds) are
/// skipped even when the filter selects them — use [`AgpEventStream`]
/// (`subscribe_frames`) to observe those.
pub struct EventStream {
    /// Shared subscription state.
    inner: Subscription,
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
        Self {
            inner: Subscription::new(events, filter, subscription_id, connection),
        }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.inner.subscription_id()
    }
}

impl Stream for EventStream {
    type Item = Result<RuntimeEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.inner.poll_matching(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(event))) => {
                    if let AgpEvent::Runtime(runtime) = event {
                        return Poll::Ready(Some(Ok(runtime)));
                    }
                    // `quiet` / `inspect_frame` / unknown kinds have no typed
                    // core event.
                }
            }
        }
    }
}

/// Stream of **all** AGP event frames (`subscribe_frames`), including `quiet`,
/// `inspect_frame` and unknown future kinds.
pub struct AgpEventStream {
    /// Shared subscription state.
    inner: Subscription,
}

impl AgpEventStream {
    /// Build a stream (crate-internal).
    pub(crate) fn new(
        events: EventReceiver,
        filter: EventFilter,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self {
            inner: Subscription::new(events, filter, subscription_id, connection),
        }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.inner.subscription_id()
    }
}

impl Stream for AgpEventStream {
    type Item = Result<AgpEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.poll_matching(cx)
    }
}

/// Stream of `inspect_frame` events produced by `inspect_subscribe`.
pub struct InspectStream {
    /// Shared subscription state.
    inner: Subscription,
}

impl InspectStream {
    /// Build a stream (crate-internal).
    pub(crate) fn new(
        events: EventReceiver,
        subscription_id: u64,
        connection: std::sync::Arc<crate::transport::Connection>,
    ) -> Self {
        Self {
            // `inspect_frame` is never filtered locally: this view accepts every
            // frame the connection fans out and keeps only the ones it models.
            inner: Subscription::new(events, EventFilter::all(), subscription_id, connection),
        }
    }

    /// The server-assigned subscription id (for manual `unsubscribe_events`).
    pub fn subscription_id(&self) -> u64 {
        self.inner.subscription_id()
    }
}

impl Stream for InspectStream {
    type Item = Result<InspectFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.inner.poll_matching(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(AgpEvent::InspectFrame(frame)))) => {
                    return Poll::Ready(Some(Ok(frame)))
                }
                // Any other kind is not an `inspect_frame`: skip it.
                Poll::Ready(Some(Ok(_))) => {}
            }
        }
    }
}
