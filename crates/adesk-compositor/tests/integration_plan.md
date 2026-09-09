# adesk-compositor integration test plan (Phase 2)

Document only — no code here. These are the five behavior tests that prove the
compositor core, written against `adesk-testkit` once that crate lands
(`docs/architecture.md` §10). Until then they are tracked here so the Phase 2
implementations (protocol handlers, seat path, WM bridge) have acceptance criteria.

## Ground rules

- **No desktop, no GPU, no network, no installed application.** Every test starts a
  real compositor thread in-process on a temporary `XDG_RUNTIME_DIR` with
  `RendererKind::Pixman`, and drives it with a `wayland-client` test client.
- **Events are the assertion surface**, not sleeps: read from the broadcast tap and
  assert on `seq` order, then assert state via `QueryState`. Deadlines are explicit
  and generous; no test sleeps in a loop.
- **`RenderWindow`/`RenderOutput` only when pixels are the assertion**; they are
  commands, not a frame loop.
- **GL-only tests** (renderer fallback, DMA-BUF readback) are gated behind
  `ADESK_TEST_GL=1` and must skip cleanly when it is unset (`docs/architecture.md` §10).
- File: `crates/adesk-compositor/tests/window_lifecycle.rs`, `input_delivery.rs`,
  `popups.rs`, `clipboard.rs` (one file per scenario group is fine; keep each test
  independent).

## Planned testkit surface these tests rely on

| Helper | Used for |
|---|---|
| `TestRuntime::start(CompositorConfig) -> TestRuntime` | in-process compositor + temp runtime dir + socket |
| `runtime.handle() -> CompositorHandle` | command/event/readiness access |
| `runtime.events() -> EventTap` (`next()`, `drain()`, `await_seq(n)`) | ordered event assertions |
| `runtime.connect() -> WaylandTestClient` | one `wayland-client` connection |
| `client.create_toplevel(size) -> TestToplevel` | `wl_surface` + `xdg_toplevel` |
| `client.commit_shm_buffer(&toplevel, pattern)` | SHM buffer with a known pixel pattern |
| `toplevel.await_configure() -> (Size, WindowState)` | observe the tiling configure |
| `toplevel.set_title(..)`, `toplevel.set_app_id(..)`, `toplevel.destroy()` | lifecycle mutations |
| `client.create_popup(&toplevel, position)` | xdg-popup child surface |
| `client.set_selection(mime, bytes)` / `client.read_selection(mime)` | `wl_data_device` clipboard |
| `assert_pixel_eq`, `assert_region_avg`, `assert_differs` | image assertions |
| `desktop_fixture(name, exec)` + helper process | launch/correlation tests (not used here) |

If a landed helper name differs, reconcile the test and this table in the same
commit.

## 1. A window appears with a tiling configure

**Setup:** `TestRuntime::start(pixman_config().with_output_size(1280x800))`, one
`WaylandTestClient`, one `create_toplevel()`.

**Steps:** commit an SHM buffer of exactly the output size; await the configure.

**Assertions:**
- configure size is `1280x800` and state is `Activated` (`docs/architecture.md` §4).
- event tap yields `window_created` with a fresh `window_id`, `app_id`/`title` as
  set by the client, and `seq` strictly greater than the previous event.
- a follow-up `QueryState` reports exactly one `WindowInfo`: `mapped == true`,
  `state == Active`, `geometry == {0,0,1280,800}`, `active_window_id == Some(id)`,
  `keyboard_focus == Some(id)`.
- the first `SurfaceCommit` for the window carries `commit_seq >= 1` and a damage
  `Region` that intersects the committed area (never empty for a full-buffer commit).
- `RenderWindow { window_id, region: None, max_dimension: None }` returns an image of
  the window's size whose pixels match the committed pattern.

**Helpers:** `runtime.handle()`, `runtime.events()`, `client.commit_shm_buffer`,
`toplevel.await_configure`, `assert_pixel_eq`.

## 2. Focus follows activation

**Setup:** two clients, each with one toplevel (`A` mapped first, then `B`), so the
policy has already auto-activated `B` on map.

**Steps:** `handle.send(RuntimeCommand::ActivateWindow { window_id: A })`; drain the
event tap until the activation is acknowledged.

**Assertions:**
- event order is exactly `window_activated { window_id: A, previous: Some(B) }`
  followed by `focus_changed { window_id: Some(A) }` — both with increasing `seq`.
- the `ActivateWindow` reply is `Ok(())` and is delivered *after* both events are
  observable (commands are FIFO; the reply is sent when the state change is done).
