//! The Wayland test client: the real protocol path, known SHM fills, bounded pumps.
//!
//! [`WaylandTestClient`] is an ordinary Wayland client built on `wayland-client` 0.31: it
//! connects to the runtime's socket, binds `wl_compositor`/`wl_shm`/`xdg_wm_base`, creates
//! `xdg_toplevel`/`xdg_popup` surfaces and commits SHM buffers filled with a
//! [`FillPattern`](crate::FillPattern). It never reaches into compositor state, so a test that drives this
//! client exercises exactly what an application would.
//!
//! ## Threading model
//!
//! `wayland-client` is synchronous while tests are async, so the client is split in two:
//!
//! - The **calling thread** owns [`WaylandTestClient`]. It only *sends* requests
//!   (creating surfaces, attaching buffers, committing) through the [`Connection`]; it
//!   never reads the socket.
//! - The **reader thread** owns the [`EventQueue`](wayland_client::EventQueue) created by
//!   `registry_queue_init`. It loops `prepare_read()` → `ReadEventsGuard::read()`
//!   (blocking on the socket) → `dispatch_pending(&mut *state.lock())`, and after every
//!   cycle it sends one `PumpEvent` on an unbounded `tokio::sync::mpsc` channel. A read
//!   error (EOF when the runtime shuts down) ends the loop with `PumpEvent::Closed`; a
//!   dispatch error ends it with `PumpEvent::Error`.
//!
//! Protocol state is shared as `Arc<Mutex<ClientState>>` with a `std::sync::Mutex`: the
//! reader holds the lock only while dispatching, never across the blocking read. Per-window
//! state lives in `Arc<Mutex<WindowState>>` slots stored in `ClientState`, so every window
//! handle holds a `QueueHandle<ClientState>` (which is `Clone + Send`) plus its own slot,
//! and all [`TestWindow`] methods take `&self`.
//!
//! `connect_in` builds the socket from an absolute path (`runtime_dir.join(display_name)`),
//! so it never depends on process environment and parallel tests cannot race on
//! `WAYLAND_DISPLAY`; `connect` resolves `$XDG_RUNTIME_DIR` from the environment and then
//! delegates to `connect_in`.
//!
//! ## Teardown
//!
//! Nothing may block forever. `UnixStream::shutdown` is the only way to interrupt the
//! reader's blocking read (the connection itself cannot be closed while the reader holds
//! its own clone), so the client keeps a *clone* of the socket purely to force EOF:
//! [`WaylandTestClient::close`] marks the client closed, shuts the socket down, and joins
//! the reader thread with a bounded wait. The reader sees EOF, reports
//! `PumpEvent::Closed` and exits. Dropping the client without `close()` shuts the socket
//! down and detaches the reader handle, so `Drop` can never hang a test.
//!
//! ## Deadlines
//!
//! Every public wait/pump takes an explicit [`Duration`]: `pump_until`/`roundtrip` return
//! [`TestkitError::Timeout`](crate::TestkitError::Timeout) when it expires and [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) when
//! the reader reported EOF; `pump_for` is a "run for this long" pump and returns the
//! counters it accumulated (with `closed` set if EOF happened meanwhile). There is no
//! unbounded wait in this module.
//!
//! ## Phase 1 skeleton
//!
//! Public signatures and the data model are final. Every body that touches the protocol,
//! threads, time or I/O is `todo!()` with the exact Phase-2 semantics in its doc comment;
//! only trivial constructors/accessors are implemented.
//!
//! Phase 1 skeleton: most internals are unreachable until the `todo!()` bodies land, so
//! dead-code analysis is disabled for this module tree only.
#![allow(dead_code)]

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use wayland_client::{Connection, QueueHandle};

use crate::error::Result;

mod protocol;
mod shm;
mod state;
mod window;

pub use protocol::Globals;
pub use window::{ConfiguredSize, PopupSpec, TestPopup, TestWindow, ToplevelSpec};

use state::ClientState;

/// Internal deadline for [`WaylandTestClient::roundtrip`].
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(5);

