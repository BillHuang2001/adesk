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
//! ## Implementation state
//!
//! The client is fully implemented: the reader thread owns the `EventQueue` and dispatches
//! into `Arc<Mutex<ClientState>>`, every pump/wait is deadline-bounded, and teardown
//! interrupts the reader by shutting the socket down rather than blocking on a join.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use wayland_client::backend::WaylandError;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_shm;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_positioner;

use crate::error::{Result, TestkitError};
use crate::fill::FillPattern;

mod protocol;
mod shm;
mod state;
mod window;

pub use protocol::Globals;
pub use window::{ConfiguredSize, PopupSpec, TestPopup, TestWindow, ToplevelSpec};

use shm::ShmPool;
use state::{lock_client, ClientState, WindowSlot};

/// Internal deadline for [`WaylandTestClient::roundtrip`].
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(5);

/// Internal deadline for joining the reader thread in [`WaylandTestClient::close`].
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bytes reserved for the client's single SHM pool (page aligned).
///
/// A `1280x800` buffer is ~4 MiB, so the pool holds several frames at once; released
/// buffers go back to the pool's free list ([`shm::ShmPool::free`]), so a commit loop
/// reuses one allocation instead of growing the pool.
const SHM_POOL_CAPACITY: usize = 16 * 1024 * 1024;

