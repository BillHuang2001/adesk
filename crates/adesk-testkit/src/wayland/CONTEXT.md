# wayland — the Wayland test client

## Intent

A `wayland-client`-based client that drives the compositor over the *real* protocol path: SHM buffers with known fills, xdg-shell toplevels and popups, bounded event pumps.
It must never reach into compositor internals.

## API Surface

- `WaylandTestClient` — `connect` / `connect_in`, `display_name`, `globals`, `is_closed`, `create_toplevel`, `create_popup`, `pump_for`, `pump_until`, `roundtrip`, `flush`, `close`.
- Seat-input recording on `WaylandTestClient` — `pointer_events()`, `keyboard_events()`, `clear_input_events()`, `wait_for_pointer_event(timeout, what, pred)`, `wait_for_keyboard_event(...)`, `wait_for_pointer_button(button, ButtonState, timeout)`, `wait_for_key(Keycode, KeyState, timeout)`, `last_modifiers()`; all `&self`, all waits deadline-bounded.
- Recorded event types (module `input`, re-exported here): `PointerEvent` (`Enter`/`Leave`/`Motion`/`Button`/`Axis`/`Frame`), `KeyboardEvent` (`Keymap`/`Enter`/`Leave`/`Key`/`Modifiers`/`RepeatInfo`), `AxisKind`, `ButtonState`, `KeyState`, `ModifiersState`, plus `BTN_LEFT` / `KEY_LEFTCTRL` / `KEY_C`.
- `ToplevelSpec`, `TestWindow`, `ConfiguredSize`, `PopupSpec`, `TestPopup`, `PumpStats`, `Globals`.
- These items are reachable as `adesk_testkit::wayland::…`; `lib.rs` re-exports only the window/globals subset.

## Constraints

- A reader thread owns the `EventQueue` and dispatches into `Arc<Mutex<ClientState>>`; windows hold a `QueueHandle` plus their per-window `Arc<Mutex<WindowState>>`, so every method takes `&self`.
- `connect_in` connects an absolute socket path (env-independent, parallel-test safe); `connect` resolves `$XDG_RUNTIME_DIR`.
- Every pump/wait takes a deadline; teardown is `UnixStream::shutdown` plus a bounded join — never an unbounded block.
- `#![forbid(unsafe_code)]`: SHM is `tempfile`-backed and written via `FileExt::write_all_at`; Argb8888 little-endian bytes are `[b, g, r, 255]`.
- Only opaque fills (`FillPattern::require_opaque`); `connect_in` fails with `Unsupported` when the compositor does not advertise ARGB8888.

## Routing Table

| Area | Owner |
|---|---|
| Client lifecycle, pumps, reader thread | `mod.rs` |
| `Dispatch` impls and shared state | `state.rs` |
| SHM pool and buffers | `shm.rs` |
| Window/popup objects and configure state | `window.rs` |
| Global binding, version negotiation | `protocol.rs` |