/// Internal deadline for joining the reader thread in [`WaylandTestClient::close`].
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// One notification from the reader thread to the client.
///
/// The reader sends exactly one event per completed read/dispatch cycle, so a
/// [`PumpStats::dispatches`] count equals the number of reader cycles a pump observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PumpEvent {
    /// A reader cycle completed after dispatching `events` protocol events (`0` when the
    /// read only completed a partial message).
    Dispatched {
        /// Number of protocol events dispatched in this cycle.
        events: usize,
    },
    /// The reader observed EOF or an unrecoverable read error and has exited; the
    /// connection is dead. Pumps translate this into `PumpStats::closed` and public waits
    /// into [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed).
    Closed,
    /// The reader hit a dispatch/protocol error and has exited; the payload is the
    /// `Display` rendering of the underlying `DispatchError`. Pumps translate this into
    /// [`TestkitError::Wayland`](crate::TestkitError::Wayland).
    Error(String),
}

/// Counters accumulated by a bounded event pump.
///
/// `Default` is the zero state, which is also what a pump returns when the deadline
/// expires before the reader completes a single cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpStats {
    /// Number of completed reader cycles (read + dispatch) observed by the pump.
    pub dispatches: usize,
    /// Total number of protocol events dispatched in those cycles.
    pub events: usize,
    /// The reader reported EOF / an unrecoverable read error and has exited.
    pub closed: bool,
}

/// A client speaking the real Wayland protocol to a test runtime.
///
/// Construct one with [`WaylandTestClient::connect_in`] (or
/// [`TestRuntime::wayland_client`](crate::TestRuntime::wayland_client)), create windows
/// with [`create_toplevel`](WaylandTestClient::create_toplevel) and pump events with
/// [`pump_for`](WaylandTestClient::pump_for) / [`pump_until`](WaylandTestClient::pump_until).
/// All window methods take `&self`; the client itself is `&mut` only for pumps/teardown.
pub struct WaylandTestClient {
    /// The request-side connection; the reader thread owns the event queue.
    conn: Connection,
    /// Handle used to create objects dispatched on the reader's event queue.
    qhandle: QueueHandle<ClientState>,
    /// Protocol state shared with the reader thread.
    state: Arc<Mutex<ClientState>>,
    /// The globals bound at connect time, with negotiated versions.
    globals: Globals,
    /// The socket name this client connected to, as passed to `connect`/`connect_in`.
    display_name: String,
    /// A clone of the socket, kept only so teardown can force EOF on the reader thread.
    socket: Option<UnixStream>,
    /// Reader-thread notifications.
    pump_rx: UnboundedReceiver<PumpEvent>,
    /// The reader thread; joined by [`WaylandTestClient::close`] with a bounded wait.
    reader: Option<JoinHandle<()>>,
    /// Set once `close` ran or the reader reported EOF.
    closed: bool,
}

impl WaylandTestClient {
    /// Connects to `display_name` inside `$XDG_RUNTIME_DIR`.
    ///
    /// Phase 2: read `$XDG_RUNTIME_DIR` (missing or non-absolute is
    /// [`TestkitError::WaylandConnect`](crate::TestkitError::WaylandConnect)) and delegate to
    /// [`connect_in`](WaylandTestClient::connect_in). Tests that own their runtime should
    /// prefer `connect_in`, which never consults the environment.
    pub fn connect(display_name: &str) -> Result<WaylandTestClient> {
        let _ = display_name;
        todo!("Phase 2: resolve $XDG_RUNTIME_DIR, then delegate to connect_in")
    }

