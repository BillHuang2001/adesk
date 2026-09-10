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
//! ## Seat input recording
//!
//! `wl_seat.capabilities` decides which input objects the client creates
//! ([`ClientState::pointer`]/[`ClientState::keyboard`]) and every event those objects
//! deliver is appended to [`ClientState::pointer_events`]/[`ClientState::keyboard_events`]
//! in delivery order — see [`super::input`] for the recorded contract. Recording happens
//! in the dispatch bodies (the reader thread already holds the `ClientState` lock), so it
//! never blocks, never touches I/O and is complete even if a test never pumps.
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

use adesk_core::{Point, Rect, Size};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{Global, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry,
    wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

use crate::fill::FillPattern;

use super::input::{self, KeyboardEvent, PointerEvent};
use super::shm::{ShmBuffer, ShmPool};
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
    /// The `wl_seat` this client bound.
    ///
    /// Held here (a clone of `Globals::seat()`) because the seat is the object every input
    /// object is created from: `wl_seat.capabilities` arrives on the reader thread, which
    /// creates/destroys `wl_pointer`/`wl_keyboard` from this handle.
    pub(crate) seat: wl_seat::WlSeat,
    /// The `wl_seat.name` the compositor reported (v2+), when it sent one.
    pub(crate) seat_name: Option<String>,
    /// The live `wl_pointer`, while the seat advertises the pointer capability.
    pub(crate) pointer: Option<wl_pointer::WlPointer>,
    /// The live `wl_keyboard`, while the seat advertises the keyboard capability.
    pub(crate) keyboard: Option<wl_keyboard::WlKeyboard>,
    /// Every `wl_pointer` event recorded, in delivery order (append-only until cleared).
    pub(crate) pointer_events: Vec<PointerEvent>,
    /// Every `wl_keyboard` event recorded, in delivery order (append-only until cleared).
    pub(crate) keyboard_events: Vec<KeyboardEvent>,
    /// Serial of the most recent serial-carrying input event (see [`Self::latest_input_serial`]).
    pub(crate) latest_input_serial: Option<u32>,
    /// Every live surface, keyed by the `wl_surface` object id.
    pub(crate) windows: HashMap<ObjectId, WindowSlot>,
    /// SHM formats the compositor advertised (`wl_shm.format`).
    pub(crate) shm_formats: HashSet<wl_shm::Format>,
    /// Set when the reader thread reported EOF or a fatal protocol error.
    pub(crate) closed: bool,
    /// Number of `wl_display.sync` callbacks the compositor answered.
    ///
    /// Recorded by [`Dispatch<wl_callback::WlCallback, ()>`]; a test that needs a
    /// *specific* server roundtrip can compare it before and after a request, which is
    /// what [`roundtrip`](super::WaylandTestClient::roundtrip) documents as out of scope
    /// for its own "one reader cycle" contract.
    pub(crate) sync_watermark: u64,
}

impl ClientState {
    /// Creates empty client state seeded with the connect-time globals snapshot.
    pub(crate) fn new(
        globals: Vec<Global>,
        shm_formats: HashSet<wl_shm::Format>,
        seat: wl_seat::WlSeat,
    ) -> ClientState {
        ClientState {
            globals,
            seat,
            seat_name: None,
            pointer: None,
            keyboard: None,
            pointer_events: Vec::new(),
            keyboard_events: Vec::new(),
            latest_input_serial: None,
            windows: HashMap::new(),
            shm_formats,
            closed: false,
            sync_watermark: 0,
        }
    }

