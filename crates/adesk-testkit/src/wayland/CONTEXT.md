# wayland — the Wayland test client
## Intent
A `wayland-client`-based client that drives the compositor over the *real* protocol path: SHM buffers with known fills, xdg-shell toplevels and popups, a bound `wl_seat` whose pointer/keyboard events are recorded, clipboard selection helpers over `wl_data_device_manager`, and bounded event pumps.
It must never reach into compositor internals.
## API Surface
- `WaylandTestClient` — `connect` / `connect_in`, `display_name`, `globals`, `is_closed`, `create_toplevel`, `create_popup`, `pump_for`, `pump_until`, `roundtrip`, `flush`, `close`.
- Seat-input recording on `WaylandTestClient` — `pointer_events()`, `keyboard_events()`, `clear_input_events()`, `wait_for_pointer_event(timeout, what, pred)`, `wait_for_keyboard_event(...)`, `wait_for_pointer_button(button, state, timeout)`, `wait_for_key(keycode, state, timeout)`, `last_modifiers()`; all take `&self` and every wait is deadline-bounded.
- Clipboard on `WaylandTestClient` (module `clipboard`) — `set_selection(mime, bytes)`, `clear_selection()`, `read_selection(mime)`, `read_selection_with_timeout(mime, timeout)`, `selection_offer_count()`, `wait_for_selection_offer(count, timeout)`, plus `pub const DEFAULT_SELECTION_TIMEOUT` (10 s).
- Recorded event types (module `input`, re-exported here): `PointerEvent` (`Enter`/`Leave`/`Motion`/`Button`/`Axis`/`Frame`), `KeyboardEvent` (`Keymap`/`Enter`/`Leave`/`Key`/`Modifiers`/`RepeatInfo`), `AxisKind`, `ButtonState`, `KeyState`, `ModifiersState`, and the evdev constants `BTN_LEFT`, `KEY_LEFTCTRL`, `KEY_C`.
- `ToplevelSpec`, `TestWindow`, `ConfiguredSize`, `PopupSpec`, `TestPopup`, `PumpStats`, `Globals` (with `compositor_version`/`shm_version`/`xdg_wm_base_version`/`seat_version`/`data_device_manager_version`/`supports_argb8888`).
- Reachable as `adesk_testkit::wayland::…`; the crate root re-exports the window/globals subset (`ConfiguredSize`, `PopupSpec`, `PumpStats`, `TestPopup`, `TestWindow`, `ToplevelSpec`, `WaylandTestClient`) plus the input types (`AxisKind`, `ButtonState`, `KeyboardEvent`, `KeyState`, `ModifiersState`, `PointerEvent`, `BTN_LEFT`, `KEY_LEFTCTRL`, `KEY_C`) and `DEFAULT_SELECTION_TIMEOUT`.
## Constraints
- A reader thread owns the `EventQueue` and dispatches into `Arc<Mutex<ClientState>>`; windows hold a `QueueHandle` plus their per-window `Arc<Mutex<WindowState>>`, so every method takes `&self`.
- `connect_in` connects an absolute socket path (env-independent, parallel-test safe); `connect` resolves `$XDG_RUNTIME_DIR`.
- Every pump/wait takes a deadline; teardown is `UnixStream::shutdown` plus a bounded join — never an unbounded block.
- `#![forbid(unsafe_code)]`: SHM is `tempfile`-backed and written via `FileExt::write_all_at`; clipboard pipes use `rustix::pipe::pipe()` (no libc, no `std::io::pipe` — MSRV 1.80); Argb8888 little-endian bytes are `[b, g, r, 255]`.
- Only opaque fills (`FillPattern::require_opaque`); `connect_in` fails with `Unsupported` when the compositor does not advertise ARGB8888.
- Recording rules (see `input.rs` module docs): delivery order and uninterpreted values only, unknown `WEnum` codes skipped rather than guessed, `wl_keyboard.keymap`'s fd dropped immediately, and only an event's own serial recorded.
- Clipboard rules (see `clipboard.rs` module docs): `wl_data_source.send` writes the stored bytes synchronously on the reader thread (test payloads must stay ≤ 64 KiB or the reader can stall on a full pipe); the read-to-EOF of a receive pipe runs on a short-lived worker thread awaited with `recv_timeout` (detached if the peer never closes — bounded by process exit); `set_selection`/`clear_selection` need a real input serial and wait for one, bounded.
- The recorded event types, the clipboard methods and their semantics are a pinned contract consumed by integration tests; extend them, never rename or weaken them.
## Routing Table
| Area | Owner |
|---|---|
| Client lifecycle, pumps, reader thread, public input accessors | `mod.rs` |
| Shared state (`ClientState`/`WindowState`), input `Dispatch` impls, serial plumbing | `state.rs` |
| Clipboard helpers, clipboard `Dispatch` impls, fd/pipe plumbing | `clipboard.rs` |
| Recorded input events, decoders, capability bits | `input.rs` |
| SHM pool and buffers | `shm.rs` |
| Window/popup objects and configure state | `window.rs` |
| Global binding, version negotiation, seat + data-device-manager handles | `protocol.rs` |
## Design Decisions
- **The seat is bound, not assumed.** `protocol.rs` binds `wl_seat` v1+ (`min(server, 11)`) and `wl_data_device_manager` v1+ (`min(server, 4)`) alongside the other required globals; a client-side seat clone lives in `ClientState` because `wl_seat.capabilities` is dispatched on the reader thread and is what creates/destroys `wl_pointer`/`wl_keyboard` (missing global = `Unsupported`, never a panic).
- **Recording lives in the dispatch bodies.** Events are appended to `ClientState::pointer_events`/`keyboard_events` under the lock the reader already holds, so the history is complete even if a test never pumps and `wait_for_*` scans the whole history (an event delivered before the wait call still satisfies it). No dispatch body may panic: unknown enum codes are skipped, a trailing partial `wl_keyboard.enter` keys word is dropped, the keymap fd is closed at once.
- **The latest input serial is plumbing.** `ClientState::latest_input_serial` records `wl_pointer.enter`, `wl_pointer.button` and `wl_keyboard.enter` serials; `clear_input_events` keeps it, and the clipboard serial wait (`wait_for_input_serial`) is its only consumer.
- **Clipboard offers are tracked by object id.** `ClientState::offers` maps every `wl_data_offer` to its advertised mimes and `current_offer` names the one `selection` announced last; `selection_offer_count` counts only `Some` selections. A superseded or cleared offer leaves the map with exactly one `destroy()` (a second destructor would be a protocol error); re-announcing the same object is not a supersede.
- **Only the current source is mutable.** Publishing destroys the previous source proxy, and `wl_data_source.cancelled` clears the stored source only on object-id match, so the compositor's late cancel for a superseded source cannot clear its replacement. `send` answers only when source id *and* mime match; anything else closes the fd without writing.
- **`read_selection_with_timeout` is two bounded phases** (offer wait, then bytes-to-EOF), each getting the full `timeout` — worst case `2 * timeout` — and the failing phase is named in `Timeout { what, .. }`.
- **Data-device focus tracks keyboard focus.** `adesk-compositor` calls Smithay's `set_data_device_focus` on every keyboard-focus change, so the focused client is delivered `data_offer`/`selection`, including the echo of a selection it published itself; the clipboard self-tests assert the round trip strictly.
- **Clipboard `Dispatch` impls live in `clipboard.rs`,** not `state.rs`, to keep `state.rs` under the ~1000-line threshold; the two are one feature and their module docs describe the whole flow.
- **`block_until` is imported from the crate root.** `crate::wait` is not reachable from this module, so waits call the `crate::block_until` re-export with closures that re-lock `ClientState`, without touching the pump channel (a concurrent `pump_until`/`roundtrip` cannot lose notifications).
- **`wl_seat::Capability` is a `bitflags` type**, so capability bits come from its `const fn bits()`; the raw bitfield is the ground truth because a seat with pointer *and* keyboard arrives as `WEnum::Unknown(0b11)`.
- **Reader-thread Wayland client.** The calling thread only sends requests; the reader thread owns the `EventQueue` and sends one `PumpEvent` per read/dispatch cycle on a `tokio::sync::mpsc` channel. Teardown uses `UnixStream::shutdown` plus a bounded join.
- **One pixel ground truth.** `FillPattern::at(x, y, size)` is evaluated both by the SHM writer and by `ImageAssert::matches_pattern`, so the client and the assertion cannot disagree.
## Known Issues
- Superseded SHM buffer ranges are never returned to the pool: `commit_buffer` (`window.rs`) overwrites `WindowState::attached_buffer` without moving the old buffer to `pending_buffer`, and `pending_buffer` is never assigned anywhere, so the `wl_buffer.release` arm in `state.rs` finds neither slot for the superseded buffer id and skips it (`continue`).
- The compositor emits `wl_buffer.release` for the superseded buffer while dispatching the superseding commit (Smithay `RendererSurfaceState::update_buffer` drop), i.e. always after this client overwrote `attached_buffer`, so the release can never be attributed and the free list is never fed by `commit_frame`.
- Consequence: the 16 MiB pool grows by one frame per `commit_frame` and fails with `SHM pool exhausted` after ~3 fresh frames per client at the 1280x800 default, contradicting the "a commit loop reuses one allocation" claim in `mod.rs:115-119` / `state.rs:649-654`; in-tree tests commit at most two fresh frames per client, so the gap is latent.
- `TestWindow::destroy`/`TestPopup::destroy` deregister the window slot before the compositor's release for the still-attached buffer arrives, so that buffer's range is not reclaimed either (same skip arm in `state.rs`).

## Notes for Agents
- `mod.rs` documents the reader-thread, lock-order (`ClientState` → `WindowState`) and teardown rules; `state.rs` documents configure sequencing and clipboard state; `input.rs` documents the recording contract; `clipboard.rs` documents serial handling, offer tracking and fd lifetimes; `shm.rs` documents the Argb8888 byte order.
- Largest files: `state.rs` (~930), `mod.rs` (~870), `clipboard.rs` (~855) — all under the concern threshold but close; put new dispatch impls in their feature module instead of growing `state.rs` further.
- Server-created objects (`wl_data_device.data_offer` → `wl_data_offer`) need the `event_created_child!` declaration next to the parent's `Dispatch` impl, or wayland-client panics while dispatching.
- `state.rs`'s `ClientState`/`WindowSlot` are `pub(crate)` but the modules are private to `wayland`, so a *descendant* module (e.g. `input::tests`) may still reach them; public tests must go through the documented `WaylandTestClient` API.
- Test-side windows are per-runtime ids and popups never appear in `list_windows`; see the crate-level CONTEXT.md for details.
