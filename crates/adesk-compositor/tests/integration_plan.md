# adesk-compositor integration tests

`tests/window_lifecycle.rs`, `tests/input_delivery.rs`, `tests/popups.rs`,
`tests/clipboard.rs`, `tests/reserve_seq.rs` and `tests/output_composition.rs` hold the 20
integration tests that prove the compositor core end to end.
Each starts a **real in-process runtime** — one compositor thread, one virtual output, the real
protocol handlers and seat — and drives it the way an ordinary application does: a
`wayland-client` connection (`adesk-testkit`) over the runtime's own socket, `RuntimeCommand`s
on the command channel, and the compositor's `RuntimeEvent` broadcast plus the window model as
the assertion surface. There is no AGP client, no screenshot loop and no fixture application in
these suites.

They build on `adesk-testkit` (a dev-dependency of this crate) and are ordinary
`#[tokio::test]`s: no `#[ignore]`, no feature gate, no external service.

```sh
./scripts/dev.sh cargo test -p adesk-compositor                    # all suites
./scripts/dev.sh cargo test -p adesk-compositor --test clipboard   # one suite
```

## Ground rules

- **No desktop, GPU, network or installed application.** Every test starts its own compositor
  thread on a private temp `XDG_RUNTIME_DIR` (mode `0700`, created by the harness) with
  `RendererKind::Pixman` (the `TestRuntimeConfig` default) and connects by absolute socket path.
  Nothing is launched, so `TestRuntimeConfig::new().with_apply_env(false)` releases the harness's
  process-env lock after startup and the tests stay independent and parallel.
- **Events are the assertion surface**, not sleeps: read the broadcast through `EventAssert` and
  assert on `seq` order, then assert state via `QueryState`. Deadlines are explicit (a 10 s
  `DEADLINE`; a 250 ms `QUIET_BOUND` for negative claims) and no test sleeps or polls in a loop.
  The broadcast never replays, so a tap is installed *before* the action it must observe.
- **`roundtrip` is a flush, not a synchronization barrier** (it waits one reader poll cycle).
  An assertion that must come after a client request commits on the same connection and awaits
  the resulting `surface_commit` — the late-app-id test in `window_lifecycle.rs` is the model.
- **`RenderWindow`/`RenderOutput` only where pixels are the assertion**; they are commands, not
  frame loops. (`RenderOutput` is used by `output_composition.rs` to prove output-level
  composition; the other suites assert on `RenderWindow`.)
- **GL-only paths** (renderer selection, DMA-BUF readback) sit behind `ADESK_TEST_GL=1` and skip
  cleanly when it is unset (`adesk_testkit::{gl_enabled, require_gl}`). Every test in these six
  files runs under pixman and is ungated.
- **One file per scenario group**, each test independent: its own runtime, its own Wayland
  connection(s), its own window ids.
- **Expectations come from the window model**, never a hard-coded `1280x800`:
  `TestRuntime::output_size`, `TestRuntime::tiled_rect` and `expected_window_geometry`.

## 1. A window appears with a tiling configure — `tests/window_lifecycle.rs`

- `window_appears_with_tiling_configure` — a client asking for the output size is configured by
  the tiling policy with the full output, a non-zero serial and the `activated` state, not with
  the size it requested. The mapping commit then produces `window_created` (fresh id, echoed
  `app_id`/`title`), `window_activated` and `surface_commit` (`commit_seq >= 1`, window-relative
  damage covering the committed area); `QueryState` reports exactly one window — `mapped`,
  `Active`, `geometry == tiled_rect()`, `last_commit_seq >= 1`, `active_window_id ==
  keyboard_focus == id`. `RenderWindow` returns a frame at the tiled size whose `commit_seq` is
  the window's and whose pixels match the committed checker fill (and differ from a blank frame).
- `late_app_id_reaches_the_window_model` — after the mapped toplevel has committed once, the client
  sets `app_id` again (a post-map mutation through the real protocol) and `QueryState` must report
  the late value: the write-back is verified through the public observation path after a *commit
  barrier* (a commit on the same connection whose `surface_commit` event is awaited, since
  `roundtrip` is not a `wl_display.sync` barrier), whose own single event and commit step are
  accounted for; geometry, state, title, focus, window count and configure state stay unchanged,
  and the write-back itself emits nothing.