    /// The serial of the most recent input event that answered with one.
    ///
    /// Exactly three events update it — `wl_pointer.enter`, `wl_pointer.button` and
    /// `wl_keyboard.enter` — because those are the events a client may legally answer with
    /// a request that needs a serial (`wl_data_source.set_selection` in the later clipboard
    /// support). Other serial-carrying events (`wl_pointer.leave`, `wl_keyboard.leave`,
    /// `wl_keyboard.key`, `wl_keyboard.modifiers`) deliberately do *not*: a serial from an
    /// event the compositor sent only to notify is not a grant of input, and the pinned
    /// contract names exactly these three sources.
    ///
    /// Internal plumbing: no public API exposes it yet, and
    /// [`clear_input_events`](super::WaylandTestClient::clear_input_events) keeps it,
    /// because a serial belongs to the seat's input stream rather than to the recorded
    /// event history.
    pub(crate) fn latest_input_serial(&self) -> Option<u32> {
        self.latest_input_serial
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
    /// Buffers whose bytes were already returned to [`Self::pool`] by `wl_buffer.release`.
    ///
    /// A buffer that is re-attached by
    /// [`commit_pending`](super::TestWindow::commit_pending) is released again by the
    /// compositor; the set keeps the second release from handing the same range out
    /// twice (see [`ShmPool::free`]).
    pub(crate) released_buffers: HashSet<ObjectId>,
    /// The SHM pool this window allocates from (one pool per client).
    pub(crate) pool: Arc<Mutex<ShmPool>>,
    /// Damage the last commit reported, or `None` before the first commit.
    pub(crate) last_damage: Option<Rect>,
    /// Outputs the surface entered and has not left, by object id.
    pub(crate) outputs: HashSet<ObjectId>,
    /// `wl_surface.preferred_buffer_scale` (`1` until the compositor says otherwise).
    pub(crate) preferred_buffer_scale: i32,
    /// `wl_surface.preferred_buffer_transform`, when the compositor sent one.
    pub(crate) preferred_buffer_transform: Option<wl_output::Transform>,
    /// `xdg_toplevel.configure_bounds`, when the compositor sent one.
    pub(crate) configure_bounds: Option<Size>,
    /// Raw `xdg_toplevel.wm_capabilities` entries (native-endian `u32` codes).
    pub(crate) wm_capabilities: Vec<u8>,
    /// Offset the compositor placed the popup at (parent-window coordinates).
    pub(crate) popup_offset: Point,
    /// Last `xdg_popup.repositioned` token, when the compositor sent one.
    pub(crate) popup_reposition_token: Option<u32>,
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
        pool: Arc<Mutex<ShmPool>>,
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
            released_buffers: HashSet::new(),
            pool,
            last_damage: None,
            outputs: HashSet::new(),
            preferred_buffer_scale: 1,
            preferred_buffer_transform: None,
            configure_bounds: None,
            wm_capabilities: Vec::new(),
            popup_offset: Point::ORIGIN,
            popup_reposition_token: None,
            configure_tx,
        }
    }
}

/// Locks a window slot, ignoring poisoning.
///
/// Dispatch bodies must not panic, but if one does the mutex is poisoned and the harness
/// must keep reporting the real failure (the panic in the test) instead of cascading
/// `PoisonError`s, so poisoned state is used as-is.
pub(crate) fn lock_window(slot: &WindowSlot) -> MutexGuard<'_, WindowState> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks the client state, ignoring poisoning (see [`lock_window`]).
pub(crate) fn lock_client(state: &Mutex<ClientState>) -> MutexGuard<'_, ClientState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks a window's SHM pool, ignoring poisoning (see [`lock_window`]).
pub(crate) fn lock_pool(pool: &Mutex<ShmPool>) -> MutexGuard<'_, ShmPool> {
    pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- Dispatch implementations -------------------------------------------------------
