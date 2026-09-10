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

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adesk_core::{Point, Rect, Size};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_surface, xdg_toplevel};

use crate::error::{Result, TestkitError};
use crate::fill::FillPattern;

use super::state::{lock_client, lock_pool, lock_window, ClientState, WindowSlot};

/// Flushes `conn` so the requests just queued reach the runtime.
///
/// `wayland-client` buffers requests and only writes them out when the buffer fills or
/// the caller flushes, so every method that changes protocol state has to flush before it
/// returns; otherwise a test that never pumps would wait for an event the runtime never
/// saw a request for.
fn flush(conn: &Connection) -> Result<()> {
    conn.flush().map_err(|err| super::map_wayland_error(&err))
}

/// Allocates a fresh buffer of `fill` at the surface's current size, attaches it, damages
/// the whole surface and commits one frame.
///
/// Shared by [`TestWindow::commit_frame`] and [`TestPopup::commit_frame`]. A commit before
/// the first `ack_configure` is a protocol error, and a commit after
/// [`destroy`](TestWindow::destroy) cannot reach the runtime at all; both are reported
/// instead of being sent.
fn commit_buffer(
    surface: &WlSurface,
    state: &WindowSlot,
    conn: &Connection,
    fill: FillPattern,
) -> Result<()> {
    let mut window = lock_window(state);
    if window.destroyed {
        return Err(TestkitError::SurfaceDestroyed);
    }
    if window.applied_serial.is_none() {
        return Err(TestkitError::NoPendingConfigure);
    }
    let size = window.size;
    let buffer = lock_pool(&window.pool).alloc(size, fill)?;
    surface.attach(Some(buffer.buffer()), 0, 0);
    surface.damage(0, 0, size.w as i32, size.h as i32);
    surface.commit();
    window.attached_buffer = Some(buffer);
    window.commits = window.commits.saturating_add(1);
    window.fill = fill;
    window.last_damage = Some(Rect::from_size(size));
    drop(window);
    flush(conn)
}

/// Re-attaches the buffer of the last commit and commits it again without damaging
/// anything.
///
/// Shared by [`TestWindow::commit_pending`] and [`TestPopup::commit_pending`] users (a
/// resize re-commits the pixels already on screen, so the damage hint is empty).
fn recommit_buffer(surface: &WlSurface, state: &WindowSlot, conn: &Connection) -> Result<()> {
    let mut window = lock_window(state);
    if window.destroyed {
        return Err(TestkitError::SurfaceDestroyed);
    }
    let buffer = match window.attached_buffer.as_ref() {
        Some(buffer) => buffer.buffer().clone(),
        None => return Err(TestkitError::NoPendingConfigure),
    };
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    window.commits = window.commits.saturating_add(1);
    // Nothing new was drawn: the second commit of identical pixels damages nothing.
    window.last_damage = Some(Rect::EMPTY);
    drop(window);
    flush(conn)
}

/// Waits for the next complete configure on `configure_rx` (see
/// [`TestWindow::wait_for_configure`]).
fn await_configure(
    configure_rx: &Mutex<mpsc::Receiver<ConfiguredSize>>,
    timeout: Duration,
) -> Result<ConfiguredSize> {
    let receiver = configure_rx
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match receiver.try_recv() {
        Ok(configure) => Ok(configure),
        // A configure that already arrived must never be hidden behind the timeout.
        Err(TryRecvError::Empty) => receiver.recv_timeout(timeout).map_err(|err| match err {
            RecvTimeoutError::Timeout => TestkitError::Timeout {
                what: "xdg configure",
                timeout,
            },
            RecvTimeoutError::Disconnected => TestkitError::ConnectionClosed,
        }),
        // The handle outlives the sender only if the surface was destroyed with the slot
        // still registered, which cannot happen; report it instead of blocking.
        Err(TryRecvError::Disconnected) => Err(TestkitError::ConnectionClosed),
    }
}

/// Acknowledges the pending configure and clears it (see
/// [`TestWindow::apply_configure`]).
fn ack_pending_configure(
    xdg_surface: &xdg_surface::XdgSurface,
    state: &WindowSlot,
    conn: &Connection,
) -> Result<()> {
    let mut window = lock_window(state);
    let Some(serial) = window.pending_serial.take() else {
        return Err(TestkitError::NoPendingConfigure);
    };
    xdg_surface.ack_configure(serial);
    window.applied_serial = Some(serial);
    // The configured size only replaces the requested one when the compositor picked a
    // size (`0` means "client chooses"); the role event already stored it, but a popup's
    // `size` is only known here.
    if let Some(configure) = window
        .last_configure
        .as_ref()
        .filter(|configure| configure.serial == serial)
    {
        if configure.width > 0 && configure.height > 0 {
            window.size = configure.size();
        }
    }
    // A completed configure leaves no role event behind: `xdg_surface.configure` consumed
    // it. A role event still carrying `serial == 0` belongs to the *next* sequence and
    // must survive.
    if window
        .pending_configure
        .as_ref()
        .is_some_and(|configure| configure.serial != 0)
    {
        window.pending_configure = None;
    }
    drop(window);
    flush(conn)
}

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
    /// Client state shared with the reader thread; used to deregister the slot on
    /// [`destroy`](TestWindow::destroy).
    client: Arc<Mutex<ClientState>>,
    /// The request-side connection, so every mutation reaches the runtime even when the
    /// caller never pumps (see [`flush`]).
    conn: Connection,
}