- `QueryState` shows `active_window_id == Some(A)`, `keyboard_focus == Some(A)`,
  `A.state == Active`, `B.state == Inactive`, both `mapped == true` (the inactive
  window stays mapped, it is only not visible).
- `A` receives a new tiling configure for the full output; `B` receives none.
- `ActivateWindow` with an unknown `WindowId` replies `Err(unknown_window)` and emits
  no events (state is untouched).
- activation is **never** synthesized input: the event tap contains no
  `SurfaceCommit`/pointer events caused by the activation itself.

**Helpers:** `runtime.events()`, `toplevel.await_configure`, `QueryState`.

## 3. Input delivery through the real seat path

**Setup:** one client with a focused toplevel; the test client installs
`wl_pointer`/`wl_keyboard` listeners and records every event.

**Steps:** `PointerMove { position: Normalized(0.5, 0.5) }`, then
`PointerButton { button: Left, state: Pressed }` / `Released`, then
`PointerAxis { dx: 0.0, dy: -3.0 }`, then `KeyEvent { key: KeyCode::parse("ctrl+c"),
state: Pressed }`.

**Assertions:**
- the pointer `wl_pointer.motion` surface coordinates equal the point the window
  model produces for the window's geometry (window-relative → output → surface, via
  `WmBridge::resolve_position`, never a hard-coded `(0,0)` assumption).
- `wl_pointer.button` reports `BTN_LEFT` with the matching `state`; press and release
  are two distinct events in command order.
- `wl_pointer.axis` reports `vertical` with a negative value and is followed by
  `wl_pointer.frame`.
- the chord `ctrl+c` arrives as `wl_keyboard.key` for LeftCtrl (`Pressed`) then `c`
  (`Pressed`), then the reverse order for the implicit release — press in order,
  release in reverse (`docs/architecture.md` §8).
- `KeyEvent { state: Released }` with a chord replies `Err(invalid_request)` and
  sends nothing to the client.
- injection with no focused window replies `Err(...)` (mapped to
  `unknown_window`/`invalid_request`), never panics.

**Helpers:** `client.commit_shm_buffer` (to give the window real geometry),
`runtime.events()`, `QueryState` for geometry cross-checks.

## 4. Popup tracking

**Setup:** one client with a mapped toplevel; `client.create_popup(&toplevel, pos)`.

**Steps:** map the popup and commit a buffer, then destroy the popup surface.

**Assertions:**
- `popup_appeared { window_id, popup_id }` is emitted with the *owner* window id, and
  `QueryState` reports `popup_count == 1` for that window.
- `RenderWindow` of the owner includes the popup content (the pixel at the popup
  position matches the popup pattern, not the toplevel pattern).
- popup commits emit `surface_commit` with the owner `window_id` and a monotonic
  `commit_seq` that shares the window's counter.
- destroying the popup emits `popup_disappeared` with the same `popup_id` and
  `popup_count` returns to `0`; the window itself stays mapped and active.
- destroying the owner window while a popup is open emits `popup_disappeared` first,
  then `window_destroyed` (no dangling popup ids in later snapshots).

**Helpers:** `client.create_popup`, `runtime.events()`, `QueryState`,
`assert_region_avg`.

## 5. Clipboard basics

**Setup:** two clients (`owner`, `reader`) with mapped toplevels; `owner` has
keyboard focus.

**Steps:** `owner` calls `wl_data_device.set_selection` with `text/plain;charset=utf-8`
and a known payload; `reader` requests the same mime type.

**Assertions:**
- `reader` receives a `wl_data_offer` containing the advertised mime type and can
  read back the exact bytes through the pipe (no truncation, no re-encoding).
- a second `set_selection` by `owner` invalidates the previous offer and the `reader`
  read-back returns the new payload.
- `set_selection` with a mime type the reader does not request is not delivered to
  the reader; requesting an unadvertised mime type fails at the client, not with a
  compositor panic.
- when the focus moves to `reader` (scenario 2 mechanism), `reader` becomes the
  selection target and can `set_selection` itself; the compositor never inspects or
  logs the payload (no pixel/selection data in logs).

**Helpers:** `client.set_selection`, `client.read_selection`, `runtime.events()`
(assert no clipboard content appears in any event).

## Phase 2 gate

These tests are added only after: (a) the protocol handlers call the `State`
side-effect API instead of `todo!()`, (b) `adesk-wm` and `adesk-render` have landed
and `WmBridge`'s assumed surface (see `src/wm.rs`) has been reconciled, and
(c) `adesk-testkit` exposes the helpers above. Until then the public-API smoke tests
in `tests/compositor_smoke.rs` remain `#[ignore]`d.