//
// One `Dispatch` impl per interface this client binds, all implemented. Event argument
// types below were verified against the generated bindings
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
        match event {
            // `Global { name: u32, interface: String, version: u32 }` /
            // `GlobalRemove { name: u32 }`.
            //
            // The framework's `GlobalListContents` is already up to date when the event is
            // dispatched (its object data updates it before forwarding the message), so the
            // snapshot is simply re-read.
            wl_registry::Event::Global { .. } | wl_registry::Event::GlobalRemove { .. } => {
                state.globals = data.clone_list();
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
        // `Format { format: WEnum<wl_shm::Format> }`.
        //
        // Only known formats are recorded so `supports_argb8888` reflects the runtime
        // instead of an assumption; unknown numeric codes are ignored.
        if let wl_shm::Event::Format {
            format: WEnum::Value(format),
        } = event
        {
            state.shm_formats.insert(format);
        }
        // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
        // versions fall through and are ignored rather than rejected.
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Capabilities { capabilities: WEnum<Capability> }`.
            //
            // The client keeps exactly the input objects the seat advertises: the pointer
            // and the keyboard are created when their bit is present and released when it
            // disappears (a seat can change capabilities at any time).
            //
            // `capabilities` is a *bitfield*: the generated binding only knows single-bit
            // values, so a seat with a pointer *and* a keyboard arrives as
            // `WEnum::Unknown(0b11)`. The raw bits are the ground truth
            // (`input::capability_bits`), which is why an unknown numeric code cannot be
            // mistaken for "no capability" — and why nothing here can panic.
            wl_seat::Event::Capabilities { capabilities } => {
                let bits = input::capability_bits(capabilities);
                if bits & input::POINTER_CAPABILITY != 0 {
                    if state.pointer.is_none() {
                        state.pointer = Some(state.seat.get_pointer(qhandle, ()));
                    }
                } else if let Some(pointer) = state.pointer.take() {
                    release_pointer(&pointer);
                }
                if bits & input::KEYBOARD_CAPABILITY != 0 {
                    if state.keyboard.is_none() {
                        state.keyboard = Some(state.seat.get_keyboard(qhandle, ()));
                    }
                } else if let Some(keyboard) = state.keyboard.take() {
                    release_keyboard(&keyboard);
                }
            }
            // `Name { name: String }` (since v2).
            //
            // Recorded so a failure can name the seat the runtime exposed.
            wl_seat::Event::Name { name } => state.seat_name = Some(name),
            // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
            // versions fall through and are ignored rather than rejected.
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Enter { serial, surface, surface_x, surface_y }` — the pointer entered a
            // surface.
            //
            // The position is surface-local, which is what an injected window-relative
            // position becomes once the window model has resolved it. The serial is the one
            // a client may answer with (see `ClientState::latest_input_serial`).
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.latest_input_serial = Some(serial);
                state.pointer_events.push(PointerEvent::Enter {
                    surface: surface.id(),
                    x: surface_x,
                    y: surface_y,
                });
            }
            // `Leave { serial, surface }` — the pointer left a surface.
            wl_pointer::Event::Leave { surface, .. } => {
                state.pointer_events.push(PointerEvent::Leave {
                    surface: surface.id(),
                });
            }
            // `Motion { time, surface_x, surface_y }` — movement on the focused surface.
            wl_pointer::Event::Motion {
                surface_x, surface_y, ..
            } => {
                state.pointer_events.push(PointerEvent::Motion {
                    x: surface_x,
                    y: surface_y,
                });
            }
            // `Button { serial, time, button, state }` — a button changed state.
            //
            // `state` arrives as `WEnum`; a code the pinned bindings do not know is skipped
            // rather than recorded as the wrong button state.
            wl_pointer::Event::Button {
                serial,
                button,
                state: button_state,
                ..
            } => {
                state.latest_input_serial = Some(serial);
                if let Some(button_state) = input::button_state(button_state) {
                    state.pointer_events.push(PointerEvent::Button {
                        button,
                        state: button_state,
                    });
                }
            }
            // `Axis { time, axis, value }` — a scroll axis moved.
            //
            // An axis code the bindings do not know is skipped (recording it as the wrong
            // axis would make a scroll assertion lie).
            wl_pointer::Event::Axis { axis, value, .. } => {
                if let Some(axis) = input::axis_kind(axis) {
                    state.pointer_events.push(PointerEvent::Axis { axis, value });
                }
            }
            // `Frame` (since v5) — the end of a group of pointer events.
            wl_pointer::Event::Frame => state.pointer_events.push(PointerEvent::Frame),
            // Generated event enums are `#[non_exhaustive]`: unknown future events
            // (`axis_source`, `axis_stop`, `axis_value120`, ...) are ignored.
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Keymap { format, fd, size }` — the keymap the compositor uses for keycodes.
            //
            // The `OwnedFd` is an open file descriptor to a server-side keymap file and is
            // *dropped immediately*: the harness only ever needs the keycodes (or the
            // shortcut text) a test triggers, so the file is never memory-mapped, never
            // stored and never leaked — and with it there is nothing that could keep the
            // descriptor alive past this dispatch.
            wl_keyboard::Event::Keymap { format, fd, size } => {
                drop(fd);
                state.keyboard_events.push(KeyboardEvent::Keymap {
                    format: input::keymap_format(format),
                    size,
                });
            }
            // `Enter { serial, surface, keys }` — the keyboard focused a surface.
            //
            // `keys` is the raw native-endian `u32` array of keycodes logically down; the
            // decoding (`input::decode_keys`) drops a trailing partial word instead of
            // panicking. The serial is the one a client may answer with (see
            // `ClientState::latest_input_serial`).
            wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => {
                state.latest_input_serial = Some(serial);
                state.keyboard_events.push(KeyboardEvent::Enter {
                    surface: surface.id(),
                    keys: input::decode_keys(&keys),
                });
            }
            // `Leave { serial, surface }` — the keyboard left a surface.
            wl_keyboard::Event::Leave { surface, .. } => {
                state.keyboard_events.push(KeyboardEvent::Leave {
                    surface: surface.id(),
                });
            }
            // `Key { serial, time, key, state }` — a key changed state.
            //
            // The state arrives as `WEnum`; an unknown code is skipped rather than recorded
            // as the wrong key state.
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                if let Some(key_state) = input::key_state(key_state) {
                    state.keyboard_events.push(KeyboardEvent::Key {
                        keycode: key,
                        state: key_state,
                    });
                }
            }
            // `Modifiers { serial, mods_depressed, mods_latched, mods_locked, group }`.
            //
            // The four masks are recorded exactly as delivered; the harness never
            // interprets them, so a test compares what the compositor actually sent.
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state
                    .keyboard_events
                    .push(KeyboardEvent::Modifiers(input::ModifiersState {
                        depressed: mods_depressed,
                        latched: mods_latched,
                        locked: mods_locked,
                        group,
                    }));
            }
            // `RepeatInfo { rate, delay }` (since v4).
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state
                    .keyboard_events
                    .push(KeyboardEvent::RepeatInfo { rate, delay });
            }
            // Generated event enums are `#[non_exhaustive]`: unknown future events are
            // ignored.
            _ => {}
        }
    }
}

