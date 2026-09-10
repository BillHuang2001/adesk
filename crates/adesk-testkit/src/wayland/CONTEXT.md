# wayland — the Wayland test client

## Intent

A `wayland-client`-based client that drives the compositor over the *real* protocol path: SHM buffers with known fills, xdg-shell toplevels and popups, a bound `wl_seat` whose pointer/keyboard events are recorded, bounded event pumps.
It must never reach into compositor internals.

## API Surface

- `WaylandTestClient` — `connect` / `connect_in`, `display_name`, `globals`, `is_closed`, `create_toplevel`, `create_popup`, `pump_for`, `pump_until`, `roundtrip`, `flush`, `close`.
- Seat-input recording on `WaylandTestClient` — `pointer_events()`, `keyboard_events()`, `clear_input_events()`, `wait_for_pointer_event(timeout, what, pred)`, `wait_for_keyboard_event(...)`, `wait_for_pointer_button(button, state, timeout)`, `wait_for_key(keycode, state, timeout)`, `last_modifiers()`; all take `&self` and every wait is deadline-bounded.
- Recorded event types (module `input`, re-exported here): `PointerEvent` (`Enter`/`Leave`/`Motion`/`Button`/`Axis`/`Frame`), `KeyboardEvent` (`Keymap`/`Enter`/`Leave`/`Key`/`Modifiers`/`RepeatInfo`), `AxisKind`, `ButtonState`, `KeyState`, `ModifiersState`, and the evdev constants `BTN_LEFT`, `KEY_LEFTCTRL`, `KEY_C`.
- `ToplevelSpec`, `TestWindow`, `ConfiguredSize`, `PopupSpec`, `TestPopup`, `PumpStats`, `Globals` (with `compositor_version`/`shm_version`/`xdg_wm_base_version`/`seat_version`/`supports_argb8888`).
- Reachable as `adesk_testkit::wayland::…`; `lib.rs` re-exports only the window/globals subset (`ConfiguredSize`, `PopupSpec`, `PumpStats`, `TestPopup`, `TestWindow`, `ToplevelSpec`, `WaylandTestClient`).

## Constraints

- A reader thread owns the `EventQueue` and dispatches into `Arc<Mutex<ClientState>>`; windows hold a `QueueHandle` plus their per-window `Arc<Mutex<WindowState>>`, so every method takes `&self`.
- `connect_in` connects an absolute socket path (env-independent, parallel-test safe); `connect` resolves `$XDG_RUNTIME_DIR`.
- Every pump/wait takes a deadline; teardown is `UnixStream::shutdown` plus a bounded join — never an unbounded block.
- `#![forbid(unsafe_code)]`: SHM is `tempfile`-backed and written via `FileExt::write_all_at`; Argb8888 little-endian bytes are `[b, g, r, 255]`.
- Only opaque fills (`FillPattern::require_opaque`); `connect_in` fails with `Unsupported` when the compositor does not advertise ARGB8888.
- Recording rules (see `input.rs` module docs): delivery order and uninterpreted values only, unknown `WEnum` codes skipped rather than guessed, `wl_keyboard.keymap`'s fd dropped immediately, and only an event's own serial recorded.
- The recorded event types, their fields and the client's input methods are a pinned contract consumed by tests; extend them, never rename or weaken them.

## Routing Table

| Area | Owner |
|---|---|
| Client lifecycle, pumps, reader thread, public input accessors | `mod.rs` |
| `Dispatch` impls and shared state | `state.rs` |
| SHM pool and buffers | `shm.rs` |
| Window/popup objects and configure state | `window.rs` |
| Global binding, version negotiation, seat handle | `protocol.rs` |
| Recorded input events, decoders, capability bits | `input.rs` |

## Design Decisions

- **The seat is bound, not assumed.** `protocol.rs` binds `wl_seat` v1+ (`min(server, 11)`) alongside the three required globals and exposes the handle via `Globals::seat()`; a client-side clone lives in `ClientState::seat`, because `wl_seat.capabilities` is dispatched on the reader thread and is what creates/destroys the `wl_pointer`/`wl_keyboard` objects (`wl_seat` missing = `Unsupported`, seat below v1 = `Unsupported`, never a panic).
- **Recording lives in the dispatch bodies.** Events are appended to `ClientState::pointer_events`/`keyboard_events` under the `ClientState` lock the reader already holds, so the history is complete even if a test never pumps and `wait_for_*` scans the whole history (an event delivered before the wait call still satisfies it). Nothing in a dispatch body may panic on malformed input: unknown enum codes are skipped, a trailing partial `wl_keyboard.enter` keys word is dropped, and the keymap fd is closed at once.
- **The latest input serial is plumbing, kept across `clear_input_events`.** `ClientState::latest_input_serial` records `wl_pointer.enter`, `wl_pointer.button` and `wl_keyboard.enter` serials for the later clipboard pass (`wl_data_source.set_selection` needs a serial a real input event carried). No public accessor exists yet; the self-validation test reads it through the crate-internal state lock, and the item carries a narrowly scoped `#[cfg_attr(not(test), allow(dead_code))]` until the clipboard pass consumes it.
- **`block_until` is imported from the crate root.** `crate::wait` is not reachable from this module, so the input waits call the `crate::block_until` re-export with a closure that re-locks `ClientState`, scanning the history in bounded slices without touching the pump channel (a concurrent `pump_until`/`roundtrip` cannot lose notifications).
- **`wl_seat::Capability` is a `bitflags` type**, so the capability bits come from its `const fn bits()`; the raw bitfield is the ground truth because a seat with pointer *and* keyboard arrives as `WEnum::Unknown(0b11)`.
- **Reader-thread Wayland client.** `wayland-client` is synchronous while tests are async, so the client is split: the calling thread only sends requests through the `Connection`, the reader thread owns the `EventQueue` (`prepare_read()` → `read()` → `dispatch_pending`) and sends one `PumpEvent` per cycle on a `tokio::sync::mpsc` channel. Public pumps await that channel with a deadline; teardown uses `UnixStream::shutdown` plus a bounded join, so nothing can block forever.
- **One pixel ground truth.** `FillPattern::at(x, y, size)` is evaluated both by the SHM writer and by `ImageAssert::matches_pattern`, so the client and the assertion cannot disagree.

## Known Issues

- None recorded.

## Notes for Agents

- `mod.rs` documents the reader-thread, lock-order (`ClientState` → `WindowState`) and teardown rules; `state.rs` documents configure sequencing; `input.rs` documents the recording contract; `shm.rs` documents the Argb8888 byte order.
- `state.rs`'s `ClientState`/`WindowSlot` are `pub(crate)` but the modules are private to `wayland`, so a *descendant* module (e.g. `input::tests`) may still reach them; public tests must go through the documented `WaylandTestClient` API.
- Test-side windows are per-runtime ids and popups never appear in `list_windows`; see the crate-level CONTEXT.md for details.
