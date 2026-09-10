//! The runtime event vocabulary: compositor → server (`docs/architecture.md` §2).
//!
//! `seq` is globally monotonic across all events and assigned by the compositor
//! under one lock; `ts_ms` is monotonic milliseconds since compositor start.
//! `commit_seq` on [`RuntimeEvent::SurfaceCommit`] is the per-surface-tree commit
//! counter used for `since_commit` filters.
//!
//! Wire shape (internal): an internally tagged enum, e.g.
//! `{"type":"surface_commit","seq":8291,"ts_ms":51234,"window_id":17,
//!   "commit_seq":8291,"damage":[{"x":630,"y":220,"w":410,"h":180}]}`.
//! `adesk-proto` maps these into the external event frames of AGP.

use serde::{Deserialize, Serialize};

use crate::geometry::{Rect, Region};
use crate::ids::{ActionId, AppId, LaunchId, WindowId};

/// The kind of a [`RuntimeEvent`], without its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A new toplevel was mapped.
    WindowCreated,
    /// A toplevel was unmapped/destroyed.
    WindowDestroyed,
    /// The active (visible) window changed.
    WindowActivated,
    /// A toplevel title changed.
    TitleChanged,
    /// Any commit on the window's surface tree.
    SurfaceCommit,
    /// Keyboard focus moved (possibly to nothing).
    FocusChanged,
    /// An xdg-popup was mapped.
    PopupAppeared,
    /// An xdg-popup was unmapped.
    PopupDisappeared,
    /// The registry spawned a process.
    AppLaunched,
}

/// Everything the compositor reports to the rest of the runtime.
///
/// Events for one window are delivered in `seq` order; the broadcast channel
/// preserves global order for non-lagged receivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// A new xdg-toplevel was mapped.
    WindowCreated {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The new window.
        window_id: WindowId,
        /// App id resolved by correlation, when already known.
        app_id: Option<AppId>,
        /// Client process id, when known.
        pid: Option<i32>,
        /// Launch that produced this window, when correlated.
        launch_id: Option<LaunchId>,
        /// Initial title.
        title: Option<String>,
    },
    /// A toplevel was unmapped or destroyed.
    WindowDestroyed {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The destroyed window.
        window_id: WindowId,
    },
    /// The active (visible) window changed.
    WindowActivated {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The window that became active.
        window_id: WindowId,
        /// The previously active window, if any.
        previous: Option<WindowId>,
    },
    /// A toplevel title changed.
    TitleChanged {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The window whose title changed.
        window_id: WindowId,
        /// The new title (`None` when cleared).
        title: Option<String>,
    },
    /// Any commit on the window's surface tree (toplevel, subsurface, popup).
    ///
    /// This is the high-frequency event (animations); consumers must treat it
    /// as cheap — counters, damage union, timestamps — never rendering.
    SurfaceCommit {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The window whose surface tree committed.
        window_id: WindowId,
        /// Per-surface-tree commit counter.
        commit_seq: u64,
        /// Damaged region, window-relative.
        damage: Region,
    },
    /// Keyboard focus moved.
    FocusChanged {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The newly focused window, or `None` when focus was cleared.
        window_id: Option<WindowId>,
    },
    /// An xdg-popup belonging to a window was mapped.
    PopupAppeared {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The owning window.
        window_id: WindowId,
        /// Popup identifier assigned by the compositor.
        popup_id: u64,
    },
    /// An xdg-popup belonging to a window was unmapped.
    PopupDisappeared {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The owning window.
        window_id: WindowId,
        /// Popup identifier assigned by the compositor.
        popup_id: u64,
    },
    /// The application registry spawned a process.
    AppLaunched {
        /// Global event sequence.
        seq: u64,
        /// Monotonic ms since compositor start.
        ts_ms: u64,
        /// The launch this event belongs to.
        launch_id: LaunchId,
        /// The launched application.
        app_id: AppId,
        /// The spawned process id, when known.
        pid: Option<i32>,
    },
}

