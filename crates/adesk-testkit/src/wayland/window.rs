//! Window handles: `xdg_toplevel`/`xdg_popup` surfaces with observable configure state.
//!
//! A handle owns its protocol objects (`wl_surface`, `xdg_surface`, role object) plus the
//! [`WindowSlot`] shared with the reader thread, so all methods take `&self`. Handles are
//! created by [`WaylandTestClient::create_toplevel`](super::WaylandTestClient::create_toplevel)
//! / [`create_popup`](super::WaylandTestClient::create_popup); the constructors are
//! `pub(crate)` on purpose — a test can only obtain a handle through the real protocol
//! path.
//!
//! Metadata accessors ([`TestWindow::app_id`], [`TestWindow::title`]) return the values the
//! window was created with. `set_title`/`set_app_id` change the *protocol* state (and
//! [`WindowState`](super::state::WindowState)) without mutating the immutable spec, so the
//! `&str` accessors never return a reference into a lock.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use adesk_core::{Point, Rect, Size};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_surface, xdg_toplevel};

use crate::error::Result;
use crate::fill::FillPattern;

use super::state::{lock_window, WindowSlot};

/// Description of a toplevel to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToplevelSpec {
    /// `app_id` sent with `xdg_toplevel.set_app_id` (the runtime matches it against the
    /// app registry).
    pub app_id: String,
    /// Title sent with `xdg_toplevel.set_title`.
    pub title: String,
    /// Size of the first committed buffer, in pixels.
    pub size: Size,
    /// Fill pattern used by commits that do not override it.
    pub fill: FillPattern,
}

impl ToplevelSpec {
    /// A toplevel with `app_id`, `title`, `size` and the default fill
    /// ([`FillPattern::default`]).
    pub fn new(app_id: impl Into<String>, title: impl Into<String>, size: Size) -> ToplevelSpec {
        ToplevelSpec {
            app_id: app_id.into(),
            title: title.into(),
            size,
            fill: FillPattern::default(),
        }
    }

    /// Overrides the `app_id`.
    pub fn with_app_id(mut self, app_id: impl Into<String>) -> ToplevelSpec {
        self.app_id = app_id.into();
        self
    }

    /// Overrides the title.
    pub fn with_title(mut self, title: impl Into<String>) -> ToplevelSpec {
        self.title = title.into();
        self
    }

    /// Overrides the fill pattern.
    pub fn with_fill(mut self, fill: FillPattern) -> ToplevelSpec {
        self.fill = fill;
        self
    }
}

/// An xdg configure the compositor sent, with the serial that acknowledges it.
///
/// `width`/`height` of `0` mean "the client chooses" (the protocol sends `0` in
/// `xdg_toplevel.configure` when the compositor has no preferred size); negative protocol
/// values are clamped to `0`. `states` are the raw native-endian `u32` state codes from the
/// role event, kept as bytes because that is exactly what the wire carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredSize {
    /// Configured width in pixels (`0` = client chooses).
    pub width: u32,
    /// Configured height in pixels (`0` = client chooses).
    pub height: u32,
    /// Raw `xdg_toplevel` state entries (empty for popups).
    pub states: Vec<u8>,
    /// Serial to pass to `xdg_surface.ack_configure`.
    pub serial: u32,
}

impl ConfiguredSize {
    /// The configured size as a [`Size`].
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

/// A toplevel surface created by the test client.
pub struct TestWindow {
    /// The `wl_surface`.
    surface: WlSurface,
    /// The `xdg_surface` role wrapper.
    xdg_surface: xdg_surface::XdgSurface,
    /// The `xdg_toplevel` role object.
    toplevel: xdg_toplevel::XdgToplevel,
    /// State shared with the reader thread.
    state: WindowSlot,
    /// Configures published by the reader thread; `Mutex` because
    /// [`wait_for_configure`](TestWindow::wait_for_configure) needs `&mut` on the receiver
    /// while the method takes `&self`.
    configure_rx: Mutex<mpsc::Receiver<ConfiguredSize>>,
    /// The immutable description this window was created from.
    spec: ToplevelSpec,
}

impl TestWindow {
    /// Wires a new handle to its protocol objects (client-internal).
    pub(crate) fn new(
        surface: WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
        state: WindowSlot,
        configure_rx: mpsc::Receiver<ConfiguredSize>,
        spec: ToplevelSpec,
    ) -> TestWindow {
        TestWindow {
            surface,
            xdg_surface,
            toplevel,
            state,
            configure_rx: Mutex::new(configure_rx),
            spec,
        }
    }

    /// The `app_id` this window was created with (see the module docs).
    pub fn app_id(&self) -> &str {
        &self.spec.app_id
    }

    /// The title this window was created with (see the module docs).
    pub fn title(&self) -> &str {
        &self.spec.title
    }

    /// The current surface size: the last configured size, else the requested size.
    pub fn size(&self) -> Size {
        lock_window(&self.state).size
    }