impl TestWindow {
    /// Wires a new handle to its protocol objects (client-internal).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        surface: WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
        state: WindowSlot,
        configure_rx: mpsc::Receiver<ConfiguredSize>,
        spec: ToplevelSpec,
        client: Arc<Mutex<ClientState>>,
        conn: Connection,
    ) -> TestWindow {
        TestWindow {
            surface,
            xdg_surface,
            toplevel,
            state,
            configure_rx: Mutex::new(configure_rx),
            spec,
            client,
            conn,
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
    /// `fill.require_opaque()?` runs first, then a buffer is allocated from the client's
    /// `ShmPool`, attached with `surface.attach(Some(buffer.buffer()), 0, 0)`, the whole
    /// surface is damaged with `surface.damage(0, 0, w as i32, h as i32)` and
    /// `surface.commit()` is sent. The buffer is stored in `attached_buffer`, `commits` is
    /// bumped, and `size`/`fill` are remembered for [`damage_hint`](TestWindow::damage_hint)
    /// and [`size`](TestWindow::size). A commit before the first `apply_configure` is a
    /// protocol error and fails here with
    /// [`TestkitError::NoPendingConfigure`].
    pub fn commit_frame(&self, fill: FillPattern) -> Result<()> {
        commit_buffer(&self.surface, &self.state, &self.conn, fill)
    }

    /// Re-commits the currently attached buffer without allocating.
    ///
    /// `surface.attach(Some(attached_buffer.buffer()), 0, 0)` followed by
    /// `surface.commit()`; [`TestkitError::NoPendingConfigure`] when nothing was
    /// committed before, [`TestkitError::SurfaceDestroyed`] after [`destroy`](TestWindow::destroy).
    /// Used by [`resize`](TestWindow::resize) and by tests that need a second commit of
    /// identical pixels (damage is empty for the second one).
    pub fn commit_pending(&self) -> Result<()> {
        recommit_buffer(&self.surface, &self.state, &self.conn)
    }

    /// Waits for the next complete xdg configure (role event + serial).
    ///
    /// An already-pending configure (drained from the channel) is returned before blocking;
    /// otherwise the call waits with `recv_timeout(timeout)` on the channel fed by the
    /// reader thread. Expiry is [`TestkitError::Timeout`] with `what = "xdg configure"`, a disconnected
    /// sender (surface destroyed / connection closed) is [`TestkitError::ConnectionClosed`].
    pub fn wait_for_configure(&self, timeout: Duration) -> Result<ConfiguredSize> {
        await_configure(&self.configure_rx, timeout)
    }

    /// Acknowledges the pending configure and clears it.
    ///
    /// Takes `pending_serial`, sends `xdg_surface.ack_configure(serial)`, sets
    /// `applied_serial` and updates `size` from the acknowledged configure;
    /// [`TestkitError::NoPendingConfigure`] when no configure is waiting. The caller is
    /// expected to commit a buffer of that size next (that is what makes the surface
    /// mapped).
    pub fn apply_configure(&self) -> Result<()> {
        ack_pending_configure(&self.xdg_surface, &self.state, &self.conn)
    }

    /// The damage region of the most recent commit, or `None` before the first commit.
    ///
    /// [`commit_frame`](TestWindow::commit_frame) damages the whole surface, so the hint is
    /// `Rect::new(0, 0, w, h)` for the size of the last committed buffer and tests can
    /// assert the compositor observed exactly that.
    pub fn damage_hint(&self) -> Option<Rect> {
        lock_window(&self.state).last_damage
    }

    /// Changes the requested size and re-commits the current buffer.
    ///
    /// `size` is stored in the window state, then [`commit_pending`](TestWindow::commit_pending) runs;
    /// the compositor answers with a new configure that must be applied before a buffer of
    /// the new size is legal. [`TestkitError::SurfaceDestroyed`] after
    /// [`destroy`](TestWindow::destroy).
    pub fn resize(&self, size: Size) -> Result<()> {
        {
            let mut window = lock_window(&self.state);
            if window.destroyed {
                return Err(TestkitError::SurfaceDestroyed);
            }
            window.size = size;
        }
        // The compositor answers the re-commit with a fresh configure; a buffer of the new
        // size is only legal after that configure was applied.
        self.commit_pending()
    }

    /// Sends `xdg_toplevel.set_title` and mirrors the title into the window state;
    /// [`TestkitError::SurfaceDestroyed`] after [`destroy`](TestWindow::destroy).
    pub fn set_title(&self, title: &str) -> Result<()> {
        let mut window = lock_window(&self.state);
        if window.destroyed {
            return Err(TestkitError::SurfaceDestroyed);
        }
        self.toplevel.set_title(title.to_string());
        window.title = title.to_string();
        drop(window);
        flush(&self.conn)
    }

    /// Sends `xdg_toplevel.set_app_id` and mirrors it into the window state;
    /// [`TestkitError::SurfaceDestroyed`] after [`destroy`](TestWindow::destroy).
    pub fn set_app_id(&self, app_id: &str) -> Result<()> {
        let mut window = lock_window(&self.state);
        if window.destroyed {
            return Err(TestkitError::SurfaceDestroyed);
        }
        self.toplevel.set_app_id(app_id.to_string());
        window.app_id = app_id.to_string();
        drop(window);
        flush(&self.conn)
    }

    /// Destroys the toplevel, its xdg surface and its `wl_surface`.
    ///
    /// Idempotent — the first call sends `xdg_toplevel.destroy()`, `xdg_surface.destroy()`
    /// and `wl_surface.destroy()` (in that order), sets `destroyed`, removes the slot from
    /// `ClientState::windows` and returns `Ok(())`; later calls return
    /// [`TestkitError::SurfaceDestroyed`]. Any buffer still attached is
    /// released with the surface.
    pub fn destroy(&self) -> Result<()> {
        // Capture the key before the proxies are gone: the slot is registered under it.
        let surface_id = self.surface.id();
        {
            let mut window = lock_window(&self.state);
            if window.destroyed {
                return Err(TestkitError::SurfaceDestroyed);
            }
            window.destroyed = true;
        }
        // Role object first, then the xdg wrapper, then the surface itself; any buffer
        // still attached is released by the compositor with the surface.
        self.toplevel.destroy();
        self.xdg_surface.destroy();
        self.surface.destroy();
        lock_client(&self.client).windows.remove(&surface_id);
        flush(&self.conn)
    }

    /// Whether [`destroy`](TestWindow::destroy) already ran for this window.
    pub fn is_destroyed(&self) -> bool {
        lock_window(&self.state).destroyed
    }

    /// Whether the compositor sent `xdg_toplevel.close` for this window.
    ///
    /// The client deliberately keeps the surface alive after `close`, so a test can
    /// observe the request here and only then call [`destroy`](TestWindow::destroy).
    pub fn close_requested(&self) -> bool {
        lock_window(&self.state).close_requested
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
    /// Client state shared with the reader thread; used to deregister the slot on
    /// [`destroy`](TestPopup::destroy).
    client: Arc<Mutex<ClientState>>,
    /// The request-side connection, so every mutation reaches the runtime even when the
    /// caller never pumps (see [`flush`]).
    conn: Connection,
}

impl TestPopup {
    /// Wires a new popup handle to its protocol objects (client-internal).
    pub(crate) fn new(
        surface: WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
        state: WindowSlot,
        configure_rx: mpsc::Receiver<ConfiguredSize>,
        client: Arc<Mutex<ClientState>>,
        conn: Connection,
    ) -> TestPopup {
        TestPopup {
            surface,
            xdg_surface,
            popup,
            state,
            configure_rx: Mutex::new(configure_rx),
            client,
            conn,
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
        commit_buffer(&self.surface, &self.state, &self.conn, fill)
    }

    /// Waits for the next complete xdg configure of this popup.
    ///
    /// Same rules as [`TestWindow::wait_for_configure`], with
    /// [`TestkitError::Timeout`] `what = "xdg configure"`.
    pub fn wait_for_configure(&self, timeout: Duration) -> Result<ConfiguredSize> {
        await_configure(&self.configure_rx, timeout)
    }

    /// Acknowledges the pending popup configure.
    ///
    /// Sends `xdg_surface.ack_configure(pending_serial)` and clears the pending state;
    /// [`TestkitError::NoPendingConfigure`] when nothing is pending.
    pub fn apply_configure(&self) -> Result<()> {
        ack_pending_configure(&self.xdg_surface, &self.state, &self.conn)
    }

    /// Destroys the popup, its xdg surface and its `wl_surface`; idempotent, like
    /// [`TestWindow::destroy`].
    pub fn destroy(&self) -> Result<()> {
        let surface_id = self.surface.id();
        {
            let mut window = lock_window(&self.state);
            if window.destroyed {
                return Err(TestkitError::SurfaceDestroyed);
            }
            window.destroyed = true;
        }
        // Role object first, then the xdg wrapper, then the surface itself (see
        // [`TestWindow::destroy`]).
        self.popup.destroy();
        self.xdg_surface.destroy();
        self.surface.destroy();
        lock_client(&self.client).windows.remove(&surface_id);
        flush(&self.conn)
    }
}
