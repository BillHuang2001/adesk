//! The single-visible-toplevel policy: pure decision functions over
//! [`WindowModel`].
//!
//! This module is the *policy seam* required by `docs/architecture.md` §4:
//! every window-management decision is a pure function of the window model and
//! the [`PolicyConfig`], with no Smithay, no I/O, no async and no clock, so it
//! is unit-testable in isolation and replaceable.
//!
//! Multi-window support is added by a sibling module (for example
//! `policy::multi_window`) exposing the same function signatures plus a
//! dispatch in [`crate::WindowManager`]; the model, the public API and the
//! compositor's call sites do not change. Do not move policy decisions into
//! `manager.rs` or into the compositor.
//!
//! Every function is total: unknown window ids and duplicate events are
//! ignored (returning no actions), because a compositor may legitimately
//! observe an event for a window it has just destroyed.

use adesk_core::{Point, Position, Region, Size, WindowId, WindowInfo};

use crate::action::WmAction;
use crate::config::PolicyConfig;
use crate::error::Result;
use crate::model::{MapRequest, SurfaceKey, WindowModel, WindowRecord};

/// Handles a toplevel map.
///
/// Assigns the next [`WindowId`], creates a [`WindowRecord`] with
/// `geometry = config.tiled_rect()`, `state = Active`, `mapped = true`,
/// `last_commit_seq = 0`, `popup_count = 0` and the request's metadata, marks
/// the previously active window `Inactive` (it stays mapped) and returns:
///
/// 1. [`WmAction::ConfigureWindow`] with `config.tiled_rect()`, then
/// 2. [`WmAction::Activate`] for the new window.
///
/// A `surface_key` that is already tracked is not re-created: the existing id
/// is returned together with a single [`WmAction::None`]. The compositor must
/// call [`on_destroy`] before a surface key can be mapped again.
pub(crate) fn on_map(
    model: &mut WindowModel,
    config: &PolicyConfig,
    request: MapRequest,
) -> (WindowId, Vec<WmAction>) {
    let _ = (model, config, request);
    todo!("adesk-wm policy: on_map")
}

/// Handles a toplevel destroy (v1 does not distinguish unmap from destroy).
///
/// Removes the record and its MRU entry. If `id` was the active window:
///
/// - with remaining windows, the most recently used one becomes `Active` and
///   the returned list is `[WmAction::ActivatePrevious { id: fallback }]`;
/// - with no remaining windows, the manager has no active window and the
///   returned list is empty.
///
/// If `id` was not active (or unknown) the active window is unchanged and the
/// returned list is empty.
pub(crate) fn on_destroy(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    let _ = (model, id);
    todo!("adesk-wm policy: on_destroy")
}

/// Handles a title change.
///
/// Sets `title` on the record and returns no actions: titles never influence
/// the v1 tiling policy. Unknown ids are ignored.
pub(crate) fn on_title(
    model: &mut WindowModel,
    id: WindowId,
    title: Option<String>,
) -> Vec<WmAction> {
    let _ = (model, id, title);
    todo!("adesk-wm policy: on_title")
}

/// Handles a surface commit on the window's surface tree.
///
/// Sets `last_commit_seq` to `max(current, commit_seq)` and returns no actions.
/// `damage` is window-relative evidence of what changed; it is informational in
/// v1 (`adesk-observer` owns damage history) and never changes a policy
/// decision, but the parameter is part of the signature so a damage-aware
/// policy can be added without touching call sites. Unknown ids are ignored.
pub(crate) fn on_commit(
    model: &mut WindowModel,
    id: WindowId,
    commit_seq: u64,
    damage: &Region,
) -> Vec<WmAction> {
    let _ = (model, id, commit_seq, damage);
    todo!("adesk-wm policy: on_commit")
}

/// Handles a popup being mapped for this window.
///
/// Increments `popup_count` (saturating) and returns no actions: popups render
/// above their parent and never change which toplevel is visible. Unknown ids
/// are ignored.
pub(crate) fn on_popup_added(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    let _ = (model, id);
    todo!("adesk-wm policy: on_popup_added")
}

