//! Event vocabulary: the subscription filter kind (§5.6) and the typed `data`
//! object of an event frame.
//!
//! Wire shape (§1): `{"event": <kind>, "seq": n, "ts_ms": n, "data": {...}}`.
//! `seq`/`ts_ms` live on the frame, `event` names the kind, and `data` carries
//! exactly the variant fields of the corresponding [`RuntimeEvent`] minus
//! `seq`/`ts_ms` — plus the two protocol-only kinds (`quiet`, `inspect_frame`).

use adesk_core::{AppId, LaunchId, Region, RuntimeEvent, WindowId};
use serde::{Deserialize, Serialize};

use crate::image::ImagePayload;
use crate::Result;

/// Event kind: the `event` field of an event frame and the `kinds` filter of
/// `subscribe_events` (§5.6).
///
/// The eleven variants of §5.6 are filterable ([`EventKind::SUBSCRIBABLE`]);
/// [`EventKind::InspectFrame`] is pushed only to `inspect_subscribe` subscribers
/// (§5.7) and must not appear in a subscription filter.
///
/// `SurfaceDamage` is a filter alias, never an emitted event name: it counts
/// [`RuntimeEvent::SurfaceCommit`] events whose damage region is non-empty, and
/// subscribers still receive frames named `surface_commit` (matching the §1
/// example, which carries `damage` inside a `surface_commit` event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A toplevel was mapped.
    WindowCreated,
    /// A toplevel was destroyed.
    WindowDestroyed,
    /// The active window changed.
    WindowActivated,
    /// A toplevel title changed.
    TitleChanged,
    /// Any commit on the window's surface tree (carries `damage`).
    SurfaceCommit,
    /// Filter alias: `SurfaceCommit` events with non-empty `damage`.
    SurfaceDamage,
    /// Keyboard focus moved (window or `null`).
    FocusChanged,
    /// A popup was mapped.
    PopupAppeared,
    /// A popup was unmapped.
    PopupDisappeared,
    /// Protocol-only: a window (or the runtime) has been quiet for `quiet_ms`.
    Quiet,
    /// The registry spawned a process.
    AppLaunched,
    /// Protocol-only: an inspector frame pushed by `inspect_subscribe` (§5.7).
    InspectFrame,
}

impl EventKind {
    /// The eleven filterable kinds of §5.6, in spec order.
    pub const SUBSCRIBABLE: [EventKind; 11] = [
        EventKind::WindowCreated,
        EventKind::WindowDestroyed,
        EventKind::WindowActivated,
        EventKind::TitleChanged,
        EventKind::SurfaceCommit,
        EventKind::SurfaceDamage,
        EventKind::FocusChanged,
        EventKind::PopupAppeared,
        EventKind::PopupDisappeared,
        EventKind::Quiet,
        EventKind::AppLaunched,
    ];

    /// Whether this kind is accepted in a `subscribe_events` filter (§5.6).
    pub const fn is_subscribable(&self) -> bool {
        !matches!(self, EventKind::InspectFrame)
    }

    /// Whether a subscription to this kind counts `event` (§5.6 semantics).
    ///
    /// `SurfaceDamage` counts only commits with non-empty damage; `SurfaceCommit`
    /// counts every commit; `Quiet`/`InspectFrame` are not derived from
    /// [`RuntimeEvent`]s and therefore never match one.
    pub fn matches(&self, event: &RuntimeEvent) -> bool {
        todo!()
    }
}

/// `data` of a `window_created` event (§5.2, §5.6).
///
/// `launch_id` is set when the window correlates to a `launch_app` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCreatedEvent {
    /// The new window.
    pub window_id: WindowId,
    /// Correlated application, when known.
    pub app_id: Option<AppId>,
    /// Process id, when known.
    pub pid: Option<i32>,
    /// Launch that created this window, when correlated.
    pub launch_id: Option<LaunchId>,
    /// Initial title, when known.
    pub title: Option<String>,
}

/// `data` of a `window_destroyed` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowDestroyedEvent {
    /// The destroyed window.
    pub window_id: WindowId,
}

/// `data` of a `window_activated` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowActivatedEvent {
    /// The newly active window.
    pub window_id: WindowId,
    /// Previously active window, if any.
    pub previous: Option<WindowId>,
}

/// `data` of a `title_changed` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleChangedEvent {
    /// The window whose title changed.
    pub window_id: WindowId,
    /// New title, or `null` when the title was cleared.
    pub title: Option<String>,
}

/// `data` of a `surface_commit` event (§1 example, §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceCommitEvent {
    /// The window whose surface tree committed.
    pub window_id: WindowId,
    /// Per-surface-tree commit counter (used by `since_commit` filters).
    pub commit_seq: u64,
    /// Damage of this commit, window-relative and clipped to the window.
    pub damage: Region,
}

/// `data` of a `focus_changed` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusChangedEvent {
    /// Window holding keyboard focus, or `null` when focus was cleared.
    pub window_id: Option<WindowId>,
}

/// `data` of a `popup_appeared` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopupAppearedEvent {
    /// The window owning the popup.
    pub window_id: WindowId,
    /// Popup id, unique per window.
    pub popup_id: u64,
}

