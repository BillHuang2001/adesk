//! Real-seat input capture: AGP actions proven to arrive as `wl_pointer` / `wl_keyboard`
//! events on the protocol path.
//!
//! `WaylandTestClient` binds `wl_seat` and keeps a `wl_pointer` + `wl_keyboard` alive, so
//! every AGP input action can be checked against the *seat's own* report instead of the
//! runtime's word: the client records each delivered event, in delivery order, with
//! coordinates exactly as the protocol carried them (the harness's `wayland::input` recorder's
//! contract — raw evdev button/keycodes, surface-local `f64` coordinates, no
//! transformation). These tests drive input **only** through the AGP client
//! ([`TestRuntime::client`]); the harness client is the observer, never the injector.
//!
//! # What each test proves
//!
//! | Test | Evidence |
//! |---|---|
//! | `pointer_motion_surface_coordinates_match_injection` | `pointer_move` → `wl_pointer.enter`/`motion` on the mapped surface, coordinates derived from the tiled geometry, in delivery order |
//! | `pointer_button_press_and_release_are_distinct_and_ordered` | `mouse_down`/`mouse_up` → exactly `[(BTN_LEFT, Pressed), (BTN_LEFT, Released)]` |
//! | `pointer_axis_reports_vertical_negative_and_is_followed_by_frame` | `scroll` with `dy < 0` → `wl_pointer.axis(Vertical, < 0)` then `wl_pointer.frame` |
//! | `ctrl_c_chord_arrives_as_ordered_press_and_reverse_release` | `keypress(["CTRL", "C"])` → press in order, release in reverse, plus keymap and focus entry |
//! | `keyboard_enter_and_pointer_enter_follow_the_mapped_window` | focus (and its leave/enter transition) follows the mapped toplevel |
//!
//! # The focus prerequisite
//!
//! The compositor delivers pointer buttons and axes at the *current pointer location*, and
//! Smithay only sends them to a surface that has pointer focus. A button or axis injection
//! without a preceding `pointer_move` therefore reaches no client at all, so every test
//! here moves the pointer first — that is what makes the later assertion about *what the
//! client received* meaningful rather than vacuous.
//!
//! # Coordinate expectations
//!
//! Window-relative normalized coordinates are converted by the window model
//! (`adesk_core::Position::resolve`, which `adesk_wm::WindowManager::resolve_position`
//! delegates to) into `round(n * (dim - 1))` — the first and last *pixel* of the tiled
//! window, not `n * dim`. The tests derive the expected point with that same conversion
//! from [`TestRuntime::tiled_rect`], never from a hard-coded output size, and compare it
//! with [`COORD_EPSILON`] (tighter than the 0.5 px band a caller may assume, and far above
//! the 1/256 px `wl_fixed` encoding step the protocol uses). The naive product `n * dim`
//! is checked separately against a documented ≤ 1 px difference, so a transform that is
//! wrong by more than the model's own rounding still fails.
//!
//! # Execution model
//!
//! The recorders and their waiters are synchronous (they poll the reader thread's history
//! under a mutex; the harness's own rule is that they must not run on an executor's only
//! thread), so the tests run on a small multi-thread executor: a bounded wait can never
//! stall the runtime's own tasks. Every wait is deadline-bounded by [`DEADLINE`] — a lost
//! event fails the test with `TestkitError::Timeout`, it never hangs.
//!
//! The runtime does not need the process environment after startup (no application is
//! launched here), so the tests run without an env-scoped runtime lock and stay parallel
//! within this binary.
//!
//! # Diagnostics that are observations, not requirements
//!
//! Whether the compositor sends `wl_keyboard.modifiers` / `wl_keyboard.repeat_info` is the
//! runtime's decision, so the chord test *reports* what was observed (see its diagnostics)
//! and asserts only the harness invariant: `last_modifiers()` is `Some` exactly when a
//! `Modifiers` event was recorded.

use std::time::Duration;

