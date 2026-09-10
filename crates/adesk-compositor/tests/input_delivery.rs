//! Integration scenario 3 (`tests/integration_plan.md` §3): input delivery through the
//! real seat path.
//!
//! Each test starts a real in-process runtime (pixman, private temp `XDG_RUNTIME_DIR`),
//! connects one `WaylandTestClient` and maps one toplevel, which the tiling policy makes
//! visible and focused. Input is injected **directly** through `RuntimeCommand` on the
//! compositor handle — no AGP client — so what is asserted is the compositor's own seat
//! path rather than the server's request plumbing. The client records every `wl_pointer` /
//! `wl_keyboard` event it receives (`adesk_testkit`'s recorder, `wayland/input.rs`), and
//! the tests assert on that recorded history: no sleeps, every wait deadline-bounded.
//!
//! | Test | What it proves |
//! |---|---|
//! | [`pointer_move_delivers_the_window_model_point`] | `PointerMove { Normalized(0.5, 0.5) }` arrives as `wl_pointer.motion` carrying exactly the point `adesk-wm`'s resolution rule produces for the window geometry reported by `QueryState` |
//! | [`pointer_button_press_and_release_are_two_ordered_events`] | `PointerButton` Left pressed/released arrive as two distinct `wl_pointer.button` events with `BTN_LEFT`, in command order |
//! | [`pointer_axis_is_negative_vertical_and_framed`] | `PointerAxis { dx: 0.0, dy: -3.0 }` arrives as `wl_pointer.axis(vertical, < 0)` and is terminated by `wl_pointer.frame` |
//! | [`ctrl_c_chord_is_a_press_in_order_and_a_reverse_release`] | a pressed chord arrives as LeftCtrl↓, c↓, c↑, LeftCtrl↑ in exactly that order |
//! | [`a_released_chord_is_rejected_and_delivers_nothing`] | a chord with `KeyState::Released` replies `Err(invalid_request)` and sends no key event to the client |
//! | [`injection_without_a_focused_window_fails_without_panicking`] | with no toplevel at all every injection replies `Err(invalid_request)` and the thread keeps serving commands |
//!
//! # Coordinate expectations
//!
//! Window-relative coordinates are resolved by the window model
//! (`adesk_wm::WindowManager::resolve_position`, documented in `adesk-wm/src/policy.rs`:
//! resolve against a rect at the origin with the window's *size*, then translate by the
//! geometry origin), and `adesk_core::Position::resolve` maps a normalized value onto the
//! *last* pixel of that rect (`round(n * (dim - 1))`, never `n * dim`). The expected point
//! is therefore derived from the runtime's own tiled geometry — read over `QueryState`,
//! never from a hard-coded output size — with the model's own conversion
//! ([`model_point`]); the naive `n * dim` product is checked separately against a
//! documented ≤ 1 px difference so a materially wrong transform (an output-size rect, a
//! swapped axis) still fails.
//!
//! # The pointer-focus prerequisite
//!
//! Smithay delivers `wl_pointer.button` and `wl_pointer.axis` only to a surface that has
//! pointer focus, and the *first* move onto a surface is the focus change: it is reported
//! as `wl_pointer.enter`, not as `wl_pointer.motion`. Every test here therefore makes a
//! leading move and waits for it to be recorded before the plan's injections (this is the
//! hazard `crates/adesk-compositor/CONTEXT.md` documents for `PointerButton`/`PointerAxis`),
//! and clears the recorded history afterwards so each assertion is scoped to the injection
//! it names.
//!
//! # Deviations from the plan text (evidence is asserted, not glossed over)
//!
//! - Plan §3 writes the chord as `KeyCode::parse("ctrl+c")`. `KeyCode::parse` parses a
//!   **single** key name (see its docs and unit tests); chords are built with
//!   `KeyCode::parse_chord(["CTRL", "C"])`, which is exactly what the server's `keypress`
//!   path uses (`adesk-server/src/dispatch/input.rs`). The chord test asserts the
//!   difference next to the chord it injects instead of silently substituting an API.
//! - The plan moves straight to `Normalized(0.5, 0.5)`; the tests add the entry move the
//!   focus prerequisite above requires, and assert that move's own coordinates too.
//!
//! # Execution model
//!
//! The recorders and their waiters are synchronous, so the tests run on a small
//! multi-thread executor: a bounded wait can never stall the runtime's tasks. No
//! application is launched, so the runtime runs with `apply_env(false)` and the process
//! environment is never held for its lifetime (`TestRuntime::wayland_client` connects by
//! absolute socket path).

