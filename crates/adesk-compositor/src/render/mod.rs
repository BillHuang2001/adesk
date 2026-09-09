//! Headless rendering: renderer construction and render-element collection.
//!
//! This module is the compositor's only contact surface with Smithay's renderer
//! abstraction. It answers exactly two questions for the rest of the crate:
//!
//! 1. *Which renderer do we have?* — [`HeadlessRenderer`] wraps either a
//!    `GlesRenderer` (surfaceless EGL; Mesa `llvmpipe` in a VM) or a
//!    `PixmanRenderer` (pure software), chosen once at startup from
//!    [`RendererKind`](crate::config::RendererKind). The dmabuf formats it
//!    advertises feed the `zwp_linux_dmabuf` global created in
//!    [`State::new`](crate::state::State).
//! 2. *What has to be drawn?* — [`OutputWindow`] is the output-composition
//!    path's view of one window, and [`elements`] turns a window's surface tree
//!    and its popups into Smithay render elements.
//!
//! Pixel production is deliberately *not* here: offscreen targets, damage
//! tracking, readback, crop/downscale and image encoding belong to
//! `adesk-render`. The compositor keeps only renderer construction and element
//! collection, so the renderer stays an implementation detail of the compositor
//! thread (see the threading contract in the [crate root docs](crate)).//!
//! Phase 1 declares the call graph; the bodies arrive in Phase 2.

// Phase 1 declares the render call graph but nothing calls it yet; the attribute
// covers this module and its children. Remove it when Phase 2 wires these into
// `State::render_window` / `State::render_output`.
#![allow(dead_code)]

mod elements;
mod headless;

pub(crate) use headless::HeadlessRenderer;

use adesk_core::Rect;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// One window as seen by the output composition path.
///
/// `adesk-wm` owns geometry, z-order and activity; the compositor's
/// `RenderOutput` command flattens the visible windows into this type so the
/// renderer never has to know about window-management types. `geometry` is in
/// output coordinates — the window model, not the renderer, converts
/// window-relative coordinates.
pub(crate) struct OutputWindow {
    /// Window rectangle in output pixels (`(0,0)`-origin, tiled by the WM).
    pub(crate) geometry: Rect,
    /// Root surface of the window's surface tree (the `xdg_toplevel`);
    /// subsurfaces are discovered while walking the tree and popups through
    /// [`elements::popup_surfaces`].
    pub(crate) surface: WlSurface,
    /// Whether this window is the active (visible, tiled) one; overlays and
    /// future multi-window composition use this to highlight focus.
    pub(crate) active: bool,
}
