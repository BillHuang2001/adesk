//! Window data: opaque surface keys, per-window records and the internal model
//! the policy operates on.

use std::fmt;

use adesk_core::{AppId, Rect, WindowId, WindowInfo, WindowState};

/// Opaque handle identifying a Wayland surface tree (toplevel + subsurfaces +
/// popups).
///
/// The compositor creates one per toplevel `wl_surface` (for example from the
/// protocol object id) and uses it to route commits, popups and destroys to
/// the right window. The window model only compares keys; it never interprets
/// the value. The inner value is private on purpose: no crate may depend on
/// how the compositor names surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceKey(u64);

impl SurfaceKey {
    /// Creates a key from the compositor's raw surface identifier.
    pub const fn new(raw: u64) -> SurfaceKey {
        SurfaceKey(raw)
    }
}

impl fmt::Display for SurfaceKey {
    /// Renders as `surface#<raw>` for logs and error messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "surface#{}", self.0)
    }
}

/// Everything the window model knows about one toplevel.
///
/// This is a read-model: fields are public so the compositor can inspect them,
/// but they must only change through [`crate::WindowManager`] methods, which
/// keep the policy invariants (at most one `Active` window, geometry equal to
/// the tiled rect, ...). Writing to a record directly is a bug.
///
/// A record exists exactly while its toplevel is mapped: v1 does not
/// distinguish unmap from destroy, so `mapped` is `true` for every record the
/// manager tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRecord {
    /// Stable window id, assigned on first map, never reused.
    pub id: WindowId,
    /// Surface tree this window belongs to.
    pub surface_key: SurfaceKey,
    /// Desktop-file id of the owning application, once correlated by the
    /// compositor/registry; `None` until then (never guessed).
    pub app_id: Option<AppId>,
    /// Client process id, when known.
    pub pid: Option<i32>,
    /// Current toplevel title.
    pub title: Option<String>,
    /// Window-relative geometry in output coordinates; always
    /// [`crate::WindowManager::tiled_rect`] under the v1 policy.
    pub geometry: Rect,
    /// Visibility state; at most one record is `Active`.
    pub state: WindowState,
    /// Whether the toplevel is mapped (`true` for every tracked record in v1).
    pub mapped: bool,
    /// Global event sequence at which the window was created.
    pub created_seq: u64,
    /// Per-surface-tree commit counter of the last observed commit.
    pub last_commit_seq: u64,
    /// Number of currently mapped popups belonging to this window.
    pub popup_count: u32,
}

impl WindowRecord {
    /// Projects the record into the wire-facing [`WindowInfo`] (AGP §4).
    pub fn info(&self) -> WindowInfo {
        WindowInfo {
            id: self.id,
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            geometry: self.geometry,
            state: self.state,
            mapped: self.mapped,
            pid: self.pid,
            created_seq: self.created_seq,
            last_commit_seq: self.last_commit_seq,
            popup_count: self.popup_count,
        }
    }
}

/// Metadata the compositor supplies when a toplevel is mapped.
///
/// The policy assigns the [`WindowId`], the geometry and the initial state;
/// everything else comes from the client's `xdg_toplevel` (`app_id`, `title`),
/// the `wl_client` (`pid`) and the compositor's global event counter
/// (`created_seq`).
#[derive(Debug)]
pub struct MapRequest {
    /// Surface tree that is being mapped.
    pub surface_key: SurfaceKey,
    /// `xdg_toplevel.app_id`, when the client set one.
    pub app_id: Option<AppId>,
    /// Client process id, when known.
    pub pid: Option<i32>,
    /// `xdg_toplevel.title`, when the client set one.
    pub title: Option<String>,
    /// Global event sequence of the `WindowCreated` event.
    pub created_seq: u64,
}

impl MapRequest {
    /// A map request for `surface_key` with no metadata yet.
    pub fn new(surface_key: SurfaceKey) -> MapRequest {
        MapRequest {
            surface_key,
            app_id: None,
            pid: None,
            title: None,
            created_seq: 0,
        }
    }
}

/// The mutable state the policy operates on.
///
/// Kept separate from [`crate::WindowManager`] so the functions in
/// `crate::policy` are pure with respect to the compositor: they read and
/// write only this struct, never Smithay or any I/O. A future multi-window
/// policy reuses the same model.
///
/// Fields are crate-internal on purpose; the compositor goes through
/// [`crate::WindowManager`].
#[derive(Debug)]
pub(crate) struct WindowModel {
    /// Tracked windows in creation order (`WindowManager::windows` order).
    pub(crate) records: Vec<WindowRecord>,
    /// Most-recently-used order, front = active window. Contains exactly the
    /// ids of `records`, so it never holds a stale id.
    pub(crate) mru: Vec<WindowId>,
    /// Next window id to assign. Starts at `1`; ids are monotonic and never
    /// reused, so this only ever grows.
    pub(crate) next_id: u64,
}

impl WindowModel {
    /// An empty model with the id counter at `1`.
    pub(crate) fn new() -> WindowModel {
        WindowModel {
            records: Vec::new(),
            mru: Vec::new(),
            next_id: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{Point, Rect};

    fn record() -> WindowRecord {
        WindowRecord {
            id: WindowId(7),
            surface_key: SurfaceKey::new(3),
            app_id: Some(AppId::from("org.mozilla.firefox")),
            pid: Some(4242),
            title: Some("GitHub".into()),
            geometry: Rect::new(0, 0, 1280, 800),
            state: WindowState::Active,
            mapped: true,
            created_seq: 800,
            last_commit_seq: 8291,
            popup_count: 2,
        }
    }

    #[test]
    fn surface_key_is_constructible_and_displayable() {
        assert_eq!(SurfaceKey::new(42).to_string(), "surface#42");
        assert_ne!(SurfaceKey::new(1), SurfaceKey::new(2));
    }

    #[test]
    fn map_request_new_has_no_metadata() {
        let request = MapRequest::new(SurfaceKey::new(5));
        assert_eq!(request.surface_key, SurfaceKey::new(5));
        assert_eq!(request.app_id, None);
        assert_eq!(request.pid, None);
        assert_eq!(request.title, None);
        assert_eq!(request.created_seq, 0);
    }

    #[test]
    fn record_projects_every_field_to_window_info() {
        let info = record().info();
        assert_eq!(info.id, WindowId(7));
        assert_eq!(info.app_id, Some(AppId::from("org.mozilla.firefox")));
        assert_eq!(info.title.as_deref(), Some("GitHub"));
        assert_eq!(info.geometry, Rect::new(0, 0, 1280, 800));
        assert_eq!(info.state, WindowState::Active);
        assert!(info.mapped);
        assert_eq!(info.pid, Some(4242));
        assert_eq!(info.created_seq, 800);
        assert_eq!(info.last_commit_seq, 8291);
        assert_eq!(info.popup_count, 2);
    }

    #[test]
    fn empty_model_starts_ids_at_one() {
        let model = WindowModel::new();
        assert_eq!(model.next_id, 1);
        assert!(model.records.is_empty());
        assert!(model.mru.is_empty());
        let _ = Point::ORIGIN;
    }
}