/// How long the reader thread sleeps between non-blocking read attempts.
///
/// `wayland-client` reads with `MSG_DONTWAIT`, so the reader would spin at 100% CPU if it
/// retried immediately; this bounds both the idle wake-up rate and the extra latency a
/// dispatched event can see. `close` sets `ClientState::closed` and shuts the socket
/// down, so the reader notices teardown within one interval.
const READER_POLL_INTERVAL: Duration = Duration::from_millis(5);

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
    /// The client's single SHM pool, shared with every window it creates.
    pool: Arc<Mutex<ShmPool>>,
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
    /// Reads `$XDG_RUNTIME_DIR` (missing or non-absolute is
    /// [`TestkitError::WaylandConnect`](crate::TestkitError::WaylandConnect)) and delegates to
    /// [`connect_in`](WaylandTestClient::connect_in). Tests that own their runtime should
    /// prefer `connect_in`, which never consults the environment.
    pub fn connect(display_name: &str) -> Result<WaylandTestClient> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            TestkitError::WaylandConnect(
                "XDG_RUNTIME_DIR is not set, so the wayland socket cannot be resolved".to_string(),
            )
        })?;
        let runtime_dir = PathBuf::from(runtime_dir);
        if !runtime_dir.is_absolute() {
            return Err(TestkitError::WaylandConnect(format!(
                "XDG_RUNTIME_DIR must be absolute, got {}",
                runtime_dir.display()
            )));
        }
        WaylandTestClient::connect_in(&runtime_dir, display_name)
    }

    /// Connects to `runtime_dir/display_name` by absolute path.
    ///
    /// Steps:
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
    ///    `xdg_wm_base` (v1+) at `min(server, interface_max)`; a missing global is
    ///    [`TestkitError::Unsupported`](crate::TestkitError::Unsupported).
    /// 5. Spawn the reader thread described in the module docs with a clone of `state`,
    ///    the event queue and an `UnboundedSender<PumpEvent>`, then wait (bounded by
    ///    [`ROUNDTRIP_TIMEOUT`]) for the real `wl_shm.format` events to arrive. A runtime
    ///    that never advertises `ARGB8888` is
    ///    [`TestkitError::Unsupported`](crate::TestkitError::Unsupported): every buffer the
    ///    harness commits is `Argb8888`, so a silent downgrade would corrupt every pixel
    ///    assertion.
    pub fn connect_in(runtime_dir: &Path, display_name: &str) -> Result<WaylandTestClient> {
        let path = runtime_dir.join(display_name);
        let stream = UnixStream::connect(&path).map_err(|err| {
            TestkitError::WaylandConnect(format!(
                "cannot connect to the wayland socket {}: {err}",
                path.display()
            ))
        })?;
        // A clone is the only way to force EOF on the reader thread's blocking wait.
        let socket = stream.try_clone().map_err(|err| {
            TestkitError::WaylandConnect(format!(
                "cannot clone the wayland socket {}: {err}",
                path.display()
            ))
        })?;
        let conn = Connection::from_socket(stream).map_err(|err| {
            TestkitError::WaylandConnect(format!(
                "cannot wrap the wayland socket {}: {err}",
                path.display()
            ))
        })?;
        let (globals_list, mut event_queue) =
            registry_queue_init::<ClientState>(&conn).map_err(|err| {
                TestkitError::WaylandConnect(format!("wayland registry init failed: {err}"))
            })?;
        let globals_snapshot = globals_list.contents().clone_list();
        let qhandle = event_queue.handle();
        let mut globals = protocol::bind_globals(&globals_list, &qhandle)?;
        let pool = Arc::new(Mutex::new(ShmPool::new(
            globals.shm(),
            &qhandle,
            SHM_POOL_CAPACITY,
        )?));

        // The advertised formats start empty: `wait_for_shm_formats` must observe the
        // real `wl_shm.format` events before `supports_argb8888` means anything (a
        // pre-seeded set would make that check vacuous).
        let state = Arc::new(Mutex::new(ClientState::new(
            globals_snapshot,
            HashSet::new(),
        )));
        let (pump_tx, mut pump_rx) = unbounded_channel();
        let reader_state = Arc::clone(&state);
        let reader = std::thread::Builder::new()
            .name(format!("adesk-testkit-wayland-{display_name}"))
            .spawn(move || reader_loop(&mut event_queue, reader_state, pump_tx))
            .map_err(|err| {
                TestkitError::WaylandConnect(format!(
                    "cannot spawn the wayland reader thread: {err}"
                ))
            })?;

        // `wl_shm.format` events only arrive once the bind requests are on the wire.
        conn.flush().map_err(|err| map_wayland_error(&err))?;
        wait_for_shm_formats(&state, &mut pump_rx, ROUNDTRIP_TIMEOUT)?;
        // Publish the formats the runtime actually advertised.
        globals.shm_formats = lock_client(&state).shm_formats.clone();

        Ok(WaylandTestClient {
            conn,
            qhandle,
            state,
            pool,
            globals,
            display_name: display_name.to_string(),
            socket: Some(socket),
            pump_rx,
            reader: Some(reader),
            closed: false,
        })
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
    /// Steps (all objects dispatched on `self.qhandle`, all user-data the new window's
    /// slot):
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
        let (configure_tx, configure_rx) = std::sync::mpsc::channel();
        let slot: WindowSlot = Arc::new(Mutex::new(state::WindowState::new(
            spec.size,
            spec.fill,
            spec.app_id.clone(),
            spec.title.clone(),
            configure_tx,
            Arc::clone(&self.pool),
        )));
        let surface = self
            .globals
            .compositor()
            .create_surface(&self.qhandle, Arc::clone(&slot));
        let xdg_surface =
            self.globals
                .xdg_wm_base()
                .get_xdg_surface(&surface, &self.qhandle, Arc::clone(&slot));
        let toplevel = xdg_surface.get_toplevel(&self.qhandle, Arc::clone(&slot));
        toplevel.set_app_id(spec.app_id.clone());
        toplevel.set_title(spec.title.clone());
        lock_client(&self.state)
            .windows
            .insert(surface.id(), Arc::clone(&slot));
        // The initial commit carries no buffer: it is what makes the compositor send the
        // first configure.
        surface.commit();
        self.conn.flush().map_err(|err| map_wayland_error(&err))?;
        Ok(TestWindow::new(
            surface,
            xdg_surface,
            toplevel,
            slot,
            configure_rx,
            spec,
            Arc::clone(&self.state),
            self.conn.clone(),
        ))
    }

    /// Creates an `xdg_popup` on `parent`'s xdg surface and returns its handle.
    ///
    /// Steps:
    ///
    /// 1. `globals.xdg_wm_base().create_positioner(&qhandle, ())`, then
    ///    `positioner.set_size(w, h)`, `set_anchor_rect(0, 0, parent_w, parent_h)`,
    ///    `set_offset(offset.x, offset.y)`, `set_anchor(Anchor::TopLeft)`,
    ///    `set_gravity(Gravity::BottomRight)` and
    ///    `set_constraint_adjustment(ConstraintAdjustment::all())`. That combination puts
    ///    the popup's top-left corner at `spec.offset` relative to the parent's top-left
    ///    (the anchor point is the anchor rect's top-left corner and the surface grows
    ///    towards the bottom-right of it) while still letting the compositor
    ///    slide/flip it to stay on screen.
    /// 2. `globals.compositor().create_surface(&qhandle, slot.clone())` and
    ///    `globals.xdg_wm_base().get_xdg_surface(&surface, &qhandle, slot.clone())`.
    /// 3. `xdg_surface.get_popup(Some(parent.surface()), &positioner, &qhandle,
    ///    slot.clone())`, then `positioner.destroy()` (the compositor copied the rules).
    /// 4. Register the slot in `ClientState::windows`, `surface.commit()` and
    ///    `conn.flush()`.
    pub fn create_popup(&self, parent: &TestWindow, spec: PopupSpec) -> Result<TestPopup> {
        let (configure_tx, configure_rx) = std::sync::mpsc::channel();
        let slot: WindowSlot = Arc::new(Mutex::new(state::WindowState::new(
            spec.size,
            FillPattern::default(),
            String::new(),
            String::new(),
            configure_tx,
            Arc::clone(&self.pool),
        )));
        let positioner = self
            .globals
            .xdg_wm_base()
            .create_positioner(&self.qhandle, ());
        positioner.set_size(spec.size.w as i32, spec.size.h as i32);
        let parent_size = parent.size();
        positioner.set_anchor_rect(0, 0, parent_size.w as i32, parent_size.h as i32);
        positioner.set_offset(spec.offset.x, spec.offset.y);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        positioner.set_constraint_adjustment(xdg_positioner::ConstraintAdjustment::all());
        let surface = self
            .globals
            .compositor()
            .create_surface(&self.qhandle, Arc::clone(&slot));
        let xdg_surface =
            self.globals
                .xdg_wm_base()
                .get_xdg_surface(&surface, &self.qhandle, Arc::clone(&slot));
        let popup = xdg_surface.get_popup(
            Some(parent.xdg_surface()),
            &positioner,
            &self.qhandle,
            Arc::clone(&slot),
        );
        positioner.destroy();
        lock_client(&self.state)
            .windows
            .insert(surface.id(), Arc::clone(&slot));
        surface.commit();
        self.conn.flush().map_err(|err| map_wayland_error(&err))?;
        Ok(TestPopup::new(
            surface,
            xdg_surface,
            popup,
            slot,
            configure_rx,
            Arc::clone(&self.state),
            self.conn.clone(),
        ))
    }

    /// Drains reader notifications for at most `duration` and returns what was seen.
    ///
    /// `tokio::time::timeout(duration, pump_rx.recv())` in a loop, accumulating
    /// [`PumpStats`]. `PumpEvent::Dispatched` adds one dispatch and `events` events;
    /// `PumpEvent::Closed` sets `closed` and returns immediately; `PumpEvent::Error`
    /// returns [`TestkitError::Wayland`](crate::TestkitError::Wayland). Expiry is *not* an error: the accumulated stats
    /// are returned, which is what makes this a "pump for a while" primitive. Already
    /// buffered notifications are drained even when `duration` is zero.
    pub async fn pump_for(&mut self, duration: Duration) -> Result<PumpStats> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut stats = PumpStats::default();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.pump_rx.recv()).await {
                // The deadline expired: a "pump for a while" is not an error.
                Err(_expired) => return Ok(stats),
                // The reader thread exited without a terminal notification (only possible
                // if it panicked); the connection is as dead as an explicit EOF.
                Ok(None) => {
                    self.closed = true;
                    stats.closed = true;
                    return Ok(stats);
                }
                Ok(Some(PumpEvent::Dispatched { events })) => {
                    stats.dispatches += 1;
                    stats.events += events;
                }
                Ok(Some(PumpEvent::Closed)) => {
                    self.closed = true;
                    stats.closed = true;
                    return Ok(stats);
                }
                Ok(Some(PumpEvent::Error(message))) => {
                    return Err(TestkitError::Wayland(message));
                }
            }
        }
    }

    /// Pumps until `pred` accepts the accumulated stats or `timeout` expires.
    ///
    /// Like [`pump_for`](WaylandTestClient::pump_for) but the loop ends as soon as
    /// `pred(&stats)` is `true`; the predicate is checked after every notification and once
    /// before the first receive (so an already-satisfied condition returns immediately).
    /// Expiry returns [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what` and `timeout`; EOF returns
    /// [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) *before* the deadline (a closed connection can
    /// never satisfy a predicate, so waiting for the timeout would be a lie).
    pub async fn pump_until(
        &mut self,
        timeout: Duration,
        what: &'static str,
        mut pred: impl FnMut(&PumpStats) -> bool,
    ) -> Result<PumpStats> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut stats = PumpStats::default();
        if pred(&stats) {
            return Ok(stats);
        }
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.pump_rx.recv()).await {
                Err(_expired) => return Err(TestkitError::Timeout { what, timeout }),
                Ok(None) => return Err(TestkitError::ConnectionClosed),
                Ok(Some(PumpEvent::Dispatched { events })) => {
                    stats.dispatches += 1;
                    stats.events += events;
                    if pred(&stats) {
                        return Ok(stats);
                    }
                }
                Ok(Some(PumpEvent::Closed)) => {
                    self.closed = true;
                    return Err(TestkitError::ConnectionClosed);
                }
                Ok(Some(PumpEvent::Error(message))) => {
                    return Err(TestkitError::Wayland(message));
                }
            }
        }
    }

    /// Flushes pending requests and waits for one full reader cycle.
    ///
    /// `conn.flush()` then the next [`PumpEvent`] with an internal
    /// [`ROUNDTRIP_TIMEOUT`] bound; [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what = "wayland
    /// roundtrip"` on expiry and [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) on EOF. The read
    /// happens on the reader thread, never here, so this can be called from async tests
    /// without blocking the executor.
    pub async fn roundtrip(&mut self) -> Result<()> {
        self.flush()?;
        match tokio::time::timeout(ROUNDTRIP_TIMEOUT, self.pump_rx.recv()).await {
            Err(_expired) => Err(TestkitError::Timeout {
                what: "wayland roundtrip",
                timeout: ROUNDTRIP_TIMEOUT,
            }),
            Ok(None) => Err(TestkitError::ConnectionClosed),
            Ok(Some(PumpEvent::Dispatched { .. })) => Ok(()),
            Ok(Some(PumpEvent::Closed)) => {
                self.closed = true;
                Err(TestkitError::ConnectionClosed)
            }
            Ok(Some(PumpEvent::Error(message))) => Err(TestkitError::Wayland(message)),
        }
    }

    /// Flushes pending requests to the compositor.
    ///
    /// `self.conn.flush()`, mapping `WaylandError::Io` to
    /// [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) when the socket is gone and to
    /// [`TestkitError::Wayland`](crate::TestkitError::Wayland) otherwise. Requests are buffered by `wayland-client`, so
    /// callers that do not pump may need this to make a request visible to the runtime.
    pub fn flush(&mut self) -> Result<()> {
        self.conn.flush().map_err(|err| map_wayland_error(&err))
    }

    /// Closes the connection and joins the reader thread with a bounded wait.
    ///
    /// Marks `closed`, `socket.shutdown(Shutdown::Both)` (the reader's blocking wait then
    /// fails with EOF and the thread sends [`PumpEvent::Closed`] and exits), takes the
    /// reader handle and joins it inside
    /// `tokio::time::timeout(CLOSE_TIMEOUT, tokio::task::spawn_blocking(...))`;
    /// [`TestkitError::Timeout`](crate::TestkitError::Timeout) with `what = "wayland reader thread"` if it does not exit.
    /// Idempotent and safe to call after the reader already died. Dropping the client
    /// without calling this detaches the reader instead of waiting (see the module docs).
    pub async fn close(mut self) -> Result<()> {
        self.closed = true;
        lock_client(&self.state).closed = true;
        if let Some(socket) = self.socket.take() {
            // Forcing EOF is the only way to interrupt the reader's blocking wait.
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(reader) = self.reader.take() {
            let joined = tokio::time::timeout(
                CLOSE_TIMEOUT,
                tokio::task::spawn_blocking(move || reader.join()),
            )
            .await;
            match joined {
                Err(_expired) => {
                    return Err(TestkitError::Timeout {
                        what: "wayland reader thread",
                        timeout: CLOSE_TIMEOUT,
                    })
                }
                Ok(Err(join_error)) => {
                    return Err(TestkitError::Wayland(format!(
                        "the wayland reader task failed: {join_error}"
                    )))
                }
                Ok(Ok(Err(_panic))) => {
                    return Err(TestkitError::Wayland(
                        "the wayland reader thread panicked".to_string(),
                    ))
                }
                Ok(Ok(Ok(()))) => {}
            }
        }
        Ok(())
    }
}