    /// The fill pattern the next commit uses.
    pub fn fill(&self) -> FillPattern {
        lock_window(&self.state).fill
    }

    /// The `wl_surface` this window draws into.
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// The `xdg_surface` wrapper (used by [`WaylandTestClient::create_popup`]).
    ///
    /// [`WaylandTestClient::create_popup`]: super::WaylandTestClient::create_popup
    pub(crate) fn xdg_surface(&self) -> &xdg_surface::XdgSurface {
        &self.xdg_surface
    }

    /// Allocates a buffer filled with `fill`, attaches it, damages the whole surface and
    /// commits one frame.
    ///
    /// Phase 2 steps: `fill.require_opaque()?`; allocate from the client's `ShmPool`;
    /// `surface.attach(Some(buffer.buffer()), 0, 0)`;
    /// `surface.damage(0, 0, w as i32, h as i32)`; `surface.commit()`; store the buffer in
    /// `attached_buffer`, bump `commits`, remember `size`/`fill` for
    /// [`damage_hint`](TestWindow::damage_hint) and [`size`](TestWindow::size). A commit
    /// before the first `apply_configure` is a protocol error and fails here with
    /// [`TestkitError::NoPendingConfigure`](crate::TestkitError::NoPendingConfigure).
    pub fn commit_frame(&self, fill: FillPattern) -> Result<()> {
        let _ = fill;
        todo!("Phase 2: alloc buffer, attach + damage + commit")
    }

    /// Re-commits the currently attached buffer without allocating.
    ///
    /// Phase 2: `surface.attach(Some(attached_buffer.buffer()), 0, 0)` +
    /// `surface.commit()`; [`TestkitError::NoPendingConfigure`](crate::TestkitError::NoPendingConfigure) when nothing was committed
    /// before, [`TestkitError::SurfaceDestroyed`](crate::TestkitError::SurfaceDestroyed) after [`destroy`](TestWindow::destroy).
    /// Used by [`resize`](TestWindow::resize) and by tests that need a second commit of
    /// identical pixels (damage must be empty for the second one).
    pub fn commit_pending(&self) -> Result<()> {
        todo!("Phase 2: re-commit the attached buffer")
    }

    /// Waits for the next complete xdg configure (role event + serial).
    ///
    /// Phase 2: return an already-pending configure (drained from the channel) before
    /// blocking; otherwise `recv_timeout(timeout)` on the channel fed by the reader thread.
    /// Expiry is [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what = "xdg configure"`, a disconnected
    /// sender (surface destroyed / connection closed) is [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed).
    pub fn wait_for_configure(&self, timeout: Duration) -> Result<ConfiguredSize> {
        let _ = timeout;
        todo!("Phase 2: pending check, then recv_timeout; Timeout{{what: \"xdg configure\", timeout}}")
    }

    /// Acknowledges the pending configure and clears it.
    ///
    /// Phase 2: take `pending_serial`, `xdg_surface.ack_configure(serial)`, set
    /// `applied_serial`, and update `size` from the acknowledged configure;
    /// [`TestkitError::NoPendingConfigure`](crate::TestkitError::NoPendingConfigure) when no configure is waiting. The caller is
    /// expected to commit a buffer of that size next (that is what makes the surface
    /// mapped).
    pub fn apply_configure(&self) -> Result<()> {
        todo!("Phase 2: ack the pending serial on xdg_surface and clear pending state")
    }

    /// The damage region of the most recent commit, or `None` before the first commit.
    ///
    /// Phase 2: `Rect::new(0, 0, w, h)` for the size of the last committed buffer —
    /// [`commit_frame`](TestWindow::commit_frame) damages the whole surface, so the hint is
    /// the full buffer and tests can assert the compositor observed exactly that.
    pub fn damage_hint(&self) -> Option<Rect> {
        todo!("Phase 2: full-buffer rect for the last commit, None before the first commit")
    }

    /// Changes the requested size and re-commits the current buffer.
    ///
    /// Phase 2: store `size` in the window state, then [`commit_pending`](TestWindow::commit_pending);
    /// the compositor answers with a new configure that must be applied before a buffer of
    /// the new size is legal. [`TestkitError::SurfaceDestroyed`](crate::TestkitError::SurfaceDestroyed) after
    /// [`destroy`](TestWindow::destroy).
    pub fn resize(&self, size: Size) -> Result<()> {
        let _ = size;
        todo!("Phase 2: update the desired size and commit_pending")
    }

    /// Sends `xdg_toplevel.set_title`.
    ///
    /// Phase 2: `toplevel.set_title(title.to_string())` and mirror it into the window
    /// state; [`TestkitError::SurfaceDestroyed`](crate::TestkitError::SurfaceDestroyed) after [`destroy`](TestWindow::destroy).
    pub fn set_title(&self, title: &str) -> Result<()> {
        let _ = title;
        todo!("Phase 2: xdg_toplevel.set_title + mirror into window state")
    }