    /// Connects to `runtime_dir/display_name` by absolute path.
    ///
    /// Phase 2 steps:
    ///
    /// 1. `UnixStream::connect(runtime_dir.join(display_name))` — an absolute path, so the
    ///    client never depends on process environment; failures become
    ///    [`TestkitError::WaylandConnect`](crate::TestkitError::WaylandConnect).
    /// 2. Keep `stream.try_clone()?` for teardown (see the module docs) and hand the
    ///    original to `Connection::from_socket`.
    /// 3. `wayland_client::globals::registry_queue_init::<ClientState>(&conn)` →
    ///    `(GlobalList, EventQueue<ClientState>)`; snapshot
    ///    `globals_list.contents().clone_list()` into `ClientState::globals` (a
    ///    `GlobalListContents` cannot be owned — it lives inside the registry object data).
    /// 4. `protocol::bind_globals` binds `wl_compositor` (v4+), `wl_shm` (v1+) and
    ///    `xdg_wm_base` (v1+) at `min(server, interface_max)`; a missing global or a
    ///    `wl_shm` that never advertised `ARGB8888` is [`TestkitError::Unsupported`](crate::TestkitError::Unsupported).
    /// 5. Spawn the reader thread described in the module docs with a clone of `state`,
    ///    the event queue and an `UnboundedSender<PumpEvent>`.
    pub fn connect_in(runtime_dir: &Path, display_name: &str) -> Result<WaylandTestClient> {
        let _ = (runtime_dir, display_name);
        todo!("Phase 2: UnixStream::connect -> Connection::from_socket -> registry_queue_init -> bind_globals -> spawn reader thread")
    }

    /// The socket name this client connected to.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The globals bound at connect time, with negotiated versions and SHM formats.
    pub fn globals(&self) -> &Globals {
        &self.globals
    }

    /// Whether the connection is closed (teardown ran, or the reader saw EOF).
    pub fn is_closed(&self) -> bool {
        self.closed || state::lock_client(&self.state).closed
    }

    /// Creates a mapped `xdg_toplevel` surface and returns its handle.
    ///
    /// Phase 2 steps (all objects dispatched on `self.qhandle`, all user-data the new
    /// window's slot):
    ///
    /// 1. `globals.compositor().create_surface(&qhandle, slot.clone())`.
    /// 2. `globals.xdg_wm_base().get_xdg_surface(&surface, &qhandle, slot.clone())`.
    /// 3. `xdg_surface.get_toplevel(&qhandle, slot.clone())`.
    /// 4. `toplevel.set_app_id(spec.app_id.clone())` and
    ///    `toplevel.set_title(spec.title.clone())`.
    /// 5. Register the slot under the surface's `ObjectId` in `ClientState::windows`,
    ///    then `surface.commit()` *without a buffer*: the initial commit is what maps the
    ///    surface, and the compositor answers with `xdg_toplevel.configure` +
    ///    `xdg_surface.configure` (the client must not commit a buffer before it has a
    ///    configure).
    /// 6. `conn.flush()` so the requests leave the socket before the caller starts
    ///    waiting for the configure.
    ///
    /// The returned handle holds no lock on the client; `spec.size`/`spec.fill` seed the
    /// window state so [`TestWindow::size`]/[`TestWindow::fill`] are meaningful before the
    /// first configure.
    pub fn create_toplevel(&self, spec: ToplevelSpec) -> Result<TestWindow> {
        let _ = spec;
        todo!("Phase 2: create_surface + get_xdg_surface + get_toplevel + set_app_id/set_title + initial commit")
    }

    /// Creates an `xdg_popup` on `parent`'s xdg surface and returns its handle.
    ///
    /// Phase 2 steps:
    ///
    /// 1. `globals.xdg_wm_base().create_positioner(&qhandle, ())`, then
    ///    `positioner.set_size(w, h)`, `set_anchor_rect(0, 0, parent_w, parent_h)`,
    ///    `set_anchor(Anchor::TopLeft)`, `set_gravity(Gravity::BottomRight)` and
    ///    `set_constraint_adjustment(ConstraintAdjustment::all())`. That combination puts
    ///    the popup's top-left corner at `spec.offset` relative to the parent's top-left
    ///    while still letting the compositor slide/flip it to stay on screen.
    /// 2. `globals.compositor().create_surface(&qhandle, slot.clone())` and
    ///    `globals.xdg_wm_base().get_xdg_surface(&surface, &qhandle, slot.clone())`.
    /// 3. `xdg_surface.get_popup(Some(parent.xdg_surface()), &positioner, &qhandle,
    ///    slot.clone())`, then `positioner.destroy()` (the compositor copied the rules).
    /// 4. Register the slot in `ClientState::windows`, `surface.commit()` and
    ///    `conn.flush()`.
    pub fn create_popup(&self, parent: &TestWindow, spec: PopupSpec) -> Result<TestPopup> {
        let _ = (parent, spec);
        todo!("Phase 2: xdg_positioner + get_popup on the parent xdg_surface")
    }