impl Drop for WaylandTestClient {
    fn drop(&mut self) {
        // Never blocks: mark the client closed and force EOF so the reader thread stops
        // polling, then detach its handle. `close` does the bounded join.
        self.closed = true;
        lock_client(&self.state).closed = true;
        if let Some(socket) = self.socket.take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.reader.take();
    }
}

/// The reader thread: read the socket, dispatch into [`ClientState`], report one
/// [`PumpEvent`] per completed cycle.
///
/// The state lock is taken only around `dispatch_pending`, never across the wait for
/// readability (see the module docs). EOF ends the loop with [`PumpEvent::Closed`], a
/// dispatch/protocol error with [`PumpEvent::Error`]; either way `ClientState::closed` is
/// set before the thread exits.
fn reader_loop(
    event_queue: &mut EventQueue<ClientState>,
    state: Arc<Mutex<ClientState>>,
    pump_tx: UnboundedSender<PumpEvent>,
) {
    loop {
        // `close`/`drop` mark the client closed and shut the socket down; the reader has
        // no other way to observe teardown, because the backend reads are non-blocking.
        if lock_client(&state).closed {
            let _ = pump_tx.send(PumpEvent::Closed);
            break;
        }
        // `None` means a read is already in flight (only possible with a second reader);
        // whatever is queued is dispatched below.
        if let Some(guard) = event_queue.prepare_read() {
            match guard.read() {
                Ok(_read) => {}
                Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => {
                    // No message yet. Sleeping bounds the poll rate; the backend read is
                    // `MSG_DONTWAIT`, so a plain retry would spin at 100% CPU.
                    std::thread::sleep(READER_POLL_INTERVAL);
                    continue;
                }
                // EOF / a dead socket: the runtime is gone, no more events can arrive.
                Err(WaylandError::Io(_)) => {
                    let _ = pump_tx.send(PumpEvent::Closed);
                    break;
                }
                Err(err) => {
                    let _ = pump_tx.send(PumpEvent::Error(err.to_string()));
                    break;
                }
            }
        }
        match event_queue.dispatch_pending(&mut *lock_client(&state)) {
            Ok(events) => {
                if pump_tx.send(PumpEvent::Dispatched { events }).is_err() {
                    // The client was dropped; nothing can consume notifications anymore.
                    break;
                }
            }
            Err(err) => {
                let _ = pump_tx.send(PumpEvent::Error(err.to_string()));
                break;
            }
        }
    }
    lock_client(&state).closed = true;
}