## 2. Focus follows activation — `tests/window_lifecycle.rs`

- `focus_follows_activation` — two clients `A`/`B`; `B`'s map auto-activates `B` and leaves `A`
  `Inactive` but still `mapped` at the tiled geometry. `ActivateWindow { A }` then emits exactly
  `window_activated { A, previous: Some(B) }` followed by `focus_changed { Some(A) }` (two events,
  increasing `seq`, and the reply resolves only after both are queued), while `QueryState` shows
  `A` `Active` / `B` `Inactive` / both mapped / `active_window_id == keyboard_focus == A`. The
  seat really moved (`wl_keyboard.leave` on `B`, a later `enter` on `A`), an unknown `WindowId`
  replies `unknown_window` and changes nothing (`Expected::Any` bounded by `QUIET_BOUND`, snapshot
  watermark unmoved), and neither connection saw a pointer or key event from the activation.

**Current semantics: activation does not re-tile.** `adesk-wm`'s `activate` returns exactly
`[WmAction::Activate { id }]`, and `ConfigureWindow` — the only action that sends
`xdg_toplevel.configure` — comes from `on_map` and `on_output_size` only. An activation therefore
sends no configure to either connection: `last_configure`/`pending_configure` are asserted
unchanged on both clients (after flushes give the protocol time to deliver any configure), and the tiling state that a
configure would have carried is asserted positively instead (`geometry == tiled_rect()` for both
windows, `A`'s last configure being the full-output tiling configure with `activated`).

## 3. Input through the real seat — `tests/input_delivery.rs`

- `pointer_move_delivers_the_window_model_point` — the entry move arrives as `wl_pointer.enter` at
  the window model's point; the following `PointerMove { Normalized(0.5, 0.5) }` arrives as
  exactly one `wl_pointer.motion` at that same model point, an interior pixel of the tiled rect
  (never a hard-coded `(0, 0)`), within 0.25 px of the model's integer point and within one pixel
  of the naive `n * dim` product.
- `pointer_button_press_and_release_are_two_ordered_events` — Left press and release arrive as two
  distinct `wl_pointer.button` events carrying `BTN_LEFT`, in command order, with nothing else in
  the filtered history.
- `pointer_axis_is_negative_vertical_and_framed` — `PointerAxis { dx: 0.0, dy: -3.0 }` arrives as a
  single `wl_pointer.axis` on the vertical axis with a negative value (the injected delta
  quantized to `wl_fixed`, 1/256 px); the zero horizontal delta is omitted rather than reported as
  `0.0`, and a `wl_pointer.frame` follows the axis.
- `ctrl_c_chord_is_a_press_in_order_and_a_reverse_release` — a pressed chord is delivered as
  LeftCtrl↓, `c`↓, `c`↑, LeftCtrl↑ in exactly that order (`KEY_LEFTCTRL`/`KEY_C`), starting from a
  seat that really holds the keyboard focus.
- `a_released_chord_is_rejected_and_delivers_nothing` — a chord with `KeyState::Released` replies
  `invalid_request`, and the only key the client ever records is the single key injected afterwards
  (a leaked key would have been written to the socket first).
- `injection_without_a_focused_window_fails_without_panicking` — with no window at all, every input
  command (`PointerMove`, `PointerButton`, `PointerAxis`, `KeyEvent`) replies `invalid_request`, the
  model is untouched and the thread keeps serving commands.

**Current semantics.** Chords are built with `KeyCode::parse_chord(["CTRL", "C"])`:
`KeyCode::parse` is the *single-key* parser (`KeyCode::parse("ctrl+c")` is an error, asserted in
the test), which is also what the server's `keypress` path uses. Pointer button and axis delivery
require pointer focus, and the first move onto a surface is reported as `wl_pointer.enter`, not as
`motion` — so every test makes a leading move, waits for it, and clears the recorded history
before the injection under test. No window at all yields `invalid_request` (zero windows makes
`unknown_window` unreachable on this path).

## 4. Popups — `tests/popups.rs`

- `popup_lifecycle_renders_into_the_owner_and_destroy_keeps_the_window` — `popup_appeared` names
  the *owner's* `WindowId` (a popup never consumes a window id) and `QueryState` reports
  `popup_count == 1` for that window with still exactly one window. The popup's commit is reported
  as `surface_commit` against the owner with window-relative damage covering the popup rect and a
  `commit_seq` that advances the window's single counter (read back as `last_commit_seq` and
  carried by the `RenderedFrame`). The popup is composed into the owner's frame: the pixel at its
  origin plus a popup-local offset is the popup's own fill, while the owner's pixels survive
  everywhere else. Destroying the popup emits `popup_disappeared` with that exact `popup_id` and
  owner, `popup_count` returns to `0`, the popup's pixels leave the composition, and the window
  stays `mapped` and `Active`.