use std::time::Duration;

use adesk_compositor::{CompositorHandle, KeyCode, RuntimeCommand, StateSnapshot};
use adesk_core::{
    AppId, Button, ButtonState, ErrorCode, KeyState, Position, Rect, WindowId, WindowState,
};
use adesk_testkit::{
    AxisKind, ButtonState as RecordedButtonState, EventAssert, Expected, FillPattern,
    KeyState as RecordedKeyState, KeyboardEvent, PointerEvent, Result, Size, TestRuntime,
    TestRuntimeConfig, TestWindow, ToplevelSpec, WaylandTestClient, BTN_LEFT, KEY_C, KEY_LEFTCTRL,
};
use tokio::sync::oneshot;

/// Every bounded wait and deadline in this file.
const DEADLINE: Duration = Duration::from_secs(10);

/// App id of the toplevel every test maps.
const APP_ID: &str = "org.example.compositor.input";

/// Tolerance when comparing an injected position with a recorded surface-local coordinate,
/// in pixels.
///
/// Tighter than the 0.5 px a caller may assume: the delivered value is the window model's
/// integer point and the protocol's `wl_fixed` step is 1/256 px, so anything above that
/// step but far below a mis-resolved coordinate is enough to catch a real failure.
const COORD_EPSILON: f64 = 0.25;

/// Tolerance for a recorded `wl_pointer.axis` value: the protocol quantizes it to
/// `wl_fixed` (1/256 px).
const AXIS_EPSILON: f64 = 1.0 / 256.0;

/// The extra tolerance allowed when comparing a recorded coordinate with the naive
/// `normalized * dimension` product (the model maps onto the last pixel, so the two may
/// differ by up to one pixel).
const NAIVE_MAPPING_TOLERANCE: f64 = 1.0;

/// The plan's `Normalized(0.5, 0.5)` — the position the injected actions target.
const CENTRE: Position = Position::normalized(CENTRE_X, CENTRE_Y);
/// Horizontal component of [`CENTRE`], kept for the naive-product comparison.
const CENTRE_X: f64 = 0.5;
/// Vertical component of [`CENTRE`].
const CENTRE_Y: f64 = 0.5;

/// The focus-establishing entry move. Any point different from [`CENTRE`] works; this one
/// is also far from both edges, so the model's rounding cannot put it on a boundary.
const ENTRY: Position = Position::normalized(0.25, 0.75);

/// The tapped `ctrl+c` chord exactly as the seat must deliver it: pressed in order,
/// released in reverse (`docs/architecture.md` §8).
const CHORD_SEQUENCE: [(u32, RecordedKeyState); 4] = [
    (KEY_LEFTCTRL, RecordedKeyState::Pressed),
    (KEY_C, RecordedKeyState::Pressed),
    (KEY_C, RecordedKeyState::Released),
    (KEY_LEFTCTRL, RecordedKeyState::Released),
];

/// A runtime whose Wayland socket the protocol-path client can reach, with no application
/// launching and no process-env scoping.
fn wayland_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// Starts a runtime and connects one client to its Wayland socket.
async fn start_runtime() -> Result<(TestRuntime, WaylandTestClient)> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let wayland = runtime.wayland_client()?;
    Ok((runtime, wayland))
}

