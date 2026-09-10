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

use adesk_core::{AppId, Point, Position, Rect, Region, Size, WindowId, WindowInfo, WindowState};
use tracing::{debug, trace};

use crate::action::WmAction;
use crate::config::PolicyConfig;
use crate::error::{Error, Result};
use crate::model::{MapRequest, SurfaceKey, WindowModel, WindowRecord};

/// Index of `id` in `model.records`, or `None` when it is not tracked.
fn index_of(model: &WindowModel, id: WindowId) -> Option<usize> {
    model.records.iter().position(|record| record.id == id)
}

/// Mutable record for `id`, or `None` when it is not tracked.
fn record_mut(model: &mut WindowModel, id: WindowId) -> Option<&mut WindowRecord> {
    model.records.iter_mut().find(|record| record.id == id)
}

/// Makes `id` the only `Active` record and marks every other record `Inactive`.
///
/// Only touches `records`; the MRU order is maintained by the callers so the
/// two representations can never drift.
fn mark_active(model: &mut WindowModel, id: WindowId) {
    for record in &mut model.records {
        record.state = if record.id == id {
            WindowState::Active
        } else {
            WindowState::Inactive
        };
    }
}

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
    if let Some(existing) = window_by_surface(model, request.surface_key) {
        debug!(
            surface = %request.surface_key,
            window_id = existing.0,
            "duplicate map of tracked surface key, reusing window"
        );
        return (existing, vec![WmAction::None]);
    }

    let id = WindowId(model.next_id);
    model.next_id += 1;
    let geometry = config.tiled_rect();
    model.records.push(WindowRecord {
        id,
        surface_key: request.surface_key,
        app_id: request.app_id,
        pid: request.pid,
        title: request.title,
        geometry,
        state: WindowState::Active,
        mapped: true,
        created_seq: request.created_seq,
        last_commit_seq: 0,
        popup_count: 0,
    });
    mark_active(model, id);
    model.mru.insert(0, id);
    debug!(
        window_id = id.0,
        surface = %request.surface_key,
        ?geometry,
        "mapped toplevel"
    );
    (
        id,
        vec![
            WmAction::ConfigureWindow { id, rect: geometry },
            WmAction::Activate { id },
        ],
    )
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
    let Some(index) = index_of(model, id) else {
        return Vec::new();
    };
    let was_active = active_window(model) == Some(id);
    model.records.remove(index);
    model.mru.retain(|entry| *entry != id);

    if !was_active {
        debug!(window_id = id.0, "destroyed inactive toplevel");
        return Vec::new();
    }

    match active_window(model) {
        Some(fallback) => {
            mark_active(model, fallback);
            debug!(
                window_id = id.0,
                fallback = fallback.0,
                "destroyed active toplevel, falling back to MRU window"
            );
            vec![WmAction::ActivatePrevious { id: fallback }]
        }
        None => {
            debug!(
                window_id = id.0,
                "destroyed last toplevel, no active window"
            );
            Vec::new()
        }
    }
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
    if let Some(record) = record_mut(model, id) {
        record.title = title;
        trace!(window_id = id.0, "title changed");
    }
    Vec::new()
}

/// Handles a late `xdg_toplevel.app_id` change.
///
/// Sets `app_id` on the record to exactly the passed value (`None` clears it)
/// and returns no actions: the app id is metadata used for app↔window
/// correlation and never influences the v1 tiling policy. Unknown ids are
/// ignored.
pub(crate) fn on_app_id(
    model: &mut WindowModel,
    id: WindowId,
    app_id: Option<AppId>,
) -> Vec<WmAction> {
    if let Some(record) = record_mut(model, id) {
        record.app_id = app_id;
        trace!(window_id = id.0, "app id changed");
    }
    Vec::new()
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
    if let Some(record) = record_mut(model, id) {
        // `max` keeps the watermark monotonic when commits arrive out of order
        // or twice; no allocation, this is the hot path.
        record.last_commit_seq = record.last_commit_seq.max(commit_seq);
        trace!(
            window_id = id.0,
            commit_seq,
            damage = damage.len(),
            "surface commit"
        );
    }
    Vec::new()
}

