//! [`WindowManager`] — the window-model façade owned by the compositor.

use adesk_core::{AppId, Point, Position, Rect, Region, Size, WindowId, WindowInfo};

use crate::action::WmAction;
use crate::config::PolicyConfig;
use crate::error::Result;
use crate::model::{MapRequest, SurfaceKey, WindowModel, WindowRecord};
use crate::policy;

/// The window model and tiling policy of one ADesk runtime.
///
/// The compositor owns exactly one `WindowManager` on its thread and calls it
/// for every window lifecycle event; the returned [`WmAction`]s are applied to
/// the Smithay objects (see the crate documentation for the mapping). The
/// manager is pure: no Smithay handles, no timers, no I/O, and every method is
/// deterministic, so the policy is unit-testable without a compositor.
///
/// [`WindowRecord`] is a read-model. Never mutate its fields directly: all
/// state changes go through this type so the policy invariants hold.
///
/// Method bodies are thin delegations to the crate-internal `policy` module;
/// that module is the seam where a multi-window policy is added.
#[derive(Debug, Clone)]
pub struct WindowManager {
    config: PolicyConfig,
    model: WindowModel,
}

impl WindowManager {
    /// Creates a manager for the given policy configuration.
    pub fn new(config: PolicyConfig) -> WindowManager {
        WindowManager {
            config,
            model: WindowModel::new(),
        }
    }

    /// The active policy configuration.
    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// The rect every mapped window is tiled to: the whole output at `(0, 0)`.
    pub fn tiled_rect(&self) -> Rect {
        self.config.tiled_rect()
    }

    /// Changes the virtual output size and re-tiles every mapped window.
    ///
    /// Returns one [`WmAction::ConfigureWindow`] per mapped window in creation
    /// order, inactive windows included (they must already be the right size
    /// when they become visible again). v1 fixes the output size at startup, so
    /// this only runs when the runtime is reconfigured.
    pub fn set_output_size(&mut self, size: Size) -> Vec<WmAction> {
        policy::on_output_size(&mut self.model, &mut self.config, size)
    }

    /// Handles a toplevel map and returns the assigned id plus the actions to
    /// apply.
    ///
    /// The window becomes the active one, is configured to
    /// [`WindowManager::tiled_rect`] and the previously active window becomes
    /// `Inactive` (still mapped). See `policy::on_map` for the exact contract,
    /// including the duplicate-surface-key case.
    ///
    /// The compositor emits `WindowCreated { window_id: id, app_id, pid,
    /// launch_id, title }` with the returned id, then applies the actions in
    /// order.
    pub fn on_map(&mut self, request: MapRequest) -> (WindowId, Vec<WmAction>) {
        policy::on_map(&mut self.model, &self.config, request)
    }

    /// Handles a toplevel destroy (unmap and destroy are not distinguished in
    /// v1; a remapped surface key gets a new window id).
    ///
    /// If the destroyed window was active, the most recently used remaining
    /// window becomes active through [`WmAction::ActivatePrevious`]; if no
    /// window remains there is no active window. Unknown ids are ignored.
    /// The compositor emits `WindowDestroyed` *before* applying the returned
    /// actions, so the fallback activation is observed after the destroy.
    pub fn on_destroy(&mut self, id: WindowId) -> Vec<WmAction> {
        policy::on_destroy(&mut self.model, id)
    }

    /// Handles an `xdg_toplevel` title change. Returns no actions; unknown ids
    /// are ignored.
    pub fn on_title(&mut self, id: WindowId, title: Option<String>) -> Vec<WmAction> {
        policy::on_title(&mut self.model, id, title)
    }