/// `data` of a `popup_disappeared` event (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopupDisappearedEvent {
    /// The window owning the popup.
    pub window_id: WindowId,
    /// Popup id, unique per window.
    pub popup_id: u64,
}

/// `data` of an `app_launched` event (§5.2, §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLaunchedEvent {
    /// The launch this event belongs to.
    pub launch_id: LaunchId,
    /// The launched application.
    pub app_id: AppId,
    /// Spawned process id, when known.
    pub pid: Option<i32>,
}

/// `data` of a `quiet` event (protocol-only kind, §5.6).
///
/// The spec names the kind without fixing its fields; this is the resolved shape
/// (additive fields are not breaking, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietEvent {
    /// The window that went quiet, or `null` for the whole runtime.
    pub window_id: Option<WindowId>,
    /// The quiet threshold that was satisfied, in milliseconds.
    pub quiet_ms: u64,
}

/// `data` of an `inspect_frame` event pushed by `inspect_subscribe` (§5.7).
///
/// The spec names the kind without fixing its fields; this is the resolved shape
/// (additive fields are not breaking, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectFrameEvent {
    /// The `inspect_subscribe` subscription that produced this frame.
    pub subscription_id: u64,
    /// The composed output image with debug overlays.
    pub image: ImagePayload,
}

/// Typed `data` object of an event frame: one variant per event kind that can be
/// emitted.
///
/// `EventPayload` carries no serde impls of its own — the kind lives in the
/// frame, so decoding needs it; use [`EventPayload::from_data`] /
/// [`EventPayload::to_data`] or [`EventFrame`](crate::EventFrame).
#[derive(Debug, Clone, PartialEq)]
pub enum EventPayload {
    /// See [`WindowCreatedEvent`].
    WindowCreated(WindowCreatedEvent),
    /// See [`WindowDestroyedEvent`].
    WindowDestroyed(WindowDestroyedEvent),
    /// See [`WindowActivatedEvent`].
    WindowActivated(WindowActivatedEvent),
    /// See [`TitleChangedEvent`].
    TitleChanged(TitleChangedEvent),
    /// See [`SurfaceCommitEvent`].
    SurfaceCommit(SurfaceCommitEvent),
    /// See [`FocusChangedEvent`].
    FocusChanged(FocusChangedEvent),
    /// See [`PopupAppearedEvent`].
    PopupAppeared(PopupAppearedEvent),
    /// See [`PopupDisappearedEvent`].
    PopupDisappeared(PopupDisappearedEvent),
    /// See [`AppLaunchedEvent`].
    AppLaunched(AppLaunchedEvent),
    /// See [`QuietEvent`].
    Quiet(QuietEvent),
    /// See [`InspectFrameEvent`].
    InspectFrame(InspectFrameEvent),
}

impl EventPayload {
    /// The wire kind this payload is emitted as.
    pub fn kind(&self) -> EventKind {
        match self {
            EventPayload::WindowCreated(_) => EventKind::WindowCreated,
            EventPayload::WindowDestroyed(_) => EventKind::WindowDestroyed,
            EventPayload::WindowActivated(_) => EventKind::WindowActivated,
            EventPayload::TitleChanged(_) => EventKind::TitleChanged,
            EventPayload::SurfaceCommit(_) => EventKind::SurfaceCommit,
            EventPayload::FocusChanged(_) => EventKind::FocusChanged,
            EventPayload::PopupAppeared(_) => EventKind::PopupAppeared,
            EventPayload::PopupDisappeared(_) => EventKind::PopupDisappeared,
            EventPayload::AppLaunched(_) => EventKind::AppLaunched,
            EventPayload::Quiet(_) => EventKind::Quiet,
            EventPayload::InspectFrame(_) => EventKind::InspectFrame,
        }
    }

    /// Extracts the payload from a core runtime event.
    ///
    /// `seq`/`ts_ms` are dropped: they live on the event frame (§1).
    pub fn from_runtime(event: &RuntimeEvent) -> EventPayload {
        todo!()
    }

    /// Rebuilds the core runtime event, stamping `seq`/`ts_ms` onto it.
    ///
    /// Returns `None` for protocol-only kinds (`quiet`, `inspect_frame`), which
    /// have no [`RuntimeEvent`] counterpart.
    pub fn to_runtime(&self, seq: u64, ts_ms: u64) -> Option<RuntimeEvent> {
        todo!()
    }

    /// Decodes a `data` object for `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::InvalidEventData`](crate::ProtoError::InvalidEventData)
    /// when `data` does not match the kind's schema, and
    /// [`ProtoError::Malformed`](crate::ProtoError::Malformed) for
    /// `surface_damage`, which is a filter alias and never an emitted kind.
    pub fn from_data(kind: EventKind, data: serde_json::Value) -> Result<EventPayload> {
        todo!()
    }

    /// Encodes this payload as its `data` object.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`](crate::ProtoError::Json) if serialization fails.
    pub fn to_data(&self) -> Result<serde_json::Value> {
        todo!()
    }
}
