//! Shared protocol state and the `wayland-client` [`Dispatch`] implementations.
//!
//! [`ClientState`] is the event-queue state of the reader thread: the reader locks it
//! around `dispatch_pending`. Per-window data lives in a [`WindowSlot`]
//! (`Arc<Mutex<WindowState>>`) which is passed to `wayland-client` as the object user-data
//! for `wl_surface`/`xdg_surface`/`xdg_toplevel`/`xdg_popup`, so the same slot is shared
//! by the window handle (locked from the calling thread) and the dispatch impls (locked
//! from the reader thread).
//!
//! Lock order is always `ClientState` → `WindowState`; no dispatch impl locks a window
//! slot and then the client state, so the two mutexes cannot deadlock.
//!
//! ## Configure sequencing
//!
//! An xdg configure is two events: the role event (`xdg_toplevel.configure` with
//! width/height/states, or `xdg_popup.configure` with x/y/width/height) and
//! `xdg_surface.configure` carrying the serial. The dispatch impls therefore store the
//! role event in `WindowState::pending_configure` (with `serial = 0`, a placeholder) and
//! complete it when `xdg_surface.configure` arrives: fill in the serial, move it to
//! `last_configure`, set `pending_serial`, and send a copy on `configure_tx` so
//! [`TestWindow::wait_for_configure`](super::TestWindow::wait_for_configure) can block on
//! `recv_timeout`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};