- `owner_destroyed_with_open_popup_reports_popup_disappeared_first` — destroying the owner while a
  popup is open reports `popup_disappeared` (same owner, same `popup_id`) *before* `window_destroyed`,
  with `seq` ascending across the pair; afterwards the model keeps neither record, `active_window_id`
  and `keyboard_focus` are `None`, and no further lifecycle event names the destroyed window.

## 5. Clipboard — `tests/clipboard.rs`

- `reader_receives_the_offer_and_reads_the_exact_bytes` — a freshly connected client has no offer;
  after the owner publishes, the reader still has none (a selection is announced only to the
  data-device-focus client), and activating the reader makes `wl_data_offer` arrive, from which the
  exact published bytes are read back.
- `second_set_selection_supersedes_the_first_offer` — the first publication is readable, then the
  owner re-takes the focus, publishes a different payload and the reader (focused again) receives a
  *new* offer (count 2, announced once) carrying the second payload; the superseded payload is no
  longer reachable.
- `unadvertised_mime_type_reads_as_none` — reading a mime type the offer does not advertise is
  `Ok(None)` rather than an error or a hang, while the advertised mime type still round-trips
  through the same offer (non-vacuity).
- `focus_moves_to_the_reader_and_it_publishes_back` — the newly focused reader reads what the owner
  published, publishes its own payload (with its own real input serial) and the owner, once focused
  again, reads the reader's bytes: the transfer direction is reversible.
- `no_runtime_event_carries_clipboard_payload` — a distinctive payload really crosses the protocol,
  then the whole recorded event history (proven live by a later `surface_commit`) is scanned and
  must not contain it; `window_created`, `window_activated` and `surface_commit` are present as
  non-vacuity evidence.