    /// Drains reader notifications for at most `duration` and returns what was seen.
    ///
    /// Phase 2: `tokio::time::timeout(duration, pump_rx.recv())` in a loop, accumulating
    /// [`PumpStats`]. `PumpEvent::Dispatched` adds one dispatch and `events` events;
    /// `PumpEvent::Closed` sets `closed` and returns immediately; `PumpEvent::Error`
    /// returns [`TestkitError::Wayland`](crate::TestkitError::Wayland). Expiry is *not* an error: the accumulated stats
    /// are returned, which is what makes this a "pump for a while" primitive. Already
    /// buffered notifications are drained even when `duration` is zero.
    pub async fn pump_for(&mut self, duration: Duration) -> Result<PumpStats> {
        let _ = duration;
        todo!("Phase 2: bounded recv loop; ConnectionClosed if the reader reported EOF")
    }

    /// Pumps until `pred` accepts the accumulated stats or `timeout` expires.
    ///
    /// Phase 2: like [`pump_for`](WaylandTestClient::pump_for) but the loop ends as soon as
    /// `pred(&stats)` is `true`; the predicate is checked after every notification and once
    /// before the first receive (so an already-satisfied condition returns immediately).
    /// Expiry returns [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what` and `timeout`; EOF returns
    /// [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) *before* the deadline (a closed connection can
    /// never satisfy a predicate, so waiting for the timeout would be a lie).
    pub async fn pump_until(
        &mut self,
        timeout: Duration,
        what: &'static str,
        pred: impl FnMut(&PumpStats) -> bool,
    ) -> Result<PumpStats> {
        let _ = (timeout, what, pred);
        todo!("Phase 2: bounded recv loop with a predicate; Timeout on expiry")
    }

    /// Flushes pending requests and waits for one full reader cycle.
    ///
    /// Phase 2: `conn.flush()` then wait for the next [`PumpEvent`] with an internal
    /// [`ROUNDTRIP_TIMEOUT`] bound; [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what = "wayland
    /// roundtrip"` on expiry and [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) on EOF. The read
    /// happens on the reader thread, never here, so this can be called from async tests
    /// without blocking the executor.
    pub async fn roundtrip(&mut self) -> Result<()> {
        todo!("Phase 2: flush + one bounded reader cycle (5 s)")
    }

    /// Flushes pending requests to the compositor.
    ///
    /// Phase 2: `self.conn.flush()`, mapping `WaylandError::Io` to
    /// [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) when the socket is gone and to
    /// [`TestkitError::Wayland`](crate::TestkitError::Wayland) otherwise. Requests are buffered by `wayland-client`, so
    /// callers that do not pump may need this to make a request visible to the runtime.
    pub fn flush(&mut self) -> Result<()> {
        todo!("Phase 2: connection.flush() with Wayland error mapping")
    }

    /// Closes the connection and joins the reader thread with a bounded wait.
    ///
    /// Phase 2: mark `closed`, `socket.shutdown(Shutdown::Both)` (the reader's blocking
    /// read then fails with EOF and the thread sends [`PumpEvent::Closed`] and exits), take
    /// the reader handle and join it inside
    /// `tokio::time::timeout(CLOSE_TIMEOUT, tokio::task::spawn_blocking(...))`;
    /// [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what = "wayland reader thread"` if it does not exit.
    /// Idempotent and safe to call after the reader already died. Dropping the client
    /// without calling this detaches the reader instead of waiting (see the module docs).
    pub async fn close(mut self) -> Result<()> {
        self.closed = true;
        todo!("Phase 2: shutdown the socket clone, then join the reader thread with a bounded wait")
    }
}

impl Drop for WaylandTestClient {
    fn drop(&mut self) {
        // Never blocks: force EOF so the reader thread can leave its blocking read, then
        // detach its handle. `close` does the bounded join.
        self.closed = true;
        if let Some(socket) = self.socket.take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.reader.take();
    }
}
