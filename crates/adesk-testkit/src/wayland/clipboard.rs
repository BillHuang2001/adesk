//! Clipboard helpers: publish a selection, observe offers, read the selection back.
//!
//! The clipboard runs on three protocol objects, all of them owned by the client:
//!
//! - `wl_data_source` — the publisher. [`WaylandTestClient::set_selection`] creates one from
//!   the bound `wl_data_device_manager`, offers exactly one mime type, hands it to
//!   `wl_data_device.set_selection` and keeps it alive in `ClientState::selection_source`
//!   together with the bytes it will answer with.
//! - `wl_data_device` — the seat's view of the clipboard, created once in `connect_in`.
//!   `data_offer` creates an offer object, `selection` names the offer that is the current
//!   selection. Both are recorded by the dispatch impls in [`super::state`].
//! - `wl_data_offer` — the reader. [`WaylandTestClient::read_selection_with_timeout`] waits
//!   for an offer, asks for a mime type with `wl_data_offer.receive` and reads the pipe to
//!   EOF.
//!
//! ## Serial handling
//!
//! `set_selection` quotes a serial because the protocol demands one the client may legally
//! answer with; the compositor additionally requires the *client* to hold keyboard focus
//! (Smithay drops the request silently otherwise — no protocol error, no `cancelled`). The
//! serial comes from `ClientState::latest_input_serial`, i.e. a real `wl_pointer.enter`,
//! `wl_pointer.button` or `wl_keyboard.enter` event the client received; publishing therefore
//! needs a mapped, focused window in front of it. When no such event has arrived yet the call
//! waits for one, bounded by [`DEFAULT_SELECTION_TIMEOUT`], and fails with `Timeout { what:
//! "set_selection input serial" }` if none does (a client that never mapped a window never
//! receives one).
//!
//! ## Offer tracking and supersede semantics
//!
//! `ClientState::offers` maps every `wl_data_offer` object id to the mime types it advertised
//! (`wl_data_offer.offer` events land there, keyed by object id) and
//! `ClientState::current_offer` names the one `wl_data_device.selection` announced last.
//! `selection_offer_count` counts only `Some` selections: a `selection(None)` clears the
//! current offer without counting.
//!
//! An offer that stops being the selection is *finished*: it leaves the map and its proxy is
//! destroyed exactly once (a second `wl_data_offer.destroy` for a dead object would be a
//! protocol error). Re-announcing the same object — the compositor re-broadcasting the
//! selection after a focus change — is not a supersede and does not destroy it. The same
//! "only the current one" rule protects the source side: `wl_data_source.cancelled` clears
//! `ClientState::selection_source` only when it arrives for the source stored there, because
//! the compositor cancels a superseded source *after* its replacement was published.
//! Publishing a new selection destroys the previous source proxy (if it is still live), which
//! is what lets the compositor's late `cancelled` for it be swallowed by wayland-client
//! instead of being dispatched into state that no longer exists.
//!
//! ## File descriptors
//!
//! Two descriptors are involved in a transfer, and neither crosses a thread boundary
//! unboundedly:
//!
//! - **Source side.** A `wl_data_source.send` carries an `OwnedFd`. The reader thread (the
//!   only thread that dispatches) writes the stored bytes with `write_all` and drops the
//!   descriptor, closing the write end so the reader reaches EOF. Because that write blocks
//!   the reader thread when the pipe is full (a 64 KiB Linux pipe buffer), **test payloads
//!   must stay ≤ 64 KiB**; the harness is for clipboard smoke tests, not bulk data.
//! - **Reader side.** `wl_data_offer.receive` gets one end of a `rustix::pipe::pipe()` pair.
//!   The client's own write end is dropped immediately after the request (so EOF can only
//!   come from the source closing *its* copy) and the read end is handed to a short-lived
//!   worker thread that owns it as a `std::fs::File` and reads to EOF. The calling thread
//!   waits with `std::sync::mpsc::Receiver::recv_timeout`, so the read is never unbounded.
//!   On the timeout path the worker is detached while blocked in `read`: it holds the read
//!   end until the peer closes or the process exits. That is the documented leak caveat — the
//!   alternative (a cancellable read) needs `unsafe` or a new dependency.
//!
//! ## Bounded phases
//!
//! [`read_selection_with_timeout`](WaylandTestClient::read_selection_with_timeout) is two
//! bounded phases, each getting the full `timeout`: (1) wait for a selection offer to exist,
//! and (2) read the bytes to EOF. Worst case is therefore `2 * timeout`, and the failing
//! phase is named in the error (`"read_selection selection offer"` vs `"read_selection
//! bytes"`). [`DEFAULT_SELECTION_TIMEOUT`] is what [`read_selection`] and the serial wait in
//! `set_selection` use.
//!
//! ## Compositor caveat (empirically verified)
//!
//! `wl_data_device` only receives `data_offer`/`selection` while the compositor has a
//! *data-device focus* for that client. `adesk-compositor` does not call Smithay's
//! `set_data_device_focus` yet, so today no client ever receives a selection offer — not even
//! the client that published it (there is no same-client echo). The API here is
//! contract-correct regardless: `set_selection`/`clear_selection` publish, and the read path
//! waits and reports `Timeout { what: "read_selection selection offer" }` instead of inventing
//! data. The self-tests at the bottom of this file observe which of the two behaviours is live
//! and assert accordingly; the full read round-trip assertion must be added to the "offers
//! arrive" branch when the compositor wires data-device focus.

