//! Render-element collection for a window's surface tree.
//!
//! Smithay renders *elements*, not surfaces: the compositor walks a window's
//! surface tree, turns each mapped surface into a `WaylandSurfaceRenderElement`,
//! appends its popups and hands the list to an `OutputDamageTracker`. This module
//! owns that collection logic so `headless.rs` only has to pick a renderer.
//!
//! # Verified Smithay 0.7 facts recorded here (so Phase 2 does not re-discover them)
//!
//! * `render_elements_from_surface_tree(renderer, surface, location, scale,
//!   alpha, kind) -> Vec<E>` (`src/backend/renderer/element/surface.rs:90`) walks
//!   the **whole** surface tree downward via `with_surface_tree_downward`, so
//!   subsurfaces are included — but **popups are not**. It requires
//!   `R: Renderer + ImportAll`, `R::TextureId: Clone + 'static` and
//!   `E: From<WaylandSurfaceRenderElement<R>>`; unmapped surfaces are skipped and
//!   import failures are logged and dropped by Smithay itself.
//! * Its `location` parameter is `impl Into<Point<i32, Physical>>`, so a
//!   `Point<i32, Logical>` must be converted by the caller
//!   (`location.to_physical(scale)`); it does not convert implicitly.
//! * `OutputDamageTracker::new(size, scale, transform)`
//!   (`src/backend/renderer/damage/mod.rs:243`) plus
//!   `render_output(&mut self, renderer, framebuffer, age, elements,
//!   clear_color)` (`:316`) document `elements` as **front-to-back**. The tree
//!   walk above yields back-to-front (parents before children), so the collected
//!   list must be reversed before rendering.
//! * `PopupManager::popups_for_surface(surface)` (`src/desktop/wayland/popup/
//!   manager.rs:177`) is a *static* method returning
//!   `impl Iterator<Item = (PopupKind, Point<i32, Logical>)>` with offsets already
//!   accumulated relative to `surface`. It yields `PopupKind` (xdg *and*
//!   input-method popups), not `WlSurface`: convert with
//!   `PopupKind::wl_surface().clone()` (or `WlSurface::from(kind)`).
//!
//! Nothing here produces pixels; damage regions and frames are `adesk-render`'s
//! concern.

use smithay::{
    backend::renderer::{ImportAll, Renderer},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

/// Render a surface tree (toplevel + subsurfaces) into `framebuffer`, returning
/// the damage region.
///
/// `location` is the tree's origin in output-logical coordinates and `scale` the
/// output scale; `age` is the damage-tracking age (`0` = full redraw) passed to
/// `OutputDamageTracker::render_output`. The returned region is
/// window-relative: it is the tracker's damage translated back to the window's
/// origin and clipped to the window rectangle.
///
/// Popups are *not* part of the tree walk — callers append
/// [`popup_surfaces`] separately, on top.
pub(crate) fn render_surface_tree<R>(
    _renderer: &mut R,
    _framebuffer: &mut R::Framebuffer<'_>,
    _surface: &WlSurface,
    _location: Point<i32, Logical>,
    _scale: f64,
    _age: usize,
) -> crate::Result<adesk_core::Region>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    todo!("Phase 2: render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Unspecified) then OutputDamageTracker::render_output (elements front-to-back; the tree walk yields back-to-front, so reverse)")
}

/// Collect the popup surfaces of a toplevel with their window-relative offsets,
/// in surface-tree order (bottom-to-top, i.e. back-to-front).
///
/// Offsets are relative to `surface`'s origin, so they can be added to the
/// window's output location directly. Because the order is back-to-front, the
/// caller reverses the combined list when building the front-to-back element
/// list for `OutputDamageTracker::render_output`.
pub(crate) fn popup_surfaces(_surface: &WlSurface) -> Vec<(WlSurface, Point<i32, Logical>)> {
    todo!("Phase 2: PopupManager::popups_for_surface(surface); popups are NOT included by render_elements_from_surface_tree")
}