/// Handles a popup being unmapped for this window.
///
/// Decrements `popup_count` with saturation at zero (an extra removal is
/// ignored rather than wrapping) and returns no actions. Unknown ids are
/// ignored.
pub(crate) fn on_popup_removed(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    let _ = (model, id);
    todo!("adesk-wm policy: on_popup_removed")
}

/// Activates a window (`activate_window`, and the auto-focus on map).
///
/// - Unknown id: returns an empty list; the compositor answers
///   `unknown_window` (check [`crate::WindowManager::window`] first).
/// - Already active: returns `[WmAction::None]` so the compositor can skip a
///   spurious `WindowActivated` event.
/// - Otherwise: the previously active window becomes `Inactive` (still
///   mapped), `id` becomes `Active` and moves to the front of the MRU order,
///   and the returned list is `[WmAction::Activate { id }]`.
pub(crate) fn activate(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    let _ = (model, id);
    todo!("adesk-wm policy: activate")
}

/// Changes the virtual output size and re-tiles every mapped window.
///
/// Sets `config.output_size = size`, sets every record's `geometry` to
/// `config.tiled_rect()` and returns one
/// [`WmAction::ConfigureWindow`] per mapped window in creation order. Inactive
/// windows are reconfigured too: they must already be the right size when they
/// become visible again.
pub(crate) fn on_output_size(
    model: &mut WindowModel,
    config: &mut PolicyConfig,
    size: Size,
) -> Vec<WmAction> {
    let _ = (model, config, size);
    todo!("adesk-wm policy: on_output_size")
}

/// The active window id: the front of the MRU order, or `None` when no window
/// is mapped.
pub(crate) fn active_window(model: &WindowModel) -> Option<WindowId> {
    let _ = model;
    todo!("adesk-wm policy: active_window")
}

/// The record for `id`, or `None` when the window is not tracked.
pub(crate) fn window(model: &WindowModel, id: WindowId) -> Option<&WindowRecord> {
    let _ = (model, id);
    todo!("adesk-wm policy: window")
}

/// The record for `id` as a crate error, for command paths that must answer the
/// AGP `unknown_window` code.
pub(crate) fn require_window(model: &WindowModel, id: WindowId) -> Result<&WindowRecord> {
    let _ = (model, id);
    todo!("adesk-wm policy: require_window")
}

/// All tracked windows in creation order (ascending id).
pub(crate) fn windows(model: &WindowModel) -> &[WindowRecord] {
    let _ = model;
    todo!("adesk-wm policy: windows")
}

/// The id of the window owning `key`, or `None` when the surface key is not
/// tracked.
pub(crate) fn window_by_surface(model: &WindowModel, key: SurfaceKey) -> Option<WindowId> {
    let _ = (model, key);
    todo!("adesk-wm policy: window_by_surface")
}

/// Projects `id` into the wire-facing [`WindowInfo`] (AGP §4), or `None`.
pub(crate) fn window_info(model: &WindowModel, id: WindowId) -> Option<WindowInfo> {
    let _ = (model, id);
    todo!("adesk-wm policy: window_info")
}

/// Resolves a window-relative [`Position`] to output coordinates.
///
/// The position is resolved against a rect at the origin with the window's
/// size (core's clamping and normalization rules) and then translated by the
/// window geometry's origin, so the output origin is never hard-coded.
/// Returns `None` for unknown windows.
///
/// For a window at `(100, 50)` sized `100x50`:
/// `Normalized { x: 0.0, y: 0.0 }` → `(100, 50)`,
/// `Normalized { x: 1.0, y: 1.0 }` → `(199, 99)`,
/// `Pixels(10, 10)` → `(110, 60)`,
/// `Pixels(-5, -5)` → `(100, 50)`,
/// `Pixels(500, 500)` → `(199, 99)`.
pub(crate) fn resolve_position(
    model: &WindowModel,
    id: WindowId,
    position: Position,
) -> Option<Point> {
    let _ = (model, id, position);
    todo!("adesk-wm policy: resolve_position")
}