/// Generates the uniform [`RuntimeEvent`] accessors from a single variant list.
///
/// Every enum variant is listed exactly once at the invocation site, so adding a
/// variant means one edit here (plus the enum definition itself). `seq` and
/// `ts_ms` expand their named field across all arms; `kind` maps each variant to
/// the identically named [`EventKind`]. The non-uniform `window_id` accessor is
/// written by hand below.
macro_rules! runtime_event_accessors {
    ( $( $variant:ident ),+ $(,)? ) => {
        impl RuntimeEvent {
            /// Global monotonic sequence number.
            pub fn seq(&self) -> u64 {
                match self {
                    $( RuntimeEvent::$variant { seq, .. } )|+ => *seq,
                }
            }

            /// Monotonic milliseconds since compositor start.
            pub fn ts_ms(&self) -> u64 {
                match self {
                    $( RuntimeEvent::$variant { ts_ms, .. } )|+ => *ts_ms,
                }
            }

            /// The event kind, without its payload.
            pub fn kind(&self) -> EventKind {
                match self {
                    $( RuntimeEvent::$variant { .. } => EventKind::$variant, )+
                }
            }
        }
    };
}

runtime_event_accessors! {
    WindowCreated,
    WindowDestroyed,
    WindowActivated,
    TitleChanged,
    SurfaceCommit,
    FocusChanged,
    PopupAppeared,
    PopupDisappeared,
    AppLaunched,
}

impl RuntimeEvent {
    /// The window this event belongs to, when applicable.
    ///
    /// [`RuntimeEvent::FocusChanged`] carries `Option<WindowId>` and returns it
    /// unchanged; [`RuntimeEvent::AppLaunched`] has no window and returns `None`.
    pub fn window_id(&self) -> Option<WindowId> {
        match self {
            RuntimeEvent::WindowCreated { window_id, .. }
            | RuntimeEvent::WindowDestroyed { window_id, .. }
            | RuntimeEvent::WindowActivated { window_id, .. }
            | RuntimeEvent::TitleChanged { window_id, .. }
            | RuntimeEvent::SurfaceCommit { window_id, .. }
            | RuntimeEvent::PopupAppeared { window_id, .. }
            | RuntimeEvent::PopupDisappeared { window_id, .. } => Some(*window_id),
            RuntimeEvent::FocusChanged { window_id, .. } => *window_id,
            RuntimeEvent::AppLaunched { .. } => None,
        }
    }
}