/// Handles a popup being mapped for this window.
///
/// Increments `popup_count` (saturating) and returns no actions: popups render
/// above their parent and never change which toplevel is visible. Unknown ids
/// are ignored.
pub(crate) fn on_popup_added(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    if let Some(record) = record_mut(model, id) {
        record.popup_count = record.popup_count.saturating_add(1);
        trace!(window_id = id.0, popups = record.popup_count, "popup added");
    }
    Vec::new()
}

/// Handles a popup being unmapped for this window.
///
/// Decrements `popup_count` with saturation at zero (an extra removal is
/// ignored rather than wrapping) and returns no actions. Unknown ids are
/// ignored.
pub(crate) fn on_popup_removed(model: &mut WindowModel, id: WindowId) -> Vec<WmAction> {
    if let Some(record) = record_mut(model, id) {
        record.popup_count = record.popup_count.saturating_sub(1);
        trace!(
            window_id = id.0,
            popups = record.popup_count,
            "popup removed"
        );
    }
    Vec::new()
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
    if index_of(model, id).is_none() {
        return Vec::new();
    }
    if active_window(model) == Some(id) {
        return vec![WmAction::None];
    }

    let previous = active_window(model);
    mark_active(model, id);
    model.mru.retain(|entry| *entry != id);
    model.mru.insert(0, id);
    debug!(
        window_id = id.0,
        previous = previous.map(|previous| previous.0),
        "activated window"
    );
    vec![WmAction::Activate { id }]
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
    config.output_size = size;
    let rect = config.tiled_rect();
    let mut actions = Vec::with_capacity(model.records.len());
    for record in &mut model.records {
        record.geometry = rect;
        if record.mapped {
            actions.push(WmAction::ConfigureWindow {
                id: record.id,
                rect,
            });
        }
    }
    debug!(
        width = size.w,
        height = size.h,
        windows = model.records.len(),
        "output resized, re-tiled every mapped window"
    );
    actions
}

/// The active window id: the front of the MRU order, or `None` when no window
/// is mapped.
pub(crate) fn active_window(model: &WindowModel) -> Option<WindowId> {
    model.mru.first().copied()
}

/// The record for `id`, or `None` when the window is not tracked.
pub(crate) fn window(model: &WindowModel, id: WindowId) -> Option<&WindowRecord> {
    model.records.iter().find(|record| record.id == id)
}

/// The record for `id` as a crate error, for command paths that must answer the
/// AGP `unknown_window` code.
pub(crate) fn require_window(model: &WindowModel, id: WindowId) -> Result<&WindowRecord> {
    window(model, id).ok_or(Error::UnknownWindow(id))
}

/// All tracked windows in creation order (ascending id).
pub(crate) fn windows(model: &WindowModel) -> &[WindowRecord] {
    &model.records
}

/// The id of the window owning `key`, or `None` when the surface key is not
/// tracked.
pub(crate) fn window_by_surface(model: &WindowModel, key: SurfaceKey) -> Option<WindowId> {
    model
        .records
        .iter()
        .find(|record| record.surface_key == key)
        .map(|record| record.id)
}

/// Projects `id` into the wire-facing [`WindowInfo`] (AGP §4), or `None`.
pub(crate) fn window_info(model: &WindowModel, id: WindowId) -> Option<WindowInfo> {
    window(model, id).map(WindowRecord::info)
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
    let geometry = window(model, id)?.geometry;
    // Resolve against a rect at the origin so core's `Pixels` clamping cannot
    // mistake output coordinates for window-relative ones, then translate by
    // the geometry origin. The output origin is never hard-coded.
    let local = position.resolve(Rect::from_size(geometry.size()));
    Some(Point::new(
        geometry.x.saturating_add(local.x),
        geometry.y.saturating_add(local.y),
    ))
}