/// Maps one toplevel, acknowledges its tiling configure and commits a frame.
///
/// The returned window comes back mapped, tiled and focused — the runtime's own
/// `window_created` / `window_activated` events say so (awaited here, never assumed) — and
/// the `wl_keyboard.keymap` wait is the seat-readiness barrier: the client creates its
/// `wl_pointer` and `wl_keyboard` from the same `wl_seat.capabilities` event, so once the
/// keymap has been delivered there are seat objects for an injection to reach.
async fn map_toplevel(
    runtime: &TestRuntime,
    wayland: &WaylandTestClient,
) -> Result<(TestWindow, WindowId)> {
    // The event broadcast does not replay: tap before the first commit maps the surface.
    let mut events = EventAssert::tap(runtime);
    let window =
        wayland.create_toplevel(ToplevelSpec::new(APP_ID, "Input", Size::new(320, 200)))?;
    let configure = window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy configures the toplevel, not the 320x200 the client asked for"
    );
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?;
    let window_id = created
        .window_id()
        .expect("window_created carries the new window id");
    events
        .wait_for_expected(&Expected::WindowActivated(window_id), DEADLINE)
        .await?;

    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.keymap",
        |event| matches!(event, KeyboardEvent::Keymap { size, .. } if *size > 0),
    )?;
    Ok((window, window_id))
}

/// Sends one result-bearing command and awaits its reply.
///
/// Every input command is answered from inside the callback that produced it, *after* the
/// state change, so awaiting the reply is what lets the command channel's FIFO order stand
/// in for "the seat saw these in this order".
async fn reply_of(
    handle: &CompositorHandle,
    build: impl FnOnce(oneshot::Sender<adesk_core::Result<()>>) -> RuntimeCommand,
) -> adesk_core::Result<()> {
    let (reply, answer) = oneshot::channel();
    handle
        .send(build(reply))
        .expect("the compositor accepts the command");
    answer.await.expect("every input command is answered")
}

/// `PointerMove` to a window-relative position (the focused window is the target).
async fn move_pointer(handle: &CompositorHandle, position: Position) -> adesk_core::Result<()> {
    reply_of(handle, |reply| RuntimeCommand::PointerMove {
        position,
        reply,
    })
    .await
}

/// `PointerButton` at the current pointer location.
async fn press_button(
    handle: &CompositorHandle,
    button: Button,
    state: ButtonState,
) -> adesk_core::Result<()> {
    reply_of(handle, |reply| RuntimeCommand::PointerButton {
        button,
        state,
        reply,
    })
    .await
}

/// `PointerAxis` at the current pointer location.
async fn scroll(handle: &CompositorHandle, dx: f64, dy: f64) -> adesk_core::Result<()> {
    reply_of(handle, |reply| RuntimeCommand::PointerAxis {
        dx,
        dy,
        reply,
    })
    .await
}

/// `KeyEvent` on the focused window.
async fn key_event(
    handle: &CompositorHandle,
    key: KeyCode,
    state: KeyState,
) -> adesk_core::Result<()> {
    reply_of(handle, |reply| RuntimeCommand::KeyEvent {
        key,
        state,
        reply,
    })
    .await
}

/// `QueryState`: always answered, so a bug shows up as a missing reply, never as a value.
async fn query_state(handle: &CompositorHandle) -> StateSnapshot {
    let (reply, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply })
        .expect("the compositor accepts query_state");
    answer.await.expect("query_state is always answered")
}

/// Moves the pointer onto the toplevel and waits until the seat reports the focus.
///
/// `wl_pointer.button`/`wl_pointer.axis` are only delivered to a focused surface, so this
/// is the prerequisite of every injection below; the first move is the focus change and is
/// recorded as `wl_pointer.enter`.
async fn focus_pointer(
    handle: &CompositorHandle,
    wayland: &WaylandTestClient,
    position: Position,
) -> Result<()> {
    move_pointer(handle, position)
        .await
        .expect("pointer_move is accepted while a toplevel is focused");
    wayland.wait_for_pointer_event(DEADLINE, "pointer focus on the mapped toplevel", |event| {
        matches!(
            event,
            PointerEvent::Enter { .. } | PointerEvent::Motion { .. }
        )
    })
}