/// Releases a `wl_pointer` whose capability disappeared and drops the client-side object.
///
/// `wl_pointer.release` is the destructor request, but it only exists since v3: on an older
/// object there is nothing to send, so the proxy is simply dropped (the backend removes the
/// client-side object and the compositor notices the id is gone).
fn release_pointer(pointer: &wl_pointer::WlPointer) {
    if pointer.version() >= 3 {
        pointer.release();
    }
}

/// Releases a `wl_keyboard` whose capability disappeared and drops the client-side object.
///
/// Same version rule as [`release_pointer`]: `wl_keyboard.release` is v3+.
fn release_keyboard(keyboard: &wl_keyboard::WlKeyboard) {
    if keyboard.version() >= 3 {
        keyboard.release();
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `wl_shm_pool` has no events (the generated enum is `#[non_exhaustive]`, so it
        // cannot be matched exhaustively); the event is intentionally ignored.
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        proxy: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `Release`.
        //
        // The compositor is done reading the buffer. Return its bytes to the owning pool's
        // free list (`ShmPool::free`) so repeated frames reuse one allocation instead of
        // growing the pool. `wl_buffer` carries no user data, so the owner is the slot
        // that still holds this object id.
        if let wl_buffer::Event::Release = event {
            let id = proxy.id();
            for slot in state.windows.values() {
                let mut window = lock_window(slot);
                let found = window
                    .pending_buffer
                    .iter()
                    .chain(window.attached_buffer.iter())
                    .find(|buffer| buffer.buffer().id() == id)
                    .map(|buffer| (buffer.offset, buffer.len, Arc::clone(&window.pool)));
                let Some((offset, len, pool)) = found else {
                    continue;
                };
                // A second release of the same buffer (it was re-attached by
                // `commit_pending`) must not return the range again: the pool may have
                // handed it to a newer buffer meanwhile. The buffer itself stays in the
                // slot, so `commit_pending` can still re-attach it.
                if !window.released_buffers.insert(id) {
                    break;
                }
                drop(window);
                lock_pool(&pool).free(offset, len);
                break;
            }
        }
        // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
        // versions fall through and are ignored rather than rejected.
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `wl_compositor` has no events (the generated enum is `#[non_exhaustive]`, so it
        // cannot be matched exhaustively); the event is intentionally ignored.
    }
}

impl Dispatch<wl_surface::WlSurface, WindowSlot> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_surface::WlSurface,
        event: wl_surface::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Enter { output: WlOutput }` / `Leave { output: WlOutput }`.
            //
            // Record which outputs the surface is on (the virtual output always enters
            // immediately) so a failure can tell "the surface was never mapped" from
            // "the surface was mapped but the compositor sent nothing".
            wl_surface::Event::Enter { output } => {
                lock_window(data).outputs.insert(output.id());
            }
            wl_surface::Event::Leave { output } => {
                lock_window(data).outputs.remove(&output.id());
            }
            // `PreferredBufferScale { factor: i32 }` (since v6).
            //
            // The test client always commits unscaled buffers; record the preference so
            // a mismatch is visible in a failure instead of silently blurring pixel
            // assertions.
            wl_surface::Event::PreferredBufferScale { factor } => {
                lock_window(data).preferred_buffer_scale = factor;
            }
            // `PreferredBufferTransform { transform: WEnum<wl_output::Transform> }` (v6).
            //
            // Recorded (and expected to be `Normal`): a transform would invalidate exact
            // pixel comparisons.
            wl_surface::Event::PreferredBufferTransform { transform } => {
                let transform = match transform {
                    WEnum::Value(transform) => Some(transform),
                    WEnum::Unknown(_) => None,
                };
                lock_window(data).preferred_buffer_transform = transform;
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
        // `Ping { serial: u32 }`.
        //
        // The compositor disconnects unresponsive clients, so the test client answers
        // immediately.
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
        // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
        // versions fall through and are ignored rather than rejected.
    }
}

impl Dispatch<xdg_surface::XdgSurface, WindowSlot> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `Configure { serial: u32 }`.
        //
        // Completes the configure sequence: the role event stored by
        // `xdg_toplevel.configure`/`xdg_popup.configure` gets its serial, moves to
        // `last_configure`, leaves `pending_serial` for `ack_configure` and is forwarded to
        // the waiting window (a `SendError` only means the handle was dropped).
        if let xdg_surface::Event::Configure { serial } = event {
            let mut window = lock_window(data);
            if let Some(mut configure) = window.pending_configure.take() {
                configure.serial = serial;
                window.pending_serial = Some(serial);
                window.last_configure = Some(configure.clone());
                let _ = window.configure_tx.send(configure);
            }
        }
        // Generated event enums are `#[non_exhaustive]`: events added by newer protocol
        // versions fall through and are ignored rather than rejected.
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, WindowSlot> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Configure { width: i32, height: i32, states: Vec<u8> }` (`states` is an
            // array of native-endian `u32` state codes; `width`/`height` of `0` means
            // "client chooses").
            //
            // The serial arrives with the following `xdg_surface.configure`, so it stays
            // `0` here; `size` only follows a configure that actually picked a size.
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let mut window = lock_window(data);
                let configure = ConfiguredSize {
                    width: width.max(0) as u32,
                    height: height.max(0) as u32,
                    states,
                    serial: 0,
                };
                if configure.width > 0 && configure.height > 0 {
                    window.size = configure.size();
                }
                window.pending_configure = Some(configure);
            }
            // `Close`.
            //
            // The client keeps the surface alive so a test can observe the request before
            // `destroy`.
            xdg_toplevel::Event::Close => lock_window(data).close_requested = true,
            // `ConfigureBounds { width: i32, height: i32 }` (v4+).
            //
            // Recorded; they do not change the surface size.
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                lock_window(data).configure_bounds =
                    Some(Size::new(width.max(0) as u32, height.max(0) as u32));
            }
            // `WmCapabilities { capabilities: Vec<u8> }` (v5+).
            //
            // Recorded so a test can skip capabilities the runtime does not implement.
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                lock_window(data).wm_capabilities = capabilities;
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
        _event: xdg_positioner::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `xdg_positioner` has no events (the generated enum is `#[non_exhaustive]`, so it
        // cannot be matched exhaustively); the event is intentionally ignored.
    }
}