    /// Sends `xdg_toplevel.set_app_id`.
    ///
    /// Phase 2: `toplevel.set_app_id(app_id.to_string())` and mirror it into the window
    /// state; [`TestkitError::SurfaceDestroyed`](crate::TestkitError::SurfaceDestroyed) after [`destroy`](TestWindow::destroy).
    pub fn set_app_id(&self, app_id: &str) -> Result<()> {
        let _ = app_id;
        todo!("Phase 2: xdg_toplevel.set_app_id + mirror into window state")
    }

    /// Destroys the toplevel, its xdg surface and its `wl_surface`.
    ///
    /// Phase 2: idempotent — the first call sends `xdg_toplevel.destroy()`,
    /// `xdg_surface.destroy()` and `wl_surface.destroy()` (in that order), sets
    /// `destroyed`, removes the slot from `ClientState::windows` and returns `Ok(())`;
    /// later calls return [`TestkitError::SurfaceDestroyed`](crate::TestkitError::SurfaceDestroyed). Any buffer still attached is
    /// released with the surface.
    pub fn destroy(&self) -> Result<()> {
        todo!("Phase 2: destroy role/surface objects, mark destroyed, deregister the slot")
    }

    /// Whether [`destroy`](TestWindow::destroy) already ran for this window.
    pub fn is_destroyed(&self) -> bool {
        lock_window(&self.state).destroyed
    }

    /// The configure waiting for [`apply_configure`](TestWindow::apply_configure).
    pub fn pending_configure(&self) -> Option<ConfiguredSize> {
        lock_window(&self.state).pending_configure.clone()
    }

    /// The most recently completed configure.
    pub fn last_configure(&self) -> Option<ConfiguredSize> {
        lock_window(&self.state).last_configure.clone()
    }
}

/// Description of a popup to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupSpec {
    /// Popup size in pixels.
    pub size: Size,
    /// Offset of the popup's top-left corner from the parent's top-left corner, in
    /// parent-window coordinates.
    pub offset: Point,
}

impl PopupSpec {
    /// A popup of `size` at the parent's top-left corner.
    pub fn new(size: Size) -> PopupSpec {
        PopupSpec {
            size,
            offset: Point::ORIGIN,
        }
    }

    /// Overrides the offset.
    pub fn with_offset(mut self, offset: Point) -> PopupSpec {
        self.offset = offset;
        self
    }
}

/// A popup surface created by the test client.
pub struct TestPopup {
    /// The `wl_surface`.
    surface: WlSurface,
    /// The `xdg_surface` role wrapper.
    xdg_surface: xdg_surface::XdgSurface,
    /// The `xdg_popup` role object.
    popup: xdg_popup::XdgPopup,
    /// State shared with the reader thread.
    state: WindowSlot,
    /// Configures published by the reader thread.
    configure_rx: Mutex<mpsc::Receiver<ConfiguredSize>>,
    /// The immutable description this popup was created from.
    spec: PopupSpec,
}

impl TestPopup {
    /// Wires a new popup handle to its protocol objects (client-internal).
    pub(crate) fn new(
        surface: WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
        state: WindowSlot,
        configure_rx: mpsc::Receiver<ConfiguredSize>,
        spec: PopupSpec,
    ) -> TestPopup {
        TestPopup {
            surface,
            xdg_surface,
            popup,
            state,
            configure_rx: Mutex::new(configure_rx),
            spec,
        }
    }

    /// The `wl_surface` this popup draws into.
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// The current popup size: the last configured size, else the requested size.
    pub fn size(&self) -> Size {
        lock_window(&self.state).size
    }

    /// Allocates a buffer filled with `fill`, attaches it, damages the whole surface and
    /// commits one frame (same rules as [`TestWindow::commit_frame`], but popups have no
    /// toplevel states).
    pub fn commit_frame(&self, fill: FillPattern) -> Result<()> {
        let _ = fill;
        todo!("Phase 2: alloc buffer, attach + damage + commit")
    }

    /// Waits for the next complete xdg configure of this popup.
    ///
    /// Phase 2: like [`TestWindow::wait_for_configure`], with
    /// [`TestkitError::Timeout`](crate::TestkitError::Timeout) `what = "xdg configure"`.
    pub fn wait_for_configure(&self, timeout: Duration) -> Result<ConfiguredSize> {
        let _ = timeout;
        todo!("Phase 2: pending check, then recv_timeout")
    }

    /// Acknowledges the pending popup configure.
    ///
    /// Phase 2: `xdg_surface.ack_configure(pending_serial)`, clear pending state,
    /// [`TestkitError::NoPendingConfigure`](crate::TestkitError::NoPendingConfigure) when nothing is pending.
    pub fn apply_configure(&self) -> Result<()> {
        todo!("Phase 2: ack the pending serial on xdg_surface")
    }

    /// Destroys the popup, its xdg surface and its `wl_surface`; idempotent, like
    /// [`TestWindow::destroy`].
    pub fn destroy(&self) -> Result<()> {
        todo!("Phase 2: destroy role/surface objects and mark destroyed")
    }
}