/// Waits (bounded) until the reader thread has dispatched the real `wl_shm.format` events.
///
/// `connect_in` is synchronous, so it polls the reader's notification channel instead of
/// awaiting it; the loop exits as soon as `ARGB8888` is known, which is the first reader
/// cycle in practice.
fn wait_for_shm_formats(
    state: &Arc<Mutex<ClientState>>,
    pump_rx: &mut UnboundedReceiver<PumpEvent>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if lock_client(state)
            .shm_formats
            .contains(&wl_shm::Format::Argb8888)
        {
            return Ok(());
        }
        match pump_rx.try_recv() {
            Ok(PumpEvent::Dispatched { .. }) | Err(TryRecvError::Empty) => {}
            Ok(PumpEvent::Closed) | Err(TryRecvError::Disconnected) => {
                return Err(TestkitError::WaylandConnect(
                    "the runtime closed the connection while the test client bound wl_shm"
                        .to_string(),
                ))
            }
            Ok(PumpEvent::Error(message)) => return Err(TestkitError::Wayland(message)),
        }
        if Instant::now() >= deadline {
            return Err(TestkitError::Unsupported(format!(
                "wl_shm never advertised ARGB8888 within {timeout:?}; the test client commits \
                 Argb8888 buffers"
            )));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Maps a `wayland-client` error onto the harness error type.
///
/// A hung-up socket is [`TestkitError::ConnectionClosed`] (the runtime is gone); every
/// other failure keeps the protocol rendering so a failing test shows what the server
/// said.
pub(crate) fn map_wayland_error(err: &WaylandError) -> TestkitError {
    match err {
        WaylandError::Io(io) if is_disconnected(io) => TestkitError::ConnectionClosed,
        other => TestkitError::Wayland(other.to_string()),
    }
}

/// Whether an I/O error means the peer hung up rather than a transient failure.
fn is_disconnected(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::NotConnected
            | ErrorKind::UnexpectedEof
    )
}