use adesk_client::{KeyChord, PointerButtonRequest, ScrollRequest};
use adesk_core::Position;
use adesk_testkit::{
    AppId, AxisKind, ButtonState, EventAssert, Expected, FillPattern, KeyState, KeyboardEvent,
    PointerEvent, Rect, Result, Size, TestRuntime, TestRuntimeConfig, TestWindow, ToplevelSpec,
    WaylandTestClient, WindowId, BTN_LEFT, KEY_C, KEY_LEFTCTRL,
};
// `Proxy` is what provides `WlSurface::id()`, the surface identity an event is compared
// against; the client-side `ObjectId` is stable for the life of the connection.
use wayland_client::Proxy;

/// Every bounded wait and deadline in this file.
const DEADLINE: Duration = Duration::from_secs(10);

/// Tolerance when comparing an injected position with a recorded surface-local
/// coordinate, in pixels.
///
/// Tighter than the 0.5 px a caller may assume: the delivered value is the window model's
/// integer point and the protocol's `wl_fixed` step is 1/256 px, so anything above that
/// step but far below a mis-resolved coordinate is enough to catch a real failure.
const COORD_EPSILON: f64 = 0.25;

/// Tolerance for a recorded `wl_pointer.axis` value: the protocol quantizes it to
/// `wl_fixed` (1/256 px).
const AXIS_EPSILON: f64 = 1.0 / 256.0;

/// The extra tolerance allowed when comparing a recorded coordinate with the naive
/// `normalized * dimension` product.
///
/// The two can legitimately differ by up to one pixel: the window model maps a normalized
/// value onto the *last* pixel (`round(n * (dim - 1))`), e.g. `0.75 * 1280 == 960` while
/// the model resolves `959`.
const NAIVE_MAPPING_TOLERANCE: f64 = 1.0;

/// App id of the toplevel `pointer_motion_surface_coordinates_match_injection` maps.
const MOTION_APP_ID: &str = "org.example.input.motion";
/// App id of the toplevel `pointer_button_press_and_release_are_distinct_and_ordered` maps.
const BUTTON_APP_ID: &str = "org.example.input.button";
/// App id of the toplevel `pointer_axis_reports_vertical_negative_and_is_followed_by_frame` maps.
const AXIS_APP_ID: &str = "org.example.input.axis";
/// App id of the toplevel `ctrl_c_chord_arrives_as_ordered_press_and_reverse_release` maps.
const CHORD_APP_ID: &str = "org.example.input.chord";
/// App id of the first toplevel `keyboard_enter_and_pointer_enter_follow_the_mapped_window` maps.
const FOCUS_APP_ID: &str = "org.example.input.focus";
/// App id of the second toplevel that test maps to move the seat's focus.
const FOCUS_SECOND_APP_ID: &str = "org.example.input.focus.second";

/// A runtime whose Wayland socket the protocol-path client can connect to.
///
/// `with_apply_env(false)`: nothing is launched, so the runtime scopes the process env only
/// across startup and releases the env lock as soon as the server (and the compositor's
/// socket inside the runtime's own temp dir) is up. The Wayland client connects by
/// absolute socket path, so the five tests here stay parallel.
fn wayland_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// Maps a toplevel on the protocol path and returns the runtime's own id for it.
///
/// The window comes back mapped, tiled and focused — the runtime's `window_created` /
/// `window_activated` events (awaited here, never assumed) say so, and the
/// `wl_keyboard.keymap` wait is the seat-readiness barrier: the client creates its
/// `wl_pointer` and `wl_keyboard` from the same `wl_seat.capabilities` event, so once the
/// keymap has been delivered, an injection has seat objects to reach.
async fn map_toplevel(
    runtime: &TestRuntime,
    wayland: &WaylandTestClient,
    app_id: &str,
    title: &str,
) -> Result<(TestWindow, WindowId)> {
    // The event broadcast does not replay: tap before the first commit, which is what maps
    // the surface.
    let mut events = EventAssert::tap(runtime);
    let window = wayland.create_toplevel(ToplevelSpec::new(app_id, title, Size::new(320, 200)))?;
    let configure = window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy configures the toplevel, not the size the client asked for"
    );
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(app_id)), DEADLINE)
        .await?;
    let window_id = created
        .window_id()
        .expect("window_created carries a window id");

    // A mapped toplevel takes the visible slot and the keyboard focus; injection before
    // this point would have no focus target.
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