**Current semantics.** Smithay accepts `wl_data_device.set_selection` only from the client that
currently holds **keyboard focus**, and announces a selection only to the client that holds
**data-device focus**. The compositor keeps the two together — `State::apply_activate` is the only
keyboard-focus path and moves the data-device focus in the same call — so the tests must produce
both: the owner maps **second** (the single-visible-toplevel policy gives the visible slot and the
focus to the newest toplevel), the serial `set_selection` quotes comes from a real input event (one
injected pointer move, whose peer's own `wl_pointer.enter` is awaited), and the reader becomes the
selection target by being *activated*, never by synthesized input. The compositor stores no
selection bytes at all (`SelectionHandler::SelectionUserData` is `()`), so nothing clipboard-shaped
exists to log or to put in an event.

## 6. Server-synthesized seqs — `tests/reserve_seq.rs`

- `reserving_is_strictly_increasing_and_silent` — two `ReserveSeq`s on a fresh runtime: the values
  strictly increase, the `QueryState` watermark equals the reserved number (one seq domain — a
  second counter would show up as a mismatch here), the window model is untouched, and a tap
  installed before the reservations receives nothing (bounded by `QUIET_BOUND`).
- `later_events_do_not_reuse_reserved_seqs` — two reservations, then a real toplevel map over the
  Wayland path: the first event after the reservations is exactly `second + 1`, every event of the
  tail sits strictly above the reserved watermark, and the closing `QueryState` covers both the
  reservations and the later event. Gaps are allowed, reuse is not (`docs/protocol.md` §1).

## 7. Single visible toplevel composition — `tests/output_composition.rs`

- `composition_renders_only_the_active_window` — two clients map toplevels with distinct opaque
  fills. Activating one and capturing the composed output (`RuntimeCommand::RenderOutput` with
  overlays disabled) matches the ACTIVE window's fill on *every* pixel and differs from the
  inactive window's own `RenderWindow` frame — rendered first as non-vacuity, which proves the
  excluded pixels really exist and that observing them neither activates them nor emits an event.
  `QueryState` proves exactly one `Active`/one `Inactive` at each capture; the roles are then
  reversed and the proof repeated.
- `output_without_active_window_is_a_clear_frame` — a fresh runtime (nothing ever mapped) composes
  to the pipeline's default clear color; after the only window is closed via `CloseWindow` and
  destroyed by its client, the composition is a clear frame again and differs from the frame
  captured while the window was alive.

## Testkit surface used

| Helper | Used for |
|---|---|
| `TestRuntime::start_with(TestRuntimeConfig)` | in-process compositor + private temp `XDG_RUNTIME_DIR` + bound socket |
| `TestRuntimeConfig::{new, with_apply_env}` | pixman renderer, `1280x800` output, env scoping |
| `runtime.compositor() -> &CompositorHandle` | command channel (`RuntimeCommand` + reply oneshots, incl. `RenderOutput` capture and `ReserveSeq`) |
| `runtime.wayland_client() -> WaylandTestClient` | one `wayland-client` connection (absolute socket path) |
| `runtime.output_size()`, `runtime.tiled_rect()` | expected geometry from the wm policy |
| `runtime.shutdown()` | bounded runtime teardown |
| `expected_window_geometry(Size)`, `wait_until(deadline, what, cond)` | single-sourced tiling rect, bounded state polls |
| `EventAssert::tap(&runtime)` | ordered event assertions from the broadcast |
| `Expected::{WindowCreatedFor, WindowActivated, WindowDestroyed, SurfaceCommit, PopupAppeared, Any, custom}` | event matchers |
| `EventAssert::{wait_for_expected, wait_for, expect_none, drain, seen, assert_seen_order}` | waits, bounded negative claims, history scans |
| `WaylandTestClient::{create_toplevel(ToplevelSpec), create_popup(&TestWindow, PopupSpec)}` | protocol-path surfaces |
| `WaylandTestClient::{roundtrip, flush, close}` | flush (`roundtrip` = flush + one reader cycle — not a `wl_display.sync` barrier) and connection teardown |
| `WaylandTestClient::{pointer_events, keyboard_events, clear_input_events, wait_for_pointer_event, wait_for_pointer_button, wait_for_keyboard_event, wait_for_key, last_modifiers}` | recorded `PointerEvent`/`KeyboardEvent` history as assertion source |
| `WaylandTestClient::{set_selection, read_selection, read_selection_with_timeout, selection_offer_count, wait_for_selection_offer}` | `wl_data_device` clipboard |
| `TestWindow::{wait_for_configure, apply_configure, commit_frame(FillPattern), set_app_id, commit_pending, destroy, size, damage_hint, pending_configure, last_configure}` | toplevel lifecycle |
| `TestPopup::{wait_for_configure, apply_configure, commit_frame, destroy}` | popup lifecycle |
| `ToplevelSpec::{new, with_fill}`, `PopupSpec::{new, with_offset}` | surface parameters (size, app id, title, fill, positioner offset) |
| `FillPattern::{default, solid_rgb, checker}` + `FillPattern::at` | known pixel patterns and their expected pixels |
| `ImageAssert::{new, pixel, matches_pattern, matches_solid, differs_from}`, `ImageBuffer::new_rgba` | pixel assertions on `RenderedFrame` payloads |
| `BTN_LEFT`, `KEY_LEFTCTRL`, `KEY_C`, `AxisKind`, `ButtonState`, `KeyState` | properties asserted on recorded seat events |