use adesk_core::Size;
use wayland_client::backend::ObjectId;
use wayland_client::globals::{Global, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

use crate::fill::FillPattern;

use super::shm::ShmBuffer;
use super::window::ConfiguredSize;

/// A window's state, shared between its handle and the dispatch impls.
pub(crate) type WindowSlot = Arc<Mutex<WindowState>>;

/// Event-queue state shared by the client and the reader thread.
pub(crate) struct ClientState {
    /// Snapshot of the registry contents, refreshed on every `wl_registry` event.
    ///
    /// The authoritative list lives in the registry object data
    /// (`GlobalListContents`, which cannot be owned or cloned as a whole); this snapshot
    /// exists so the client can inspect what was advertised without reaching into the
    /// event queue.
    pub(crate) globals: Vec<Global>,
    /// Every live surface, keyed by the `wl_surface` object id.
    pub(crate) windows: HashMap<ObjectId, WindowSlot>,
    /// SHM formats the compositor advertised (`wl_shm.format`).
    pub(crate) shm_formats: HashSet<wl_shm::Format>,
    /// Set when the reader thread reported EOF or a fatal protocol error.
    pub(crate) closed: bool,
}

impl ClientState {
    /// Creates empty client state seeded with the connect-time globals snapshot.
    pub(crate) fn new(globals: Vec<Global>, shm_formats: HashSet<wl_shm::Format>) -> ClientState {
        ClientState {
            globals,
            windows: HashMap::new(),
            shm_formats,
            closed: false,
        }
    }
}

/// Everything the client knows about one surface.
///
/// One `WindowState` exists per `wl_surface`, whether it carries a toplevel or a popup
/// role; `size`/`fill` are seeded from the spec so they are meaningful before the first
/// configure or commit.
pub(crate) struct WindowState {
    /// Role event of a configure sequence that has not seen `xdg_surface.configure` yet.
    pub(crate) pending_configure: Option<ConfiguredSize>,
    /// The most recent complete configure (serial included).
    pub(crate) last_configure: Option<ConfiguredSize>,
    /// Serial of the configure that is waiting for `ack_configure`.
    pub(crate) pending_serial: Option<u32>,
    /// Serial of the configure most recently acknowledged.
    pub(crate) applied_serial: Option<u32>,
    /// Current surface size in pixels (configured, else the requested size).
    pub(crate) size: Size,
    /// Fill pattern used by the next [`commit_frame`](super::TestWindow::commit_frame).
    pub(crate) fill: FillPattern,
    /// Current `app_id` (empty for popups).
    pub(crate) app_id: String,
    /// Current title (empty for popups).
    pub(crate) title: String,
    /// Number of committed frames (the commit watermark tests correlate against).
    pub(crate) commits: u64,
    /// The surface was destroyed by the client.
    pub(crate) destroyed: bool,
    /// The compositor sent `xdg_toplevel.close` / `xdg_popup.popup_done`.
    pub(crate) close_requested: bool,
    /// Buffer allocated for the next commit, before it is attached.
    pub(crate) pending_buffer: Option<ShmBuffer>,
    /// Buffer currently attached to the surface.
    pub(crate) attached_buffer: Option<ShmBuffer>,
    /// Notifies [`TestWindow::wait_for_configure`](super::TestWindow::wait_for_configure).
    pub(crate) configure_tx: Sender<ConfiguredSize>,
}

impl WindowState {
    /// Creates window state for a surface with the given initial size and fill.
    pub(crate) fn new(
        size: Size,
        fill: FillPattern,
        app_id: String,
        title: String,
        configure_tx: Sender<ConfiguredSize>,
    ) -> WindowState {
        WindowState {
            pending_configure: None,
            last_configure: None,
            pending_serial: None,
            applied_serial: None,
            size,
            fill,
            app_id,
            title,
            commits: 0,
            destroyed: false,
            close_requested: false,
            pending_buffer: None,
            attached_buffer: None,
            configure_tx,
        }
    }
}

/// Locks a window slot, ignoring poisoning.
///
/// A panic in a `todo!()` dispatch body poisons the mutex; the test harness must keep
/// reporting the real failure (a panic in a test) instead of cascading `PoisonError`s, so
/// poisoned state is used as-is.
pub(crate) fn lock_window(slot: &WindowSlot) -> MutexGuard<'_, WindowState> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks the client state, ignoring poisoning (see [`lock_window`]).
pub(crate) fn lock_client(state: &Mutex<ClientState>) -> MutexGuard<'_, ClientState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- Dispatch implementations -------------------------------------------------------
//
// Phase 1: every body is `todo!()` with the exact Phase-2 semantics in comments. Event
// argument types below were verified against the generated bindings
// (wayland-client 0.31.15 `wayland.xml`, wayland-protocols 0.32.13 xdg-shell.xml).

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = (state, data);
        match event {
            // `Global { name: u32, interface: String, version: u32 }` /
            // `GlobalRemove { name: u32 }`.
            //
            // Phase 2: `state.globals = data.clone_list()` — the framework's
            // `GlobalListContents` is already up to date when the event is dispatched.
            wl_registry::Event::Global { .. } | wl_registry::Event::GlobalRemove { .. } => {
                todo!("Phase 2: refresh the globals snapshot from GlobalListContents")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_shm::WlShm,
        event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = state;
        match event {
            // `Format { format: WEnum<wl_shm::Format> }`.
            //
            // Phase 2: record known formats in `state.shm_formats` (`WEnum::Value` only;
            // unknown numeric codes are ignored) so `supports_argb8888` reflects the
            // runtime instead of an assumption.
            wl_shm::Event::Format { .. } => {
                todo!("Phase 2: record the advertised SHM format in state.shm_formats")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_shm_pool::WlShmPool,
        event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `wl_shm_pool` has no events; the wildcard is required because the generated enum
        // is `#[non_exhaustive]`.
        match event {
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = state;
        match event {
            // `Release`.
            //
            // Phase 2: the compositor is done reading the buffer. Return its bytes to the
            // owning pool's free list (`ShmPool::free`) so repeated frames reuse one
            // allocation instead of growing the pool. The pool is found through the
            // window slot that holds the buffer.
            wl_buffer::Event::Release => {
                todo!("Phase 2: return the released buffer to its pool's free list")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_compositor::WlCompositor,
        event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `wl_compositor` has no events; the wildcard is required because the generated
        // enum is `#[non_exhaustive]`.
        match event {
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, WindowSlot> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_surface::WlSurface,
        event: wl_surface::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = (state, data);
        match event {
            // `Enter { output: WlOutput }` / `Leave { output: WlOutput }`.
            //
            // Phase 2: record which outputs the surface is on (the virtual output always
            // enters immediately) so tests can assert the surface was actually mapped.
            wl_surface::Event::Enter { .. } | wl_surface::Event::Leave { .. } => {
                todo!("Phase 2: track output enter/leave on the window slot")
            }
            // `PreferredBufferScale { factor: i32 }` (since v6).
            //
            // Phase 2: the test client always commits unscaled buffers; record the
            // preference so a mismatch is visible in failures instead of silently
            // blurring pixel assertions.
            wl_surface::Event::PreferredBufferScale { .. } => {
                todo!("Phase 2: record the preferred buffer scale")
            }
            // `PreferredBufferTransform { transform: WEnum<wl_output::Transform> }` (v6).
            //
            // Phase 2: record and assert it is `Normal`; a transform would invalidate
            // exact pixel comparisons.
            wl_surface::Event::PreferredBufferTransform { .. } => {
                todo!("Phase 2: record the preferred buffer transform")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        proxy: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = proxy;
        match event {
            // `Ping { serial: u32 }`.
            //
            // Phase 2: `proxy.pong(serial)` — the compositor disconnects unresponsive
            // clients, so the test client must answer immediately.
            xdg_wm_base::Event::Ping { .. } => todo!("Phase 2: proxy.pong(serial)"),
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, WindowSlot> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = (state, data);
        match event {
            // `Configure { serial: u32 }`.
            //
            // Phase 2: complete the configure sequence — take the role event stored by
            // `xdg_toplevel.configure`/`xdg_popup.configure`, set its `serial`, move it to
            // `last_configure`, set `pending_serial = Some(serial)` and send a copy on
            // `configure_tx` (ignore `SendError`: the window was dropped).
            xdg_surface::Event::Configure { .. } => {
                todo!("Phase 2: complete the pending configure with this serial and notify the window")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, WindowSlot> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = (state, data);
        match event {
            // `Configure { width: i32, height: i32, states: Vec<u8> }` (`states` is an
            // array of native-endian `u32` state codes; `width`/`height` of `0` means
            // "client chooses").
            //
            // Phase 2: store `ConfiguredSize { width: width.max(0) as u32, ...,
            // states, serial: 0 }` in `pending_configure` (the serial arrives with the
            // following `xdg_surface.configure`), and update `size` when both dimensions
            // are non-zero.
            xdg_toplevel::Event::Configure { .. } => {
                todo!("Phase 2: store the pending toplevel configure (width/height/states)")
            }
            // `Close`.
            //
            // Phase 2: set `close_requested = true`; the client keeps the surface alive
            // so tests can observe the request before `destroy`.
            xdg_toplevel::Event::Close => todo!("Phase 2: set close_requested"),
            // `ConfigureBounds { width: i32, height: i32 }` (v4+).
            //
            // Phase 2: record the recommended bounds; they do not change the surface size.
            xdg_toplevel::Event::ConfigureBounds { .. } => {
                todo!("Phase 2: record the recommended geometry bounds")
            }
            // `WmCapabilities { capabilities: Vec<u8> }` (v5+).
            //
            // Phase 2: record the capability set so tests can skip capabilities the
            // runtime does not implement.
            xdg_toplevel::Event::WmCapabilities { .. } => {
                todo!("Phase 2: record the advertised WM capabilities")
            }
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<xdg_positioner::XdgPositioner, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &xdg_positioner::XdgPositioner,
        event: xdg_positioner::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `xdg_positioner` has no events; the wildcard is required because the generated
        // enum is `#[non_exhaustive]`.
        match event {
            _ => {}
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, WindowSlot> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = (state, data);
        match event {
            // `Configure { x: i32, y: i32, width: i32, height: i32 }` (the position is
            // relative to the parent's window geometry; there is no `states` array).
            //
            // Phase 2: store `ConfiguredSize { width: width.max(0) as u32, height:
            // height.max(0) as u32, states: Vec::new(), serial: 0 }` plus the placed
            // offset in `pending_configure`; the following `xdg_surface.configure`
            // completes it.
            xdg_popup::Event::Configure { .. } => {
                todo!("Phase 2: store the pending popup configure (x/y/width/height)")
            }
            // `PopupDone`.
            //
            // Phase 2: set `close_requested = true`; the popup is dismissed and must be
            // destroyed by the client.
            xdg_popup::Event::PopupDone => todo!("Phase 2: set close_requested"),
            // `Repositioned { token: u32 }` (v3+).
            //
            // Phase 2: match `token` against the last `xdg_popup.reposition` token so a
            // reposition wait can complete; a configure follows immediately.
            xdg_popup::Event::Repositioned { .. } => {
                todo!("Phase 2: match the reposition token")
            }
            // Generated event enums are `#[non_exhaustive]`; unknown future events are
            // ignored.
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        let _ = state;
        match event {
            // `Done { callback_data: u32 }` (the object is a destructor).
            //
            // Phase 2: mark the matching `wl_display.sync` as complete so
            // `roundtrip` can wait for a *specific* server roundtrip rather than for the
            // next arbitrary reader cycle, and record the callback watermark.
            wl_callback::Event::Done { .. } => {
                todo!("Phase 2: mark the sync callback complete")
            }
            // Generated event enums are `#[non_exhaustive]`; unknown future events are
            // ignored.
            _ => {}
        }
    }
}
