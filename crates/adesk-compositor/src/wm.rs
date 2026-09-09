//! Bridge between the compositor and `adesk-wm`.
//!
//! `adesk-wm` is the pure-logic window model: it owns the window registry, the
//! single-visible-toplevel tiling policy, the focus state machine and — crucially —
//! the conversion of window-relative coordinates to output coordinates
//! (`docs/architecture.md` §4). It never sees Smithay types.
//!
//! [`WmBridge`] is the only place in this crate where Smithay surfaces meet the
//! window model, and the only place allowed to call `adesk_wm`. Protocol handlers
//! and the command dispatcher go through [`crate::state::State`], never through the
//! window manager directly.
//!
//! Phase 1 status: construction and coordinate resolution are real; the query and
//! mutation helpers are stubs that Phase 2 fills in once protocol handlers exist.

// Landed adesk-wm surface this bridge is written against:
//   adesk_wm::PolicyConfig::new(output_size: adesk_core::Size) -> PolicyConfig
//   adesk_wm::WindowManager::new(config: PolicyConfig) -> WindowManager
//   WindowManager::resolve_position(&self, window_id: WindowId, position: Position)
//       -> Option<adesk_core::Point>   (None = unknown window)
//   adesk_wm::WmAction — opaque here; Phase 2 applies actions returned by the WM.

#![allow(dead_code)] // Phase 1: the query/mutation helpers are wired up in Phase 2.

use adesk_core::{AppId, LaunchId, Point, Position, Size, WindowId, WindowInfo};
use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::shell::xdg::ToplevelSurface,
};

use crate::{error::CompositorError, Result};

/// Adapter around `adesk_wm::WindowManager`.
///
/// The manager is the single source of truth for window geometry, focus and
/// lifecycle; this type adds the Smithay surface lookups the protocol handlers need
/// and keeps `adesk-wm` free of compositor types.
pub(crate) struct WmBridge {
    /// The pure-logic window model.
    manager: adesk_wm::WindowManager,
}

impl WmBridge {
    /// Create the bridge around a fresh window manager for the given output size.
    ///
    /// The output size is the tiling target: the active window always fills exactly
    /// this area (`docs/architecture.md` §4).
    pub(crate) fn new(output_size: Size) -> WmBridge {
        WmBridge {
            manager: adesk_wm::WindowManager::new(adesk_wm::PolicyConfig::new(output_size)),
        }
    }

    /// The active (visible, tiled) window, if any.
    pub(crate) fn active_window(&self) -> Option<WindowId> {
        todo!("Phase 2: ask the window manager for the active window")
    }

    /// The window that currently holds keyboard focus, if any.
    ///
    /// Focus is normally the active window, but it can differ while a window is
    /// closing or before the first map.
    pub(crate) fn keyboard_focus(&self) -> Option<WindowId> {
        todo!("Phase 2: ask the window manager for the keyboard focus")
    }

    /// The window owning a Wayland surface (toplevel, subsurface or popup).
    pub(crate) fn window_for_surface(&self, _surface: &WlSurface) -> Option<WindowId> {
        todo!("Phase 2: resolve a surface through the window manager's surface keys")
    }

    /// All known windows in creation order, for `QueryState`.
    pub(crate) fn windows(&self) -> Vec<WindowInfo> {
        todo!("Phase 2: project the window registry into WindowInfo records")
    }

    /// The toplevel surface of a window, for configuring or closing it.
    pub(crate) fn toplevel_of(&self, _id: WindowId) -> Option<ToplevelSurface> {
        todo!("Phase 2: map a window id to its toplevel surface")
    }

    /// The root `wl_surface` of a window, for keyboard/pointer focus.
    pub(crate) fn surface_of(&self, _id: WindowId) -> Option<WlSurface> {
        todo!("Phase 2: map a window id to its root wl_surface")
    }

    /// Per-window commit counter of the last observed commit.
    pub(crate) fn last_commit_seq(&self, _id: WindowId) -> u64 {
        todo!("Phase 2: read the window's commit counter from the window manager")
    }

    /// Record that the app registry spawned a process, so a toplevel mapping shortly
    /// afterwards can be correlated with the launch.
    pub(crate) fn note_launch(&mut self, _launch_id: LaunchId, _app_id: AppId, _pid: Option<i32>) {
        todo!("Phase 2: register the pending launch for window correlation")
    }

    /// Resolve a window-relative position to an output point.
    ///
    /// The window model — never a hard-coded constant — is the authority for
    /// geometry, so `(0,0)` output origin assumptions stay out of this crate.
    /// An unknown window surfaces as [`CompositorError::WindowManagement`].
    pub(crate) fn resolve_position(&self, id: WindowId, position: &Position) -> Result<Point> {
        self.manager
            .resolve_position(id, *position)
            .ok_or_else(|| CompositorError::WindowManagement(format!("unknown window {id}")))
    }
}