    /// Handles a late `xdg_toplevel.app_id` change (clients may set the app id
    /// after the first buffer commit, so the map-time value can be stale).
    ///
    /// Sets the record's `app_id` to exactly the passed value, so `None` clears
    /// it (the compositor maps an empty app id to `None`). Metadata-only: it
    /// returns no actions and never re-configures or damages a window, and the
    /// updated value is what `window_info` / `list_windows` report. Unknown ids
    /// are ignored.
    pub fn on_app_id(&mut self, id: WindowId, app_id: Option<AppId>) -> Vec<WmAction> {
        policy::on_app_id(&mut self.model, id, app_id)
    }

    /// Handles a surface commit on the window's surface tree.
    ///
    /// Updates `last_commit_seq` monotonically and returns no actions. `damage`
    /// is window-relative and informational in v1. This is the high-frequency
    /// path: it must stay allocation-free and never log above `trace`.
    pub fn on_commit(&mut self, id: WindowId, commit_seq: u64, damage: &Region) -> Vec<WmAction> {
        policy::on_commit(&mut self.model, id, commit_seq, damage)
    }

    /// Handles an `xdg-popup` being mapped for this window.
    ///
    /// Increments `popup_count` (saturating) and returns no actions. Unknown
    /// ids are ignored.
    pub fn on_popup_added(&mut self, id: WindowId) -> Vec<WmAction> {
        policy::on_popup_added(&mut self.model, id)
    }

    /// Handles an `xdg-popup` being unmapped for this window.
    ///
    /// Decrements `popup_count`, saturating at zero, and returns no actions.
    /// Unknown ids are ignored.
    pub fn on_popup_removed(&mut self, id: WindowId) -> Vec<WmAction> {
        policy::on_popup_removed(&mut self.model, id)
    }

    /// Activates a window directly (AGP `activate_window`).
    ///
    /// Returns `[WmAction::Activate { id }]` after deactivating the previous
    /// active window, `[WmAction::None]` when `id` is already active, and an
    /// empty list when `id` is unknown (the compositor answers
    /// `unknown_window`; see [`WindowManager::require_window`]).
    ///
    /// This is never implemented as synthetic input: it changes compositor
    /// state, it does not go through the seat.
    pub fn activate(&mut self, id: WindowId) -> Vec<WmAction> {
        policy::activate(&mut self.model, id)
    }

    /// The active (visible, keyboard-focused) window, if any.
    pub fn active_window(&self) -> Option<WindowId> {
        policy::active_window(&self.model)
    }

    /// The record for `id`, or `None` when the window is not tracked.
    pub fn window(&self, id: WindowId) -> Option<&WindowRecord> {
        policy::window(&self.model, id)
    }

    /// The record for `id` as a crate error, for command paths that must answer
    /// the AGP `unknown_window` code.
    pub fn require_window(&self, id: WindowId) -> Result<&WindowRecord> {
        policy::require_window(&self.model, id)
    }

    /// All tracked windows in creation order (ascending id).
    ///
    /// For AGP `list_windows`, project with
    /// [`WindowRecord::info`]: `wm.windows().iter().map(WindowRecord::info)`.
    pub fn windows(&self) -> &[WindowRecord] {
        policy::windows(&self.model)
    }

    /// The id of the window owning `key`, or `None` when the surface key is not
    /// tracked. Used to route events for subsurfaces/popups to their toplevel.
    pub fn window_by_surface(&self, key: SurfaceKey) -> Option<WindowId> {
        policy::window_by_surface(&self.model, key)
    }

    /// Projects `id` into the wire-facing [`WindowInfo`] (AGP §4), or `None`
    /// when the window is not tracked.
    pub fn window_info(&self, id: WindowId) -> Option<WindowInfo> {
        policy::window_info(&self.model, id)
    }

    /// Resolves a window-relative [`Position`] to output coordinates, or
    /// `None` when the window is not tracked.
    ///
    /// The conversion goes through the window's geometry (see `policy::
    /// resolve_position` for the exact rules); the output origin is never
    /// hard-coded, so a future multi-window policy works unchanged.
    pub fn resolve_position(&self, id: WindowId, position: Position) -> Option<Point> {
        policy::resolve_position(&self.model, id, position)
    }
}