use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};

use crate::block_until;
use crate::error::{Result, TestkitError};

use super::map_wayland_error;
use super::state::{lock_client, ClientState, OfferRecord, SelectionSource};
use super::WaylandTestClient;

/// Default deadline for the clipboard helpers ([`read_selection`] and the serial wait inside
/// [`set_selection`]).
///
/// Same order of magnitude as the harness's other "the runtime should have answered by now"
/// deadlines: long enough that a loaded CI machine does not fail spuriously, short enough that
/// a broken compositor fails a test instead of hanging it.
///
/// [`read_selection`]: WaylandTestClient::read_selection
/// [`set_selection`]: WaylandTestClient::set_selection
pub const DEFAULT_SELECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// `what` of the `set_selection`/`clear_selection` serial wait.
const WHAT_INPUT_SERIAL: &str = "set_selection input serial";
/// `what` of the "the client holds a selection offer" phase of a read.
const WHAT_SELECTION_OFFER: &str = "read_selection selection offer";
/// `what` of the "the pipe reached EOF" phase of a read.
const WHAT_SELECTION_BYTES: &str = "read_selection bytes";
/// `what` of [`WaylandTestClient::wait_for_selection_offer`].
const WHAT_OFFER_COUNT: &str = "selection offer count";

impl WaylandTestClient {
    /// Publishes `bytes` as the clipboard selection under `mime`.
    ///
    /// Steps: wait (bounded by [`DEFAULT_SELECTION_TIMEOUT`]) for an input serial to answer
    /// with, `create_data_source`, `offer(mime)`, store the source with its mime and bytes (so
    /// a later `wl_data_source.send` can be answered on the reader thread), destroy the
    /// previously published source proxy if there was one, then
    /// `wl_data_device.set_selection(Some(source), serial)` and flush the requests.
    ///
    /// Only one source is remembered: publishing replaces the previous selection, exactly like
    /// a real clipboard. Whether the compositor *accepts* the selection is not reportable — it
    /// drops the request silently when this client does not hold keyboard focus — so `Ok(())`
    /// means "the requests are on the wire", and the observation side
    /// ([`selection_offer_count`](WaylandTestClient::selection_offer_count)) is what proves
    /// what the compositor did with it.
    ///
    /// Fails with `Timeout { what: "set_selection input serial" }` when no input event carried
    /// a serial before the deadline, and with the connection/flush error when the runtime is
    /// gone.
    pub fn set_selection(&self, mime: &str, bytes: impl Into<Vec<u8>>) -> Result<()> {
        let serial = self.wait_for_input_serial(DEFAULT_SELECTION_TIMEOUT)?;
        let source = self
            .globals
            .data_device_manager()
            .create_data_source(&self.qhandle, ());
        source.offer(mime.to_string());
        let replaced = lock_client(&self.state)
            .selection_source
            .replace(SelectionSource {
                source: source.clone(),
                mime: mime.to_string(),
                bytes: bytes.into(),
            });
        // The compositor cancels a superseded source, but only *after* the replacement is
        // published; destroying the old proxy here keeps the client's object table clean and
        // makes that late `cancelled` a swallowed event rather than a dispatch into state that
        // no longer exists.
        destroy_source(replaced);
        lock_client(&self.state)
            .data_device
            .set_selection(Some(&source), serial);
        self.conn.flush().map_err(|err| map_wayland_error(&err))?;
        Ok(())
    }