/// The geometry the window model holds for `window_id`, cross-checked against the policy.
///
/// The snapshot is the authority for the rect the model resolves against and the id the
/// seat focuses; both are compared with the tiling policy's own rect
/// ([`TestRuntime::tiled_rect`]) so no expectation comes from a hard-coded output size.
async fn focused_window_geometry(runtime: &TestRuntime, window_id: WindowId) -> Rect {
    let snapshot = query_state(runtime.compositor()).await;
    let window = snapshot
        .window(window_id)
        .unwrap_or_else(|| panic!("the mapped toplevel is tracked: {snapshot:?}"));
    assert_eq!(
        window.geometry,
        runtime.tiled_rect(),
        "the tiling policy's geometry is the rect input resolves against: {snapshot:?}"
    );
    assert!(window.mapped, "a focused toplevel is mapped: {snapshot:?}");
    assert_eq!(
        window.state,
        WindowState::Active,
        "the visible toplevel is the active one: {snapshot:?}"
    );
    assert_eq!(
        snapshot.active_window_id,
        Some(window_id),
        "the mapped toplevel is the active window: {snapshot:?}"
    );
    assert_eq!(
        snapshot.keyboard_focus,
        Some(window_id),
        "the seat's keyboard focus is the mapped toplevel: {snapshot:?}"
    );
    window.geometry
}

/// The surface-local point the window model produces for `position` on `window`.
///
/// This is the model's own conversion (`adesk_core::Position::resolve` against a rect at
/// the origin with the window's size — `adesk-wm/src/policy.rs::resolve_position`), and the
/// single visible toplevel is tiled at the output origin with its surface at the window
/// origin, so the window-relative point *is* the surface-local coordinate.
fn model_point(window: Rect, position: Position) -> (f64, f64) {
    let local = position.resolve(Rect::from_size(window.size()));
    (f64::from(local.x), f64::from(local.y))
}

/// Whether a recorded coordinate matches the model's point within [`COORD_EPSILON`].
fn near(recorded: f64, expected: f64) -> bool {
    (recorded - expected).abs() <= COORD_EPSILON
}

/// Index of the first recorded event satisfying `predicate`, in delivery order.
fn first_index<T>(events: &[T], predicate: impl Fn(&T) -> bool) -> Option<usize> {
    events.iter().position(predicate)
}

/// The recorded `wl_pointer.motion` positions, in delivery order.
fn motion_events(wayland: &WaylandTestClient) -> Vec<(f64, f64)> {
    wayland
        .pointer_events()
        .iter()
        .filter_map(|event| match event {
            PointerEvent::Motion { x, y } => Some((*x, *y)),
            _ => None,
        })
        .collect()
}

/// The recorded `wl_pointer.button` events, filtered in order.
fn button_events(wayland: &WaylandTestClient) -> Vec<(u32, RecordedButtonState)> {
    wayland
        .pointer_events()
        .iter()
        .filter_map(|event| match event {
            PointerEvent::Button { button, state } => Some((*button, *state)),
            _ => None,
        })
        .collect()
}