/// A temporal observation of window state relative to an action.
///
/// Produced by `adesk-observer` when `observe` / `wait_for_change` /
/// `wait_for_quiet` resolves (`docs/architecture.md` §6). Images are attached
/// by `adesk-proto` (`ObserveResult { observation, image }`), never by core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// The window the observation filtered on, when scoped.
    pub window_id: Option<WindowId>,
    /// The action this observation is causally after, when given.
    pub after_action: Option<ActionId>,
    /// Number of counted surface commits.
    pub commits: u64,
    /// Union of damage observed, simplified, window-relative.
    pub changed_regions: Vec<Rect>,
    /// `Some(true/false)` when focus info is available in the window,
    /// `None` when the observation has no focus information.
    pub focus_changed: Option<bool>,
    /// Whether the title changed during the observation.
    pub title_changed: bool,
    /// Windows created during the observation.
    pub new_windows: Vec<WindowId>,
    /// Windows destroyed during the observation.
    pub destroyed_windows: Vec<WindowId>,
    /// Popup ids that appeared during the observation.
    pub popups_appeared: Vec<u64>,
    /// Popup ids that disappeared during the observation.
    pub popups_disappeared: Vec<u64>,
    /// Wall-clock-free elapsed time of the wait, in milliseconds.
    pub elapsed_ms: u64,
    /// Evidence flag, not proof the condition was met: whether the wait's scope
    /// had been quiet for the applicable threshold at resolution time — the
    /// condition's `quiet_ms` for a quiet wait, otherwise the runtime default —
    /// so a timed-out `change` wait can legitimately carry `quiet: true`.
    pub quiet: bool,
    /// Whether the wait expired before the condition was met.
    pub timed_out: bool,
    /// Commit watermark of the observed window at resolution time.
    pub last_commit_seq: u64,
    /// Global sequence watermark at resolution time.
    pub seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn all_events() -> Vec<(RuntimeEvent, EventKind, Option<WindowId>)> {
        vec![
            (
                RuntimeEvent::WindowCreated {
                    seq: 1,
                    ts_ms: 10,
                    window_id: WindowId(1),
                    app_id: Some(AppId::from("app")),
                    pid: Some(42),
                    launch_id: Some(LaunchId(7)),
                    title: Some("t".into()),
                },
                EventKind::WindowCreated,
                Some(WindowId(1)),
            ),
            (
                RuntimeEvent::WindowDestroyed {
                    seq: 2,
                    ts_ms: 20,
                    window_id: WindowId(1),
                },
                EventKind::WindowDestroyed,
                Some(WindowId(1)),
            ),
            (
                RuntimeEvent::WindowActivated {
                    seq: 3,
                    ts_ms: 30,
                    window_id: WindowId(2),
                    previous: Some(WindowId(1)),
                },
                EventKind::WindowActivated,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::TitleChanged {
                    seq: 4,
                    ts_ms: 40,
                    window_id: WindowId(2),
                    title: None,
                },
                EventKind::TitleChanged,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::SurfaceCommit {
                    seq: 5,
                    ts_ms: 50,
                    window_id: WindowId(2),
                    commit_seq: 99,
                    damage: Region::from_rect(Rect {
                        x: 0,
                        y: 0,
                        w: 4,
                        h: 4,
                    }),
                },
                EventKind::SurfaceCommit,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::FocusChanged {
                    seq: 6,
                    ts_ms: 60,
                    window_id: Some(WindowId(2)),
                },
                EventKind::FocusChanged,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::FocusChanged {
                    seq: 7,
                    ts_ms: 70,
                    window_id: None,
                },
                EventKind::FocusChanged,
                None,
            ),
            (
                RuntimeEvent::PopupAppeared {
                    seq: 8,
                    ts_ms: 80,
                    window_id: WindowId(2),
                    popup_id: 3,
                },
                EventKind::PopupAppeared,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::PopupDisappeared {
                    seq: 9,
                    ts_ms: 90,
                    window_id: WindowId(2),
                    popup_id: 3,
                },
                EventKind::PopupDisappeared,
                Some(WindowId(2)),
            ),
            (
                RuntimeEvent::AppLaunched {
                    seq: 10,
                    ts_ms: 100,
                    launch_id: LaunchId(7),
                    app_id: AppId::from("app"),
                    pid: Some(42),
                },
                EventKind::AppLaunched,
                None,
            ),
        ]
    }

    #[test]
    fn accessors_cover_every_variant() {
        for (event, kind, window_id) in all_events() {
            assert_eq!(event.kind(), kind, "kind mismatch for {event:?}");
            assert_eq!(
                event.window_id(),
                window_id,
                "window_id mismatch for {event:?}"
            );
            assert!(event.seq() >= 1, "seq missing for {event:?}");
            assert!(event.ts_ms() >= 10, "ts_ms missing for {event:?}");
        }
    }

    #[test]
    fn seq_and_ts_ms_are_exposed() {
        let event = RuntimeEvent::WindowDestroyed {
            seq: 42,
            ts_ms: 1234,
            window_id: WindowId(5),
        };
        assert_eq!(event.seq(), 42);
        assert_eq!(event.ts_ms(), 1234);
    }

    #[test]
    fn events_round_trip_through_serde() {
        for (event, _, _) in all_events() {
            let json = serde_json::to_string(&event).unwrap();
            let back: RuntimeEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event, "round-trip failed for {json}");
        }
    }

    #[test]
    fn event_kind_wire_names() {
        let expected = [
            (EventKind::WindowCreated, "window_created"),
            (EventKind::WindowDestroyed, "window_destroyed"),
            (EventKind::WindowActivated, "window_activated"),
            (EventKind::TitleChanged, "title_changed"),
            (EventKind::SurfaceCommit, "surface_commit"),
            (EventKind::FocusChanged, "focus_changed"),
            (EventKind::PopupAppeared, "popup_appeared"),
            (EventKind::PopupDisappeared, "popup_disappeared"),
            (EventKind::AppLaunched, "app_launched"),
        ];
        for (kind, name) in expected {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(name));
            assert_eq!(
                kind,
                serde_json::from_value(serde_json::json!(name)).unwrap()
            );
        }
    }
}