    /// Clears the clipboard selection (`wl_data_device.set_selection(None, serial)`).
    ///
    /// Uses the same bounded serial wait as
    /// [`set_selection`](WaylandTestClient::set_selection) and drops the stored source, so a
    /// later `wl_data_source.send` for it is closed without writing. `Ok(())` means the request
    /// is on the wire, with the same "did the compositor accept it" caveat as `set_selection`.
    pub fn clear_selection(&self) -> Result<()> {
        let serial = self.wait_for_input_serial(DEFAULT_SELECTION_TIMEOUT)?;
        let cleared = lock_client(&self.state).selection_source.take();
        destroy_source(cleared);
        lock_client(&self.state)
            .data_device
            .set_selection(None, serial);
        self.conn.flush().map_err(|err| map_wayland_error(&err))?;
        Ok(())
    }

    /// Reads the current selection for `mime` with [`DEFAULT_SELECTION_TIMEOUT`].
    ///
    /// See [`read_selection_with_timeout`](WaylandTestClient::read_selection_with_timeout).
    pub fn read_selection(&self, mime: &str) -> Result<Option<Vec<u8>>> {
        self.read_selection_with_timeout(mime, DEFAULT_SELECTION_TIMEOUT)
    }

    /// Reads the current selection for `mime`, bounded by `timeout` per phase.
    ///
    /// Phase 1 waits (bounded by `timeout`) until the client holds a selection offer, i.e.
    /// until a `wl_data_device.selection` announced one; without one the call fails with
    /// `Timeout { what: "read_selection selection offer" }` (the compositor only sends offers to
    /// a client it has data-device focus for — see the module docs).
    ///
    /// Phase 2 then asks that offer for `mime`: if the offer does not advertise `mime` the call
    /// returns `Ok(None)` immediately (the selection exists, it just cannot be read as this mime
    /// type), otherwise `rustix::pipe::pipe()` + `wl_data_offer.receive`, then the read end is
    /// read to EOF with a second `timeout` budget and the exact bytes returned. Not reaching EOF
    /// in time fails with `Timeout { what: "read_selection bytes" }`; an I/O error while reading
    /// is reported as [`TestkitError::Io`]. The response to `receive` is always EOF-terminated,
    /// because the source closes its write end after writing (see the module docs on file
    /// descriptors).
    pub fn read_selection_with_timeout(
        &self,
        mime: &str,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>> {
        block_until(timeout, WHAT_SELECTION_OFFER, || {
            lock_client(&self.state).current_offer.is_some()
        })?;
        let Some(offer) = self.current_offer_for(mime, timeout)? else {
            return Ok(None);
        };
        let (read_fd, write_fd) =
            rustix::pipe::pipe().map_err(|err| TestkitError::Io(err.into()))?;
        offer.receive(mime.to_string(), write_fd.as_fd());
        // Closing the client's own write end is what makes EOF mean "the source is done": the
        // compositor keeps its own copy of the descriptor, so the read end cannot see EOF while
        // this one is open.
        drop(write_fd);
        // `receive` is buffered until a flush; the source only sees the request after this.
        self.conn.flush().map_err(|err| map_wayland_error(&err))?;
        read_selection_bytes(read_fd, timeout).map(Some)
    }

    /// How many `wl_data_device.selection` events carried an offer so far.
    ///
    /// Monotonic counter (wrapping at `u64::MAX`), incremented by the dispatch impl, so a test
    /// can assert "no offer arrived" by comparing before and after instead of racing a deadline.
    /// `selection(None)` — the selection was cleared — deliberately does not count.
    pub fn selection_offer_count(&self) -> u64 {
        lock_client(&self.state).selection_offer_count
    }

    /// Waits (bounded) until at least `count` selection offers were received.
    ///
    /// Satisfied immediately by an already-reached count, so `wait_for_selection_offer(0, _)` is
    /// a cheap "no wait" assertion. Expiry is `Timeout { what: "selection offer count" }`.
    pub fn wait_for_selection_offer(&self, count: u64, timeout: Duration) -> Result<()> {
        block_until(timeout, WHAT_OFFER_COUNT, || {
            lock_client(&self.state).selection_offer_count >= count
        })
    }

    /// The proxy of the current offer when it advertises `mime`.
    ///
    /// `Ok(None)` means "a selection offer exists but not this mime type", which is *not* an
    /// error ([`read_selection_with_timeout`](WaylandTestClient::read_selection_with_timeout)
    /// reports it as `Ok(None)` too). The `timeout` only names the impossible race of the
    /// selection being cleared between the offer wait and this read: the caller's contract is
    /// "no offer → timeout", so that is what it reports.
    fn current_offer_for(
        &self,
        mime: &str,
        timeout: Duration,
    ) -> Result<Option<wl_data_offer::WlDataOffer>> {
        let state = lock_client(&self.state);
        let missing = || TestkitError::Timeout {
            what: WHAT_SELECTION_OFFER,
            timeout,
        };
        let current = state.current_offer.clone().ok_or_else(missing)?;
        let record = state.offers.get(&current).ok_or_else(missing)?;
        if !record
            .mime_types
            .iter()
            .any(|advertised| advertised == mime)
        {
            return Ok(None);
        }
        Ok(Some(record.offer.clone()))
    }

    /// Waits (bounded) until a real input event carried a serial, then returns it.
    ///
    /// Internal plumbing for `set_selection`/`clear_selection`: the protocol demands a serial
    /// the client may answer with (see the module docs), so this is the one place that turns
    /// "no input yet" into `Timeout { what: "set_selection input serial" }`.
    fn wait_for_input_serial(&self, timeout: Duration) -> Result<u32> {
        block_until(timeout, WHAT_INPUT_SERIAL, || {
            lock_client(&self.state).latest_input_serial().is_some()
        })?;
        // A serial is never removed once set, so this is a safety net for the connection dying
        // inside the window above rather than a normal path.
        lock_client(&self.state)
            .latest_input_serial()
            .ok_or(TestkitError::Timeout {
                what: WHAT_INPUT_SERIAL,
                timeout,
            })
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for ClientState {
    fn event(
        _state: &mut ClientState,
        _proxy: &wl_data_device_manager::WlDataDeviceManager,
        _event: wl_data_device_manager::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `wl_data_device_manager` has no events (the generated enum is `#[non_exhaustive]`,
        // so it cannot be matched exhaustively); the event is intentionally ignored.
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        _proxy: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `DataOffer { id: WlDataOffer }` — the compositor created an offer object.
            //
            // The offer is registered before the `wl_data_offer.offer` events that follow it
            // (that is where its mime types land) and before the `selection` event that
            // makes it the current selection.
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(
                    id.id(),
                    OfferRecord {
                        offer: id,
                        mime_types: Vec::new(),
                    },
                );
            }
            // `Selection { id: Option<WlDataOffer> }` — the clipboard selection changed.
            //
            // A new offer supersedes the previous one: the old object is finished as far as
            // the compositor is concerned (`wl_data_device.selection` is the authoritative
            // list, and the client owns destroying what it no longer uses), so it leaves the
            // map and its proxy is destroyed. Re-announcing the *same* object (the compositor
            // re-broadcasting the selection) does not destroy it.
            wl_data_device::Event::Selection { id: Some(offer) } => {
                let id = offer.id();
                if let Some(previous) = state.current_offer.take() {
                    if previous != id {
                        destroy_offer(state, &previous);
                    }
                }
                // The object normally exists already (its `data_offer` event came first);
                // registering an unknown one keeps `receive` possible instead of dropping the
                // only proxy the client has.
                if !state.offers.contains_key(&id) {
                    state.offers.insert(
                        id.clone(),
                        OfferRecord {
                            offer,
                            mime_types: Vec::new(),
                        },
                    );
                }
                state.current_offer = Some(id);
                state.selection_offer_count = state.selection_offer_count.wrapping_add(1);
            }
            // `Selection { id: None }` — the selection was cleared.
            //
            // No offer is announced, so the counter deliberately stays put; the previous
            // offer is finished and destroyed like a superseded one.
            wl_data_device::Event::Selection { id: None } => {
                if let Some(previous) = state.current_offer.take() {
                    destroy_offer(state, &previous);
                }
            }
            // `Enter` / `Leave` / `Motion` / `Drop` drive drag-and-drop, which the harness
            // does not exercise (a drag needs an implicit pointer grab and a target surface):
            // the offer such an event may carry is intentionally ignored, so a stray drag
            // offer can never masquerade as a clipboard selection.
            _ => {}
        }
    }

    // `data_offer` is the one event that creates an object; the reader thread's
    // `event_created_child` hook must know its user data, or wayland-client panics while
    // dispatching. The child is created on this same queue with the client's unit user data.
    event_created_child!(ClientState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        proxy: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        // `Offer { mime_type: String }` — one mime type the offer advertises.
        //
        // Recorded on the object its `data_offer` event registered. An offer the client
        // never registered (a drag offer) is ignored rather than invented. Every other
        // event (`SourceActions` / `Action`, v3 drag-and-drop negotiation) is ignored: the
        // harness never starts a drag, so none of them is clipboard state.
        if let wl_data_offer::Event::Offer { mime_type } = event {
            if let Some(record) = state.offers.get_mut(&proxy.id()) {
                record.mime_types.push(mime_type);
            }
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for ClientState {
    fn event(
        state: &mut ClientState,
        proxy: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<ClientState>,
    ) {
        match event {
            // `Send { mime_type: String, fd: OwnedFd }` — the compositor forwarded a reader's
            // request for the data.
            //
            // The stored bytes are written to the descriptor and the descriptor is closed
            // (dropped) so the reader sees EOF. This runs on the reader thread, hence the
            // bounded-payload rule documented in `super::clipboard`. A request for a mime
            // this source does not offer (or one for a superseded source) closes the
            // descriptor without writing, which is the only answer the protocol allows.
            wl_data_source::Event::Send { mime_type, fd } => {
                let matches_current_source =
                    state.selection_source.as_ref().is_some_and(|source| {
                        source.source.id() == proxy.id() && source.mime == mime_type
                    });
                if matches_current_source {
                    let source = state
                        .selection_source
                        .as_ref()
                        .expect("checked immediately above");
                    write_selection_bytes(&source.bytes, fd);
                } else {
                    drop(fd);
                }
            }
            // `Cancelled` — the source is no longer the selection.
            //
            // Only the *current* source is cleared: the compositor cancels a superseded
            // source after the replacement has already been published, so a `Cancelled` for
            // an old object must not throw away the newer selection.
            wl_data_source::Event::Cancelled => {
                let is_current = state
                    .selection_source
                    .as_ref()
                    .is_some_and(|source| source.source.id() == proxy.id());
                if is_current {
                    state.selection_source = None;
                }
            }
            // `Target` / `DndDropPerformed` / `DndFinished` / `Action` belong to
            // drag-and-drop, which the harness does not exercise; they are ignored rather
            // than folded into clipboard state.
            _ => {}
        }
    }
}

/// Drops an offer from the client's map and destroys its proxy.
///
/// The map is the single owner of a live offer: an offer reaches this path exactly once
/// (either superseded by a new selection or cleared by `selection(None)`), so no
/// `wl_data_offer.destroy` is ever sent twice — a second destructor request for a dead
/// object is a protocol error, not a harmless duplicate.
fn destroy_offer(state: &mut ClientState, id: &ObjectId) {
    if let Some(record) = state.offers.remove(id) {
        if record.offer.is_alive() {
            record.offer.destroy();
        }
    }
}

/// Writes `bytes` to the descriptor a `wl_data_source.send` carried and closes it.
///
/// The write happens on the reader thread, so the payload bound documented in
/// `super::clipboard` applies. A write error means the reader closed its end early: there is
/// no caller to return it to and no protocol error to raise, so it is logged (never
/// silently dropped) and the descriptor is closed — the reader's own read then fails
/// visibly on its side.
fn write_selection_bytes(bytes: &[u8], fd: OwnedFd) {
    let mut file = std::fs::File::from(fd);
    if let Err(error) = file.write_all(bytes) {
        tracing::debug!(%error, len = bytes.len(), "writing the selection payload failed");
    }
    // `file` is dropped here, closing the write end so the reader reaches EOF.
}

/// Destroys a source proxy the client no longer publishes.
///
/// A source that is already dead client-side (its `destroy` was sent) is left alone: sending a
/// destructor request twice would be a protocol error.
fn destroy_source(source: Option<SelectionSource>) {
    if let Some(source) = source {
        if source.source.is_alive() {
            source.source.destroy();
        }
    }
}

/// Reads a `wl_data_offer.receive` pipe to EOF with a deadline.
///
/// The read end is owned by a short-lived worker thread (`OwnedFd` → `std::fs::File`) and the
/// caller waits on an `mpsc` channel, because the read must not run on the reader thread and
/// must not be unbounded. On the timeout path the worker stays detached and blocked (its read
/// end closes with the peer or with the process) — that is the documented leak caveat of this
/// module; it is bounded by process lifetime and never blocks the caller.
fn read_selection_bytes(read_fd: OwnedFd, timeout: Duration) -> Result<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("adesk-testkit-clipboard-read".to_string())
        .spawn(move || {
            let mut file = std::fs::File::from(read_fd);
            let mut bytes = Vec::new();
            let outcome = file.read_to_end(&mut bytes).map(|_read| bytes);
            // A send failure only means the caller stopped waiting (it timed out).
            let _ = tx.send(outcome);
        })
        .map_err(TestkitError::Io)?;
    // Detached on purpose: the timeout path must not join a thread stuck in `read`.
    drop(worker);
    match rx.recv_timeout(timeout) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(TestkitError::Io(error)),
        Err(RecvTimeoutError::Timeout) => Err(TestkitError::Timeout {
            what: WHAT_SELECTION_BYTES,
            timeout,
        }),
        Err(RecvTimeoutError::Disconnected) => Err(TestkitError::Wayland(
            "the clipboard reader thread died before reporting the selection bytes".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Instant;

    use adesk_core::{AppId, Position, WindowId};

    use super::*;
    use crate::runtime::{TestRuntime, TestRuntimeConfig};
    use crate::wayland::{PointerEvent, TestWindow, ToplevelSpec};
    use crate::{FillPattern, Size, TestkitError};

    /// Bounded wait for the self-validation tests (far above the cost of one injection).
    const DEADLINE: Duration = Duration::from_secs(10);

    /// How long the tests wait to find out whether a selection offer arrives at all.
    ///
    /// Orders of magnitude more than the compositor needs to answer a request it accepts, and
    /// short enough that the "no offer" case does not slow the suite down.
    const OFFER_OBSERVATION_WINDOW: Duration = Duration::from_millis(500);

    /// A short deadline for the paths that must fail, so a broken implementation fails fast.
    const SHORT: Duration = Duration::from_millis(50);

    /// App id of the toplevel the self-test drives over the protocol path.
    const APP_ID: &str = "org.example.testkit.clipboard";

    /// The bytes a self-test publishes; small enough for any pipe buffer.
    const PAYLOAD: &[u8] = b"adesk clipboard self-test";

    /// Starts a real runtime whose Wayland socket the client can reach.
    ///
    /// `apply_env(false)`: no app is launched, so the runtime releases the process-env lock as
    /// soon as the server is up ([`TestRuntime::wayland_client`] connects by absolute socket
    /// path). The current-thread tokio runtime is returned with it because the AGP stages run
    /// through `block_on` while the client's own helpers stay synchronous.
    fn start_runtime() -> (tokio::runtime::Runtime, TestRuntime) {
        let tokio_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("cannot build the tokio runtime for the clipboard self-test");
        let runtime = tokio_rt
            .block_on(TestRuntime::start_with(
                TestRuntimeConfig::new().with_apply_env(false),
            ))
            .expect("the test runtime starts");
        (tokio_rt, runtime)
    }

    /// Maps a toplevel, acknowledges its configure and commits one frame (it is mapped).
    ///
    /// Returns the window handle together with the runtime's own id for it (read over AGP,
    /// never assumed).
    fn mapped_window(
        tokio_rt: &tokio::runtime::Runtime,
        runtime: &TestRuntime,
        wayland: &mut WaylandTestClient,
    ) -> (TestWindow, WindowId) {
        let window = wayland
            .create_toplevel(ToplevelSpec::new(APP_ID, "Clipboard", Size::new(320, 200)))
            .expect("the toplevel is created");
        window
            .wait_for_configure(DEADLINE)
            .expect("the compositor configures the toplevel");
        window
            .apply_configure()
            .expect("the configure is acknowledged");
        window
            .commit_frame(FillPattern::default())
            .expect("the first frame maps the window");
        tokio_rt
            .block_on(wayland.roundtrip())
            .expect("one roundtrip after mapping");
        let listed = tokio_rt
            .block_on(async {
                let client = runtime.client().await.expect("the AGP client connects");
                client.list_windows().await
            })
            .expect("the AGP client lists the windows");
        let info = listed
            .windows
            .iter()
            .find(|info| info.app_id.as_ref() == Some(&AppId::from(APP_ID)))
            .expect("the mapped toplevel is listed");
        (window, info.id)
    }

    /// Gives the reader thread `window` to dispatch anything the runtime sent.
    ///
    /// `block_until` is the harness's only sleeping primitive (no ad-hoc sleep loops), and the
    /// condition is deliberately unsatisfiable: this is a bounded observation window for
    /// "nothing happened", followed by an assertion on the state it left behind.
    fn observe_for(window: Duration) {
        let elapsed = block_until(window, "the clipboard observation window", || false);
        assert!(elapsed.is_err(), "the observation window always elapses");
    }

    /// Asserts a timeout carrying exactly `what` and `timeout`.
    fn assert_timeout(error: TestkitError, expected_what: &str, expected_timeout: Duration) {
        match error {
            TestkitError::Timeout { what, timeout } => {
                assert_eq!(what, expected_what, "the failing phase must be named");
                assert_eq!(timeout, expected_timeout);
            }
            other => panic!("expected Timeout {{ what: {expected_what} }}, got {other:?}"),
        }
    }

    /// The pipe-reading half of a receive: exact bytes up to EOF, then a bounded failure.
    ///
    /// This is the part of the read path that does not depend on a compositor sending offers, so
    /// it is validated directly against real descriptors: EOF (write end closed) yields the
    /// exact bytes, and a write end that stays open fails with the documented `what` instead of
    /// blocking.
    #[test]
    fn reading_a_receive_pipe_returns_the_bytes_and_times_out_without_eof() {
        let (read_fd, write_fd) = rustix::pipe::pipe().expect("the test can create a pipe");
        let mut writer = std::fs::File::from(write_fd);
        writer
            .write_all(PAYLOAD)
            .expect("the pipe accepts the payload");
        drop(writer);
        let bytes = read_selection_bytes(read_fd, DEADLINE).expect("EOF delivers the bytes");
        assert_eq!(bytes, PAYLOAD);

        // No EOF: the write end is still open, so the read must not be able to finish.
        let (read_fd, _open_write_fd) = rustix::pipe::pipe().expect("the test can create a pipe");
        let started = Instant::now();
        let error = read_selection_bytes(read_fd, SHORT)
            .expect_err("a pipe that never reaches EOF must time out");
        assert!(
            started.elapsed() < DEADLINE,
            "the read must be bounded by its deadline"
        );
        assert_timeout(error, "read_selection bytes", SHORT);
    }

    /// The pinned default is exactly ten seconds.
    #[test]
    fn the_default_selection_timeout_is_ten_seconds() {
        assert_eq!(DEFAULT_SELECTION_TIMEOUT, Duration::from_secs(10));
    }

    /// The clipboard device is negotiated as a real global of the runtime.
    #[test]
    fn the_data_device_manager_is_bound_from_the_runtime() {
        let (tokio_rt, runtime) = start_runtime();
        let wayland = runtime.wayland_client().expect("the client connects");
        let version = wayland.globals().data_device_manager_version();
        assert!(
            (1..=4).contains(&version),
            "wl_data_device_manager must be negotiated in v1..=v4, got v{version}"
        );
        assert!(
            wayland.globals().seat_version() >= 1,
            "the clipboard needs a seat to build the data device from"
        );

        tokio_rt
            .block_on(runtime.shutdown())
            .expect("the runtime shuts down");
    }

    /// A client that never mapped a window has no input serial, so publishing cannot start.
    ///
    /// The serial wait is the one clipboard failure a test can provoke deterministically without
    /// a cooperating compositor: `wl_pointer.enter`/`wl_keyboard.enter` require a surface, so
    /// this client never receives one.
    #[test]
    fn set_selection_without_an_input_serial_times_out() {
        let (tokio_rt, runtime) = start_runtime();
        let wayland = runtime.wayland_client().expect("the client connects");
        let error = wayland
            .wait_for_input_serial(SHORT)
            .expect_err("no window is mapped, so no input event carried a serial");
        assert_timeout(error, "set_selection input serial", SHORT);

        tokio_rt
            .block_on(runtime.shutdown())
            .expect("the runtime shuts down");
    }

    /// Capstone self-validation: a real runtime, real clipboard requests, real observations.
    ///
    /// `set_selection`/`clear_selection` are driven over the protocol path against a mapped,
    /// focused toplevel and the *observed* offer behaviour is asserted: `adesk-compositor` does
    /// not set a data-device focus yet, so the expected observation is "no offer ever arrives"
    /// and the read path must name that as a timeout. Should the compositor gain data-device
    /// focus, this test takes the other branch and asserts the same-client round trip
    /// (`read_selection` returning exactly what `set_selection` published).
    #[test]
    fn selection_publish_and_offer_observation() {
        let (tokio_rt, runtime) = start_runtime();
        let mut wayland = runtime.wayland_client().expect("the client connects");
        let (_window, window_id) = mapped_window(&tokio_rt, &runtime, &mut wayland);

        // A real injection gives the client a serial to answer with (and proves the seat path
        // the serial comes from is live).
        let client = tokio_rt.block_on(runtime.client()).expect("AGP connects");
        let tiled = runtime.tiled_rect();
        tokio_rt
            .block_on(client.pointer_move(
                window_id,
                Position::pixels((tiled.w / 2) as i32, (tiled.h / 2) as i32),
            ))
            .expect("the pointer move is injected");
        wayland
            .wait_for_pointer_event(DEADLINE, "pointer enter on the mapped surface", |event| {
                matches!(event, PointerEvent::Enter { .. })
            })
            .expect("the seat delivered an entering pointer event");
        wayland
            .wait_for_input_serial(DEADLINE)
            .expect("an injected input event carried a serial");

        // Nothing is selected yet: the counter starts at zero and a zero count needs no wait.
        assert_eq!(wayland.selection_offer_count(), 0);
        wayland
            .wait_for_selection_offer(0, SHORT)
            .expect("a count of zero is satisfied without waiting");

        // Publish, and check the client-side bookkeeping the read path depends on.
        wayland
            .set_selection("text/plain", PAYLOAD)
            .expect("the selection requests are on the wire");
        {
            let state = lock_client(&wayland.state);
            let stored = state
                .selection_source
                .as_ref()
                .expect("the published source is kept alive for wl_data_source.send");
            assert_eq!(stored.mime, "text/plain");
            assert_eq!(stored.bytes, PAYLOAD);
            assert!(stored.source.is_alive(), "the source proxy is still live");
        }

        // The empirical question: does the compositor announce a selection offer?
        let announced = block_until(OFFER_OBSERVATION_WINDOW, "a selection offer", || {
            wayland.selection_offer_count() >= 1
        });
        if announced.is_ok() {
            // The compositor wired data-device focus: the same-client echo must round trip.
            let read = wayland
                .read_selection("text/plain")
                .expect("the offer can be read back");
            assert_eq!(
                read.as_deref(),
                Some(PAYLOAD),
                "reading back the selection must return exactly the bytes that were published"
            );
            let unknown = wayland
                .read_selection("application/x-unknown")
                .expect("an unadvertised mime type is not an error");
            assert_eq!(unknown, None);
        } else {
            // Today's behaviour: no client receives offers, so both observation helpers must
            // report the missing offer instead of inventing data.
            assert_eq!(
                wayland.selection_offer_count(),
                0,
                "no selection offer is announced while the compositor has no data-device focus"
            );
            let error = wayland
                .wait_for_selection_offer(1, SHORT)
                .expect_err("no offer was ever announced");
            assert_timeout(error, "selection offer count", SHORT);
            let error = wayland
                .read_selection_with_timeout("text/plain", SHORT)
                .expect_err("there is no offer to read the selection from");
            assert_timeout(error, "read_selection selection offer", SHORT);
        }

        // Publishing again replaces the selection: exactly one source is stored, and a
        // `wl_data_source.cancelled` for the superseded one (which the compositor sends after
        // accepting the replacement) must not clear the newer record.
        let replacement = b"second payload";
        wayland
            .set_selection("text/plain", &replacement[..])
            .expect("the replacement selection is on the wire");
        observe_for(OFFER_OBSERVATION_WINDOW);
        {
            let state = lock_client(&wayland.state);
            let stored = state
                .selection_source
                .as_ref()
                .expect("the replacement is the stored source, so a late cancelled was ignored");
            assert_eq!(stored.bytes, replacement);
            assert!(state.offers.is_empty(), "no offer was announced");
        }

        // Clearing drops the stored source (a later `wl_data_source.send` is answered with a
        // closed descriptor) and puts `set_selection(None, serial)` on the wire.
        wayland
            .clear_selection()
            .expect("clearing the selection is on the wire");
        assert!(
            lock_client(&wayland.state).selection_source.is_none(),
            "clearing drops the published source"
        );
        assert_eq!(
            wayland.selection_offer_count(),
            0,
            "clearing the selection is not an offer"
        );

        tokio_rt
            .block_on(runtime.shutdown())
            .expect("the runtime shuts down");
    }
}