/// The recorded `wl_keyboard.key` events, filtered in order.
fn key_events(wayland: &WaylandTestClient) -> Vec<(u32, RecordedKeyState)> {
    wayland
        .keyboard_events()
        .iter()
        .filter_map(|event| match event {
            KeyboardEvent::Key { keycode, state } => Some((*keycode, *state)),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pointer_move_delivers_the_window_model_point() -> Result<()> {
    let (runtime, wayland) = start_runtime().await?;
    let (_window, window_id) = map_toplevel(&runtime, &wayland).await?;
    let handle = runtime.compositor();

    let geometry = focused_window_geometry(&runtime, window_id).await;

    // The focus-establishing entry move: it is delivered as `wl_pointer.enter`, and the
    // entry coordinates are the model's point for the entry position (proving the
    // resolution rule is used for the entry too, not just for later motions).
    let entry = model_point(geometry, ENTRY);
    move_pointer(handle, ENTRY)
        .await
        .expect("the entry move is accepted");
    wayland.wait_for_pointer_event(DEADLINE, "wl_pointer.enter at the entry point", |event| {
        matches!(event, PointerEvent::Enter { x, y, .. } if near(*x, entry.0) && near(*y, entry.1))
    })?;

    // Scope the recorded history to the injection under test.
    wayland.clear_input_events();

    // Plan §3 step 1: the centre move must arrive as `wl_pointer.motion` carrying the
    // point the window model produces for the window's geometry.
    let expected = model_point(geometry, CENTRE);
    move_pointer(handle, CENTRE)
        .await
        .expect("the centre move is accepted");
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.motion at the window model's point",
        |event| {
            matches!(event, PointerEvent::Motion { x, y }
                if near(*x, expected.0) && near(*y, expected.1))
        },
    )?;

    // Exactly one motion, and it is the model's point: a compositor that delivered some
    // other point (or delivered two differently-resolved motions) fails here.
    let motions = motion_events(&wayland);
    assert_eq!(
        motions.len(),
        1,
        "the injection produced exactly one wl_pointer.motion: {:?}",
        wayland.pointer_events()
    );
    let (x, y) = motions[0];
    assert!(
        near(x, expected.0) && near(y, expected.1),
        "the recorded motion ({x}, {y}) must be the model's point {expected:?} for the \
         tiled geometry {geometry:?} (raw history: {:?})",
        wayland.pointer_events()
    );

    // The window-relative centre of a real window is an interior pixel: a compositor that
    // delivered a hard-coded `(0, 0)` (or never resolved the position at all) fails both
    // this check and the equality above.
    assert!(
        x > 0.0 && y > 0.0 && (x as u32) < geometry.w && (y as u32) < geometry.h,
        "the window-relative centre is an interior pixel of {geometry:?}, got ({x}, {y})"
    );

    // The model maps onto the last pixel (`round(n * (dim - 1))`), so the recorded point
    // is the naive `n * dim` product up to one pixel — but no further. A transform using
    // the wrong rect (an output size, a swapped axis) is off by far more.
    let naive = (
        CENTRE_X * f64::from(geometry.w),
        CENTRE_Y * f64::from(geometry.h),
    );
    assert!(
        (expected.0 - naive.0).abs() <= COORD_EPSILON + NAIVE_MAPPING_TOLERANCE
            && (expected.1 - naive.1).abs() <= COORD_EPSILON + NAIVE_MAPPING_TOLERANCE,
        "the model's point {expected:?} must be the naive centre {naive:?} within a pixel"
    );

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pointer_button_press_and_release_are_two_ordered_events() -> Result<()> {
    let (runtime, wayland) = start_runtime().await?;
    let (_window, _window_id) = map_toplevel(&runtime, &wayland).await?;
    let handle = runtime.compositor();

    // Buttons are delivered at the current pointer location and only to a focused
    // surface: the move is what gives the injection somewhere to land.
    focus_pointer(handle, &wayland, CENTRE).await?;
    wayland.clear_input_events();

    press_button(handle, Button::Left, ButtonState::Pressed)
        .await
        .expect("the press is accepted");
    wayland.wait_for_pointer_button(BTN_LEFT, RecordedButtonState::Pressed, DEADLINE)?;

    press_button(handle, Button::Left, ButtonState::Released)
        .await
        .expect("the release is accepted");
    wayland.wait_for_pointer_button(BTN_LEFT, RecordedButtonState::Released, DEADLINE)?;

    // Two distinct events, in command order: the filtered history is exactly the injected
    // pair, so a press the runtime reported twice — or a release that overtook its press —
    // fails here.
    assert_eq!(
        button_events(&wayland),
        vec![
            (BTN_LEFT, RecordedButtonState::Pressed),
            (BTN_LEFT, RecordedButtonState::Released),
        ],
        "the whole pointer history was {:?}",
        wayland.pointer_events()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pointer_axis_is_negative_vertical_and_framed() -> Result<()> {
    let (runtime, wayland) = start_runtime().await?;
    let (_window, _window_id) = map_toplevel(&runtime, &wayland).await?;
    let handle = runtime.compositor();

    // An axis event shares the button rule: it needs a focused surface, established by a
    // prior move.
    focus_pointer(handle, &wayland, CENTRE).await?;
    wayland.clear_input_events();

    // Negative `dy` is "scroll up"; the sign must survive the whole seat path unchanged.
    let dy = -3.0;
    scroll(handle, 0.0, dy)
        .await
        .expect("the scroll is accepted");
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.axis with a negative vertical delta",
        |event| {
            matches!(
                event,
                PointerEvent::Axis {
                    axis: AxisKind::Vertical,
                    value
                } if *value < 0.0
            )
        },
    )?;

    let events = wayland.pointer_events();
    let axis_at = first_index(&events, |event| {
        matches!(
            event,
            PointerEvent::Axis {
                axis: AxisKind::Vertical,
                ..
            }
        )
    })
    .expect("the wait above proved an axis event was recorded");
    let axis_event = events
        .get(axis_at)
        .expect("the index came from this snapshot");
    let PointerEvent::Axis { axis, value } = axis_event else {
        panic!("the located event must be an axis, got {axis_event:?}");
    };
    assert_eq!(*axis, AxisKind::Vertical);
    assert!(
        *value < 0.0,
        "the injected negative delta must stay negative, got {value} ({events:?})"
    );
    assert!(
        (*value - dy).abs() <= AXIS_EPSILON,
        "the axis value is the injected delta quantized to wl_fixed: {value} vs {dy}"
    );
    // A zero delta on the other axis is omitted, never reported as 0.0.
    assert!(
        !events.iter().any(|event| matches!(
            event,
            PointerEvent::Axis {
                axis: AxisKind::Horizontal,
                ..
            }
        )),
        "a zero horizontal delta must not be reported: {events:?}"
    );

    // Since v5 the axis belongs to a frame, and the frame terminates it: the client sees
    // the complete group, not an axis that is silently the last thing of a batch.
    let frame_at = first_index(&events[axis_at + 1..], |event| {
        matches!(event, PointerEvent::Frame)
    })
    .map(|offset| offset + axis_at + 1);
    let frame_at = frame_at.unwrap_or_else(|| {
        panic!("no wl_pointer.frame followed the axis event at {axis_at}: {events:?}")
    });
    assert!(axis_at < frame_at);

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_chord_is_a_press_in_order_and_a_reverse_release() -> Result<()> {
    let (runtime, wayland) = start_runtime().await?;
    let (_window, window_id) = map_toplevel(&runtime, &wayland).await?;
    let handle = runtime.compositor();

    // The keyboard needs a focus target and a client that holds a keyboard object: both
    // are awaited (never assumed) before the chord is injected.
    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.enter on the mapped toplevel",
        |event| matches!(event, KeyboardEvent::Enter { .. }),
    )?;
    assert_eq!(
        query_state(handle).await.keyboard_focus,
        Some(window_id),
        "the chord is typed at the focused toplevel"
    );
    wayland.clear_input_events();

    // Plan §3 writes this chord as `KeyCode::parse("ctrl+c")`, but `KeyCode::parse` is the
    // *single key* parser and chords are built with `KeyCode::parse_chord` (the server's
    // `keypress` path does exactly that: `adesk-server/src/dispatch/input.rs`). Assert the
    // difference rather than silently swapping the API, so the deviation stays visible.
    let as_written_in_the_plan = KeyCode::parse("ctrl+c");
    assert!(
        as_written_in_the_plan.is_err(),
        "plan §3's `KeyCode::parse(\"ctrl+c\")` is a single-key parser; if it now builds a \
         chord, this test should inject it directly: {as_written_in_the_plan:?}"
    );
    let chord =
        KeyCode::parse_chord(["CTRL", "C"]).expect("the chord names resolve through the aliases");
    assert!(chord.is_chord(), "two keys are a chord: {chord:?}");
    assert_eq!(chord.display_name(), "CTRL+C");

    key_event(handle, chord, KeyState::Pressed)
        .await
        .expect("a pressed chord is accepted");

    // Every key of the tap is delivered: the bounded waits below are the evidence that the
    // order asserted afterwards really reached this client.
    for (keycode, state) in CHORD_SEQUENCE {
        wayland.wait_for_key(keycode, state, DEADLINE)?;
    }

    // Press in order, release in reverse (`docs/architecture.md` §8): the modifier brackets
    // the tapped key, so a client never sees `c` up before the key it modified. Filtering
    // the history to `wl_keyboard.key` asserts the *chord key sequence* — the compositor's
    // own `modifiers`/`repeat_info` events, if it sends them, are interleaved between these
    // keys and are reported as a diagnostic below.
    assert_eq!(
        key_events(&wayland),
        CHORD_SEQUENCE.to_vec(),
        "the whole keyboard history was {:?}",
        wayland.keyboard_events()
    );

    // Diagnostics, not requirements: the runtime decides whether to send `modifiers` and
    // `repeat_info`, so this records what was actually delivered next to the chord.
    let keyboard_events = wayland.keyboard_events();
    let modifiers = keyboard_events
        .iter()
        .filter(|event| matches!(event, KeyboardEvent::Modifiers(_)))
        .count();
    let repeat_info = keyboard_events
        .iter()
        .filter(|event| matches!(event, KeyboardEvent::RepeatInfo { .. }))
        .count();
    eprintln!(
        "observed keyboard events around the chord: {} total, {modifiers} modifiers, \
         {repeat_info} repeat_info, last modifiers {:?}",
        keyboard_events.len(),
        wayland.last_modifiers()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_released_chord_is_rejected_and_delivers_nothing() -> Result<()> {
    let (runtime, wayland) = start_runtime().await?;
    let (_window, _window_id) = map_toplevel(&runtime, &wayland).await?;
    let handle = runtime.compositor();

    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.enter on the mapped toplevel",
        |event| matches!(event, KeyboardEvent::Enter { .. }),
    )?;
    wayland.clear_input_events();

    // A chord is a tap: it has no single key to release, so the request is rejected before
    // any key reaches the seat.
    let chord = KeyCode::parse_chord(["CTRL", "C"]).expect("the chord names resolve");
    let error = key_event(handle, chord, KeyState::Released)
        .await
        .expect_err("a released chord is rejected");
    assert_eq!(
        error.code,
        ErrorCode::InvalidRequest,
        "a released chord is an invalid request, got {error:?}"
    );

    // Bounded "sends nothing": the next real key is delivered, and it is the *only* key in
    // the history — a rejected chord that had leaked any key event would have been written
    // to this client's socket before it. Nothing here waits for an absence.
    key_event(
        handle,
        KeyCode::parse("c").expect("`c` resolves"),
        KeyState::Pressed,
    )
    .await
    .expect("a single key is accepted");
    wayland.wait_for_key(KEY_C, RecordedKeyState::Pressed, DEADLINE)?;
    assert_eq!(
        key_events(&wayland),
        vec![(KEY_C, RecordedKeyState::Pressed)],
        "the rejected chord delivered no key before the accepted one: {:?}",
        wayland.keyboard_events()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injection_without_a_focused_window_fails_without_panicking() -> Result<()> {
    // Deterministic setup: a runtime with no client and no toplevel, so there is no window
    // to focus and every `State::inject_*` takes its focus check
    // (`CompositorError::InvalidRequest("no window has keyboard focus")`). With zero
    // windows `unknown_window` is unreachable on this path, so `invalid_request` is the
    // code the implementation answers — asserted as such.
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let handle = runtime.compositor();

    let before = query_state(handle).await;
    assert!(
        before.is_empty(),
        "no client mapped a window in this runtime: {before:?}"
    );
    assert_eq!(before.active_window_id, None);
    assert_eq!(before.keyboard_focus, None);

    let rejections = [
        (
            "pointer_move",
            move_pointer(handle, CENTRE)
                .await
                .expect_err("no window to point at"),
        ),
        (
            "pointer_button",
            press_button(handle, Button::Left, ButtonState::Pressed)
                .await
                .expect_err("no window to click"),
        ),
        (
            "pointer_axis",
            scroll(handle, 0.0, -3.0)
                .await
                .expect_err("no window to scroll"),
        ),
        (
            "key_event",
            key_event(
                handle,
                KeyCode::parse("Escape").expect("Escape resolves"),
                KeyState::Pressed,
            )
            .await
            .expect_err("no window to type at"),
        ),
    ];
    for (method, error) in rejections {
        assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "{method} without a focused window: {error:?}"
        );
    }

    // The rejections were replies, not panics: the thread still serves commands and the
    // window state was never touched.
    let after = query_state(handle).await;
    assert_eq!(after.active_window_id, None);
    assert_eq!(after.keyboard_focus, None);
    assert!(
        after.ts_ms >= before.ts_ms,
        "the compositor is still running: {before:?} → {after:?}"
    );

    runtime.shutdown().await
}