impl Dispatch<xdg_popup::XdgPopup, WindowSlot> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        data: &WindowSlot,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Configure { x: i32, y: i32, width: i32, height: i32 }` (the position is
            // relative to the parent's window geometry; there is no `states` array).
            //
            // The placed offset goes to `popup_offset`; the size is part of the pending
            // configure, which the following `xdg_surface.configure` completes.
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                let mut window = lock_window(data);
                window.popup_offset = Point::new(x, y);
                window.pending_configure = Some(ConfiguredSize {
                    width: width.max(0) as u32,
                    height: height.max(0) as u32,
                    states: Vec::new(),
                    serial: 0,
                });
            }
            // `PopupDone`.
            //
            // The popup is dismissed and must be destroyed by the client.
            xdg_popup::Event::PopupDone => lock_window(data).close_requested = true,
            // `Repositioned { token: u32 }` (v3+).
            //
            // Recorded so a reposition wait can match the token; a configure follows
            // immediately.
            xdg_popup::Event::Repositioned { token } => {
                lock_window(data).popup_reposition_token = Some(token);
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
        // `Done { callback_data: u32 }` (the object is a destructor).
        //
        // Bumps the sync watermark so a test can tell which server roundtrip a `Done`
        // belongs to. `roundtrip` deliberately waits for the next reader cycle instead of
        // this watermark (see its method docs).
        if let wl_callback::Event::Done { .. } = event {
            state.sync_watermark = state.sync_watermark.wrapping_add(1);
        }
        // Generated event enums are `#[non_exhaustive]`; unknown future events fall
        // through and are ignored.
    }
}
