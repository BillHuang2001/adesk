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
use crate::{ProtoError, Result};

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
        match event {
            RuntimeEvent::SurfaceCommit { damage, .. } => match self {
                EventKind::SurfaceCommit => true,
                EventKind::SurfaceDamage => !damage.is_empty(),
                _ => false,
            },
            RuntimeEvent::WindowCreated { .. } => *self == EventKind::WindowCreated,
            RuntimeEvent::WindowDestroyed { .. } => *self == EventKind::WindowDestroyed,
            RuntimeEvent::WindowActivated { .. } => *self == EventKind::WindowActivated,
            RuntimeEvent::TitleChanged { .. } => *self == EventKind::TitleChanged,
            RuntimeEvent::FocusChanged { .. } => *self == EventKind::FocusChanged,
            RuntimeEvent::PopupAppeared { .. } => *self == EventKind::PopupAppeared,
            RuntimeEvent::PopupDisappeared { .. } => *self == EventKind::PopupDisappeared,
            RuntimeEvent::AppLaunched { .. } => *self == EventKind::AppLaunched,
        }
    }
}

/// The wire name of an event kind (`snake_case`, §5.6).
fn kind_name(kind: EventKind) -> &'static str {
    match kind {
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
        EventKind::InspectFrame => "inspect_frame",
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
/// §5.7 fixes this shape: the subscription that produced the frame, and the
/// composed output image with that subscription's overlays.
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
        match event {
            RuntimeEvent::WindowCreated {
                window_id,
                app_id,
                pid,
                launch_id,
                title,
                ..
            } => EventPayload::WindowCreated(WindowCreatedEvent {
                window_id: *window_id,
                app_id: app_id.clone(),
                pid: *pid,
                launch_id: *launch_id,
                title: title.clone(),
            }),
            RuntimeEvent::WindowDestroyed { window_id, .. } => {
                EventPayload::WindowDestroyed(WindowDestroyedEvent {
                    window_id: *window_id,
                })
            }
            RuntimeEvent::WindowActivated {
                window_id,
                previous,
                ..
            } => EventPayload::WindowActivated(WindowActivatedEvent {
                window_id: *window_id,
                previous: *previous,
            }),
            RuntimeEvent::TitleChanged {
                window_id, title, ..
            } => EventPayload::TitleChanged(TitleChangedEvent {
                window_id: *window_id,
                title: title.clone(),
            }),
            RuntimeEvent::SurfaceCommit {
                window_id,
                commit_seq,
                damage,
                ..
            } => EventPayload::SurfaceCommit(SurfaceCommitEvent {
                window_id: *window_id,
                commit_seq: *commit_seq,
                damage: damage.clone(),
            }),
            RuntimeEvent::FocusChanged { window_id, .. } => {
                EventPayload::FocusChanged(FocusChangedEvent {
                    window_id: *window_id,
                })
            }
            RuntimeEvent::PopupAppeared {
                window_id,
                popup_id,
                ..
            } => EventPayload::PopupAppeared(PopupAppearedEvent {
                window_id: *window_id,
                popup_id: *popup_id,
            }),
            RuntimeEvent::PopupDisappeared {
                window_id,
                popup_id,
                ..
            } => EventPayload::PopupDisappeared(PopupDisappearedEvent {
                window_id: *window_id,
                popup_id: *popup_id,
            }),
            RuntimeEvent::AppLaunched {
                launch_id,
                app_id,
                pid,
                ..
            } => EventPayload::AppLaunched(AppLaunchedEvent {
                launch_id: *launch_id,
                app_id: app_id.clone(),
                pid: *pid,
            }),
        }
    }

    /// Rebuilds the core runtime event, stamping `seq`/`ts_ms` onto it.
    ///
    /// Returns `None` for protocol-only kinds (`quiet`, `inspect_frame`), which
    /// have no [`RuntimeEvent`] counterpart.
    pub fn to_runtime(&self, seq: u64, ts_ms: u64) -> Option<RuntimeEvent> {
        Some(match self {
            EventPayload::WindowCreated(event) => RuntimeEvent::WindowCreated {
                seq,
                ts_ms,
                window_id: event.window_id,
                app_id: event.app_id.clone(),
                pid: event.pid,
                launch_id: event.launch_id,
                title: event.title.clone(),
            },
            EventPayload::WindowDestroyed(event) => RuntimeEvent::WindowDestroyed {
                seq,
                ts_ms,
                window_id: event.window_id,
            },
            EventPayload::WindowActivated(event) => RuntimeEvent::WindowActivated {
                seq,
                ts_ms,
                window_id: event.window_id,
                previous: event.previous,
            },
            EventPayload::TitleChanged(event) => RuntimeEvent::TitleChanged {
                seq,
                ts_ms,
                window_id: event.window_id,
                title: event.title.clone(),
            },
            EventPayload::SurfaceCommit(event) => RuntimeEvent::SurfaceCommit {
                seq,
                ts_ms,
                window_id: event.window_id,
                commit_seq: event.commit_seq,
                damage: event.damage.clone(),
            },
            EventPayload::FocusChanged(event) => RuntimeEvent::FocusChanged {
                seq,
                ts_ms,
                window_id: event.window_id,
            },
            EventPayload::PopupAppeared(event) => RuntimeEvent::PopupAppeared {
                seq,
                ts_ms,
                window_id: event.window_id,
                popup_id: event.popup_id,
            },
            EventPayload::PopupDisappeared(event) => RuntimeEvent::PopupDisappeared {
                seq,
                ts_ms,
                window_id: event.window_id,
                popup_id: event.popup_id,
            },
            EventPayload::AppLaunched(event) => RuntimeEvent::AppLaunched {
                seq,
                ts_ms,
                launch_id: event.launch_id,
                app_id: event.app_id.clone(),
                pid: event.pid,
            },
            EventPayload::Quiet(_) | EventPayload::InspectFrame(_) => return None,
        })
    }

    /// Decodes a `data` object for `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::InvalidEventData`] when `data` does not match the
    /// kind's schema, and [`ProtoError::Malformed`] for `surface_damage`, which
    /// is a filter alias and never an emitted kind.
    pub fn from_data(kind: EventKind, data: serde_json::Value) -> Result<EventPayload> {
        fn parse<T: serde::de::DeserializeOwned>(
            kind: EventKind,
            data: serde_json::Value,
        ) -> Result<T> {
            serde_json::from_value(data).map_err(|error| ProtoError::InvalidEventData {
                kind: kind_name(kind).to_owned(),
                message: error.to_string(),
            })
        }

        Ok(match kind {
            EventKind::WindowCreated => EventPayload::WindowCreated(parse(kind, data)?),
            EventKind::WindowDestroyed => EventPayload::WindowDestroyed(parse(kind, data)?),
            EventKind::WindowActivated => EventPayload::WindowActivated(parse(kind, data)?),
            EventKind::TitleChanged => EventPayload::TitleChanged(parse(kind, data)?),
            EventKind::SurfaceCommit => EventPayload::SurfaceCommit(parse(kind, data)?),
            EventKind::FocusChanged => EventPayload::FocusChanged(parse(kind, data)?),
            EventKind::PopupAppeared => EventPayload::PopupAppeared(parse(kind, data)?),
            EventKind::PopupDisappeared => EventPayload::PopupDisappeared(parse(kind, data)?),
            EventKind::AppLaunched => EventPayload::AppLaunched(parse(kind, data)?),
            EventKind::Quiet => EventPayload::Quiet(parse(kind, data)?),
            EventKind::InspectFrame => EventPayload::InspectFrame(parse(kind, data)?),
            EventKind::SurfaceDamage => {
                return Err(ProtoError::Malformed(
                    "`surface_damage` is a subscription filter alias, never an emitted event kind"
                        .to_owned(),
                ))
            }
        })
    }

    /// Encodes this payload as its `data` object.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`] if serialization fails.
    pub fn to_data(&self) -> Result<serde_json::Value> {
        Ok(match self {
            EventPayload::WindowCreated(event) => serde_json::to_value(event)?,
            EventPayload::WindowDestroyed(event) => serde_json::to_value(event)?,
            EventPayload::WindowActivated(event) => serde_json::to_value(event)?,
            EventPayload::TitleChanged(event) => serde_json::to_value(event)?,
            EventPayload::SurfaceCommit(event) => serde_json::to_value(event)?,
            EventPayload::FocusChanged(event) => serde_json::to_value(event)?,
            EventPayload::PopupAppeared(event) => serde_json::to_value(event)?,
            EventPayload::PopupDisappeared(event) => serde_json::to_value(event)?,
            EventPayload::AppLaunched(event) => serde_json::to_value(event)?,
            EventPayload::Quiet(event) => serde_json::to_value(event)?,
            EventPayload::InspectFrame(event) => serde_json::to_value(event)?,
        })
    }
}