/// The surface-local point the runtime delivers for the window-relative normalized
/// position `(nx, ny)` on a toplevel occupying `rect`.
///
/// This is the window model's own conversion, so the expectation is derived from the
/// runtime's tiled geometry instead of a hard-coded output size.
fn normalized_point(rect: Rect, nx: f64, ny: f64) -> (f64, f64) {
    let point = Position::normalized(nx, ny).resolve(rect);
    (f64::from(point.x), f64::from(point.y))
}

/// Whether a recorded coordinate matches an injected one within [`COORD_EPSILON`].
fn within_epsilon(recorded: f64, expected: f64) -> bool {
    (recorded - expected).abs() <= COORD_EPSILON
}

/// Index of the first recorded event satisfying `predicate`, in delivery order.
fn first_index<T>(events: &[T], predicate: impl Fn(&T) -> bool) -> Option<usize> {
    events.iter().position(predicate)
}

/// The every documented form of a recorded pointer `Button` event, filtered in order.
fn button_events(wayland: &WaylandTestClient) -> Vec<(u32, ButtonState)> {
    wayland
        .pointer_events()
        .iter()
        .filter_map(|event| match event {
            PointerEvent::Button { button, state } => Some((*button, *state)),
            _ => None,
        })
        .collect()
}

/// The recorded keyboard `Key` events, filtered in delivery order.
fn key_events(wayland: &WaylandTestClient) -> Vec<(u32, KeyState)> {
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
async fn pointer_motion_surface_coordinates_match_injection() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;
    let (window, window_id) = map_toplevel(&runtime, &wayland, MOTION_APP_ID, "Motion").await?;

    let surface = window.surface().id();
    let tiled = runtime.tiled_rect();

    // Three window-relative normalized positions. The first move *enters* the surface
    // (`wl_pointer.enter` carries the entry position), the second and third are genuine
    // `wl_pointer.motion` events.
    let entry = (0.25, 0.75);
    let moved = (0.75, 0.33);
    let third = (0.5, 0.5);
    let (entry_x, entry_y) = normalized_point(tiled, entry.0, entry.1);
    let (moved_x, moved_y) = normalized_point(tiled, moved.0, moved.1);
    let (third_x, third_y) = normalized_point(tiled, third.0, third.1);

    client
        .pointer_move(window_id, Position::normalized(entry.0, entry.1))
        .await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.enter at the injected position",
        |event| {
            matches!(
                event,
                PointerEvent::Enter { surface: entered, x, y }
                    if *entered == surface
                        && within_epsilon(*x, entry_x)
                        && within_epsilon(*y, entry_y)
            )
        },
    )?;

    client
        .pointer_move(window_id, Position::normalized(moved.0, moved.1))
        .await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.motion at the second position",
        |event| {
            matches!(
                event,
                PointerEvent::Motion { x, y }
                    if within_epsilon(*x, moved_x) && within_epsilon(*y, moved_y)
            )
        },
    )?;

    client
        .pointer_move(window_id, Position::normalized(third.0, third.1))
        .await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.motion at the third position",
        |event| {
            matches!(
                event,
                PointerEvent::Motion { x, y }
                    if within_epsilon(*x, third_x) && within_epsilon(*y, third_y)
            )
        },
    )?;

    // The moves arrived in delivery order: the history is append-only, so index order in
    // the snapshot *is* the order the compositor sent them.
    let events = wayland.pointer_events();
    let entered_at = first_index(
        &events,
        |event| matches!(event, PointerEvent::Enter { surface: entered, .. } if *entered == surface),
    );
    let moved_at = first_index(&events, |event| {
        matches!(event, PointerEvent::Motion { x, y }
            if within_epsilon(*x, moved_x) && within_epsilon(*y, moved_y))
    });
    let third_at = first_index(&events, |event| {
        matches!(event, PointerEvent::Motion { x, y }
            if within_epsilon(*x, third_x) && within_epsilon(*y, third_y))
    });
    let (Some(entered_at), Some(moved_at), Some(third_at)) = (entered_at, moved_at, third_at)
    else {
        panic!("every injected position is in the recorded history: {events:?}");
    };
    assert!(
        entered_at < moved_at && moved_at < third_at,
        "enter ({entered_at}) must precede the motions ({moved_at}, {third_at}): {events:?}"
    );

    // The recorded coordinates are the model's point, which is the naive
    // `normalized * dimension` product up to the model's ≤ 1 px rounding — a materially
    // wrong transform (output size instead of the window's, swapped axes) would not be.
    for ((nx, ny), (x, y), dimension) in [
        (entry, (entry_x, entry_y), tiled),
        (moved, (moved_x, moved_y), tiled),
        (third, (third_x, third_y), tiled),
    ] {
        let naive = (nx * f64::from(dimension.w), ny * f64::from(dimension.h));
        assert!(
            (x - naive.0).abs() <= COORD_EPSILON + NAIVE_MAPPING_TOLERANCE
                && (y - naive.1).abs() <= COORD_EPSILON + NAIVE_MAPPING_TOLERANCE,
            "recorded ({x}, {y}) must match the naive ({}, {}) within a pixel",
            naive.0,
            naive.1
        );
    }

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pointer_button_press_and_release_are_distinct_and_ordered() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;
    let (_window, window_id) = map_toplevel(&runtime, &wayland, BUTTON_APP_ID, "Buttons").await?;

    let position = Position::normalized(0.5, 0.5);
    // Buttons are delivered at the current pointer position, and Smithay only sends them
    // to a focused surface: the move is what gives the injection somewhere to land.
    client.pointer_move(window_id, position).await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "pointer focus before the button injection",
        |event| {
            matches!(
                event,
                PointerEvent::Enter { .. } | PointerEvent::Motion { .. }
            )
        },
    )?;

    // Scope the assertion to the injected action: the history is append-only, so a button
    // recorded before this point could otherwise satisfy the expectation below.
    wayland.clear_input_events();

    let down = client
        .mouse_down(PointerButtonRequest::window(window_id).position(position))
        .await?;
    assert!(down.0 > 0, "mouse_down returns an action id");
    wayland.wait_for_pointer_button(BTN_LEFT, ButtonState::Pressed, DEADLINE)?;

    let up = client
        .mouse_up(PointerButtonRequest::window(window_id).position(position))
        .await?;
    assert!(up.0 > 0, "mouse_up returns an action id");
    wayland.wait_for_pointer_button(BTN_LEFT, ButtonState::Released, DEADLINE)?;

    // The filtered history is exactly the injected pair, in that order — a press and a
    // release are distinct events, not one "click" the runtime reports twice.
    let recorded = button_events(&wayland);
    assert_eq!(
        recorded,
        vec![
            (BTN_LEFT, ButtonState::Pressed),
            (BTN_LEFT, ButtonState::Released)
        ],
        "the whole pointer history was {:?}",
        wayland.pointer_events()
    );

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pointer_axis_reports_vertical_negative_and_is_followed_by_frame() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;
    let (_window, window_id) = map_toplevel(&runtime, &wayland, AXIS_APP_ID, "Axis").await?;

    let position = Position::normalized(0.5, 0.5);
    // Axis events share the button rule: they are delivered at the current pointer
    // position on a focused surface.
    client.pointer_move(window_id, position).await?;
    wayland.wait_for_pointer_event(DEADLINE, "pointer focus before the scroll", |event| {
        matches!(
            event,
            PointerEvent::Enter { .. } | PointerEvent::Motion { .. }
        )
    })?;
    wayland.clear_input_events();

    // Negative `dy` is "scroll up": the sign must survive the whole seat path unchanged.
    let dy = -3.0;
    let action = client
        .scroll(ScrollRequest::new(window_id, 0.0, dy).position(position))
        .await?;
    assert!(action.0 > 0, "scroll returns an action id");

    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.axis with a negative vertical delta",
        |event| matches!(event, PointerEvent::Axis { axis: AxisKind::Vertical, value } if *value < 0.0),
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
    assert!(
        frame_at.is_some(),
        "no wl_pointer.frame followed the axis event at {axis_at}: {events:?}"
    );
    assert!(axis_at < frame_at.expect("checked above"));

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_chord_arrives_as_ordered_press_and_reverse_release() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;
    let (window, window_id) = map_toplevel(&runtime, &wayland, CHORD_APP_ID, "Chord").await?;

    let surface = window.surface().id();
    // Both are recorded while the toplevel maps: the keymap when the keyboard object is
    // created, the entry when the seat focuses the surface. Asserting them here is also
    // what guarantees the chord below cannot be sent to a seat the client does not hold.
    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.keymap with a nonzero size",
        |event| matches!(event, KeyboardEvent::Keymap { size, .. } if *size > 0),
    )?;
    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.enter for the mapped surface",
        |event| matches!(event, KeyboardEvent::Enter { surface: entered, .. } if *entered == surface),
    )?;

    // The AGP chord: the last element is tapped, the leading ones are its modifiers.
    let action = client
        .keypress(KeyChord::chord(["CTRL", "C"]), Some(window_id))
        .await?;
    assert!(action.0 > 0, "keypress returns an action id");

    for (keycode, state) in [
        (KEY_LEFTCTRL, KeyState::Pressed),
        (KEY_C, KeyState::Pressed),
        (KEY_C, KeyState::Released),
        (KEY_LEFTCTRL, KeyState::Released),
    ] {
        wayland.wait_for_key(keycode, state, DEADLINE)?;
    }

    // Press in order, release in reverse (protocol §3): the modifier brackets the tapped
    // key, so a client never sees `c` up before the key it modified.
    assert_eq!(
        key_events(&wayland),
        vec![
            (KEY_LEFTCTRL, KeyState::Pressed),
            (KEY_C, KeyState::Pressed),
            (KEY_C, KeyState::Released),
            (KEY_LEFTCTRL, KeyState::Released),
        ],
        "the whole keyboard history was {:?}",
        wayland.keyboard_events()
    );

    // Diagnostics, not requirements: `modifiers`/`repeat_info` are the runtime's decision,
    // so this test only pins the harness invariant — `last_modifiers()` is the last
    // recorded `Modifiers` event, and therefore `Some` exactly when one arrived.
    let keyboard_events = wayland.keyboard_events();
    let recorded_modifiers = keyboard_events
        .iter()
        .any(|event| matches!(event, KeyboardEvent::Modifiers(_)));
    assert_eq!(
        recorded_modifiers,
        wayland.last_modifiers().is_some(),
        "last_modifiers reports exactly the recorded modifier state: {keyboard_events:?}"
    );
    let modifiers = keyboard_events
        .iter()
        .filter(|event| matches!(event, KeyboardEvent::Modifiers(_)))
        .count();
    let repeat_info = keyboard_events
        .iter()
        .filter(|event| matches!(event, KeyboardEvent::RepeatInfo { .. }))
        .count();
    eprintln!(
        "observed keyboard events across the chord: {} total, {modifiers} modifiers, \
         {repeat_info} repeat_info, last modifiers {:?}",
        keyboard_events.len(),
        wayland.last_modifiers()
    );

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_enter_and_pointer_enter_follow_the_mapped_window() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;
    let (first, first_id) = map_toplevel(&runtime, &wayland, FOCUS_APP_ID, "First").await?;
    let first_surface = first.surface().id();

    // Keyboard enter: the seat focused the surface the compositor mapped, and nothing was
    // down at that moment (the `keys` array is the ground truth, not a promise).
    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.enter for the first mapped surface",
        |event| matches!(event, KeyboardEvent::Enter { surface: entered, .. } if *entered == first_surface),
    )?;
    let (entered_surface, entered_keys) = wayland
        .keyboard_events()
        .into_iter()
        .find_map(|event| match event {
            KeyboardEvent::Enter { surface, keys } => Some((surface, keys)),
            _ => None,
        })
        .expect("the wait above proved an enter was recorded");
    assert_eq!(entered_surface, first_surface);
    assert!(
        entered_keys.is_empty(),
        "no key was down when the surface was focused, got {entered_keys:?}"
    );

    // Pointer enter: moving the pointer into the tiled window enters *that* surface
    // (the full coordinate evidence is `pointer_motion_surface_coordinates_match_injection`).
    client
        .pointer_move(first_id, Position::normalized(0.5, 0.5))
        .await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.enter for the first mapped surface",
        |event| matches!(event, PointerEvent::Enter { surface: entered, .. } if *entered == first_surface),
    )?;

    // A second toplevel takes the visible slot and the focus. Both surfaces are alive on
    // this connection, so the seat's *transition* is observable: the seat reports the
    // surface it left before the one it entered.
    let (second, second_id) =
        map_toplevel(&runtime, &wayland, FOCUS_SECOND_APP_ID, "Second").await?;
    let second_surface = second.surface().id();
    assert_ne!(second_surface, first_surface);
    assert_ne!(second_id, first_id);

    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.leave for the previously focused surface",
        |event| matches!(event, KeyboardEvent::Leave { surface } if *surface == first_surface),
    )?;
    wayland.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.enter for the newly mapped surface",
        |event| matches!(event, KeyboardEvent::Enter { surface: entered, .. } if *entered == second_surface),
    )?;
    let keyboard_events = wayland.keyboard_events();
    let keyboard_leave_at = first_index(
        &keyboard_events,
        |event| matches!(event, KeyboardEvent::Leave { surface } if *surface == first_surface),
    );
    let keyboard_enter_at = first_index(
        &keyboard_events,
        |event| matches!(event, KeyboardEvent::Enter { surface: entered, .. } if *entered == second_surface),
    );
    let (Some(keyboard_leave_at), Some(keyboard_enter_at)) = (keyboard_leave_at, keyboard_enter_at)
    else {
        panic!("the waits above proved both events are recorded: {keyboard_events:?}");
    };
    assert!(
        keyboard_leave_at < keyboard_enter_at,
        "the keyboard left surface {first_surface:?} ({keyboard_leave_at}) before entering \
         {second_surface:?} ({keyboard_enter_at}): {keyboard_events:?}"
    );

    // The pointer follows the same rule: moving it into the now-active window leaves the
    // surface it was on first, so the transition is proven on both input paths.
    client
        .pointer_move(second_id, Position::normalized(0.5, 0.5))
        .await?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.leave for the previously entered surface",
        |event| matches!(event, PointerEvent::Leave { surface } if *surface == first_surface),
    )?;
    wayland.wait_for_pointer_event(
        DEADLINE,
        "wl_pointer.enter for the newly mapped surface",
        |event| matches!(event, PointerEvent::Enter { surface: entered, .. } if *entered == second_surface),
    )?;
    let pointer_events = wayland.pointer_events();
    let pointer_leave_at = first_index(
        &pointer_events,
        |event| matches!(event, PointerEvent::Leave { surface } if *surface == first_surface),
    );
    let pointer_enter_at = first_index(
        &pointer_events,
        |event| matches!(event, PointerEvent::Enter { surface: entered, .. } if *entered == second_surface),
    );
    let (Some(pointer_leave_at), Some(pointer_enter_at)) = (pointer_leave_at, pointer_enter_at)
    else {
        panic!("the waits above proved both events are recorded: {pointer_events:?}");
    };
    assert!(
        pointer_leave_at < pointer_enter_at,
        "the pointer left surface {first_surface:?} ({pointer_leave_at}) before entering \
         {second_surface:?} ({pointer_enter_at}): {pointer_events:?}"
    );

    // Diagnostics, not requirements (the assertions above are the evidence): both leaves
    // really were delivered by the seat, not merely waited for.
    eprintln!(
        "seat focus transition: keyboard leave {first_surface:?} at {keyboard_leave_at}, \
         enter {second_surface:?} at {keyboard_enter_at}; pointer leave at {pointer_leave_at}, \
         enter at {pointer_enter_at}"
    );

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}
