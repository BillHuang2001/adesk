//! Input events recorded from the seat's real protocol objects.
//!
//! The test client binds `wl_seat` and keeps a `wl_pointer` + `wl_keyboard` alive, so a
//! test can prove that an AGP action really travelled through the Wayland seat instead of
//! trusting the runtime's own report: [`WaylandTestClient`](super::WaylandTestClient)
//! records every event the compositor sends to those objects, in delivery order, and
//! exposes the history plus deadline-bounded waiters over it.
//!
//! ## Where recording happens
//!
//! Appending to the history happens inside the reader thread's `Dispatch` bodies
//! (`super::state`), which already hold the `ClientState` mutex, so:
//!
//! - a test never has to call a pump for an event to be *recorded* — the history is
//!   complete even if the test only inspects it later;
//! - recording is a `Vec` push under a lock the reader already holds: no I/O, no
//!   blocking, no lock ordering to get wrong (`ClientState` only, never a window slot);
//! - the history is append-only until
//!   [`clear_input_events`](super::WaylandTestClient::clear_input_events), so a waiter
//!   scans the whole history and an event delivered *before* the wait call still
//!   satisfies it.
//!
//! ## Recording rules
//!
//! - **Delivery order, no transformation.** Coordinates are stored exactly as the pinned
//!   bindings decoded them (fixed-point scaled to `f64`), keycodes and button codes stay
//!   the raw evdev values the protocol carries.
//! - **Unknown enum values are skipped, never guessed.** The protocol enums behind
//!   `wl_pointer.button.state`, `wl_pointer.axis` and `wl_keyboard.key.state` arrive as
//!   [`WEnum`], so a numeric code the pinned bindings do not know is dropped rather than
//!   mapped onto a wrong variant. Unknown *events* fall through the `#[non_exhaustive]`
//!   match arms and are ignored.
//! - **The keymap file descriptor is closed immediately.** `wl_keyboard.keymap` carries an
//!   `OwnedFd` for a server-side keymap file; the client records `format`/`size` and drops
//!   the fd in the dispatch body — it is never memory-mapped, stored or leaked.
//! - **Serials are plumbing, not history.** `ClientState::latest_input_serial` keeps the
//!   serial of the most recent `wl_pointer.enter`, `wl_pointer.button` and
//!   `wl_keyboard.enter` event — the three events a later clipboard pass can answer with
//!   (`wl_data_source.set_selection` needs a serial a real input event carried). No public
//!   API exposes it yet, and `clear_input_events` deliberately does *not* clear it: a
//!   serial belongs to the seat's input stream, not to the recorded history.

use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat};
use wayland_client::WEnum;

/// Which `wl_pointer.axis` scroll axis a [`PointerEvent::Axis`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    /// `wl_pointer.axis.vertical_scroll` (the usual wheel axis).
    Vertical,
    /// `wl_pointer.axis.horizontal_scroll`.
    Horizontal,
}

/// Physical state of a pointer button (`wl_pointer.button.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    /// The button is physically down.
    Pressed,
    /// The button is physically up.
    Released,
}

/// Physical state of a key (`wl_keyboard.key.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// The key is physically down.
    Pressed,
    /// The key is physically up.
    Released,
}

/// The modifier state a `wl_keyboard.modifiers` event reported.
///
/// The four fields are the raw xkbcommon masks the protocol carries (`depressed`,
/// `latched`, `locked` and the effective layout `group`); the client does not interpret
/// them, so a test compares exactly what the compositor sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModifiersState {
    /// Depressed modifier mask.
    pub depressed: u32,
    /// Latched modifier mask.
    pub latched: u32,
    /// Locked modifier mask.
    pub locked: u32,
    /// Effective keyboard layout index.
    pub group: u32,
}

/// One `wl_pointer` event, in delivery order.
///
/// Coordinates are *surface-local* (`wl_pointer.enter`/`motion` are relative to the surface
/// the pointer is on), so a test can compare them against the position it injected without
/// knowing where the compositor places the window.
#[derive(Debug, Clone, PartialEq)]
pub enum PointerEvent {
    /// `wl_pointer.enter`: the pointer entered `surface` at the surface-local `x`/`y`.
    Enter {
        /// The `wl_surface` the pointer entered.
        surface: ObjectId,
        /// Surface-local pointer position on entry (pixels).
        x: f64,
        /// Surface-local pointer position on entry (pixels).
        y: f64,
    },
    /// `wl_pointer.leave`: the pointer left `surface`.
    Leave {
        /// The `wl_surface` the pointer left.
        surface: ObjectId,
    },
    /// `wl_pointer.motion`: pointer movement on the surface the pointer is on.
    Motion {
        /// Surface-local pointer position (pixels).
        x: f64,
        /// Surface-local pointer position (pixels).
        y: f64,
    },
    /// `wl_pointer.button`: a button changed state.
    Button {
        /// The raw evdev button code (e.g. [`BTN_LEFT`]).
        button: u32,
        /// The physical state of the button.
        state: ButtonState,
    },
    /// `wl_pointer.axis`: a scroll axis moved.
    Axis {
        /// Which axis moved.
        axis: AxisKind,
        /// The scroll distance in surface-local coordinate space.
        value: f64,
    },
    /// `wl_pointer.frame` (since v5): the end of a group of pointer events.
    Frame,
}

/// One `wl_keyboard` event, in delivery order.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardEvent {
    /// `wl_keyboard.keymap`: the format code and size of the keymap the compositor sent.
    ///
    /// The accompanying file descriptor is *not* recorded: the dispatch body drops it
    /// immediately (see the module docs), so a test observes that a keymap arrived and how
    /// big it was without the client ever memory-mapping it.
    Keymap {
        /// The numeric `wl_keyboard.keymap_format` code (`1` = `xkb_v1`).
        format: u32,
        /// Keymap size in bytes.
        size: u32,
    },
    /// `wl_keyboard.enter`: the keyboard focused `surface`.
    Enter {
        /// The `wl_surface` that gained keyboard focus.
        surface: ObjectId,
        /// Keycodes logically down at that moment (decoded from the native-endian `u32`
        /// array the protocol carries; a trailing partial word is dropped).
        keys: Vec<u32>,
    },
    /// `wl_keyboard.leave`: the keyboard left `surface`.
    Leave {
        /// The `wl_surface` that lost keyboard focus.
        surface: ObjectId,
    },
    /// `wl_keyboard.key`: a key changed state.
    Key {
        /// The raw evdev keycode (e.g. [`KEY_LEFTCTRL`]).
        keycode: u32,
        /// The physical state of the key.
        state: KeyState,
    },
    /// `wl_keyboard.modifiers`: the modifier state changed.
    Modifiers(ModifiersState),
    /// `wl_keyboard.repeat_info` (since v4): key repeat rate and delay.
    RepeatInfo {
        /// Repeat rate in characters per second.
        rate: i32,
        /// Delay in milliseconds between key down and the first repeat.
        delay: i32,
    },
}

/// `BTN_LEFT`: the evdev code of the primary mouse button.
pub const BTN_LEFT: u32 = 0x110;

/// `KEY_LEFTCTRL`: the evdev code of the left control key.
pub const KEY_LEFTCTRL: u32 = 29;

/// `KEY_C`: the evdev code of the `c` key (the classic copy shortcut).
pub const KEY_C: u32 = 46;

/// Bit for `wl_seat.capability.pointer`.
///
/// `wl_seat::Capability` is a `bitflags` type, so the bit is taken through its `const fn
/// bits()` (`u32::from` it is not a `const fn`, and a cast does not exist).
pub(crate) const POINTER_CAPABILITY: u32 = wl_seat::Capability::Pointer.bits();

/// Bit for `wl_seat.capability.keyboard`.
pub(crate) const KEYBOARD_CAPABILITY: u32 = wl_seat::Capability::Keyboard.bits();

/// The raw bitfield of a `wl_seat.capabilities` event.
///
/// `capability` is a *bitfield* enum, so `WEnum<Capability>` only carries a `Value` when
/// exactly one bit is set: a seat with a pointer *and* a keyboard arrives as
/// `WEnum::Unknown(0b11)`. The raw bits are therefore the only ground truth, and reading
/// them cannot mistake "several capabilities at once" for "no capability at all".
pub(crate) fn capability_bits(capabilities: WEnum<wl_seat::Capability>) -> u32 {
    match capabilities {
        WEnum::Value(capability) => u32::from(capability),
        WEnum::Unknown(raw) => raw,
    }
}

/// The [`AxisKind`] of a `wl_pointer.axis` axis code, or `None` if it is unknown.
pub(crate) fn axis_kind(axis: WEnum<wl_pointer::Axis>) -> Option<AxisKind> {
    match axis {
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(AxisKind::Vertical),
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(AxisKind::Horizontal),
        // A newer axis code (`WEnum::Unknown`) is skipped rather than mapped onto the
        // wrong axis, which would make a scroll assertion silently lie.
        _ => None,
    }
}

/// The [`ButtonState`] of a `wl_pointer.button` state code, or `None` if it is unknown.
pub(crate) fn button_state(state: WEnum<wl_pointer::ButtonState>) -> Option<ButtonState> {
    match state {
        WEnum::Value(wl_pointer::ButtonState::Pressed) => Some(ButtonState::Pressed),
        WEnum::Value(wl_pointer::ButtonState::Released) => Some(ButtonState::Released),
        _ => None,
    }
}

/// The [`KeyState`] of a `wl_keyboard.key` state code, or `None` if it is unknown.
pub(crate) fn key_state(state: WEnum<wl_keyboard::KeyState>) -> Option<KeyState> {
    match state {
        WEnum::Value(wl_keyboard::KeyState::Pressed) => Some(KeyState::Pressed),
        WEnum::Value(wl_keyboard::KeyState::Released) => Some(KeyState::Released),
        _ => None,
    }
}

/// The numeric `wl_keyboard.keymap.format` code (the recorded contract pins a raw `u32`).
pub(crate) fn keymap_format(format: WEnum<wl_keyboard::KeymapFormat>) -> u32 {
    match format {
        WEnum::Value(format) => u32::from(format),
        WEnum::Unknown(raw) => raw,
    }
}

/// Decodes a `wl_keyboard.enter` `keys` array into keycodes.
///
/// The protocol sends an array of native-endian `u32` codes as raw bytes. `chunks_exact(4)`
/// drops any trailing partial word instead of panicking; a spec-compliant server never
/// sends one, but a dispatch body must not be able to panic on malformed input.
pub(crate) fn decode_keys(keys: &[u8]) -> Vec<u32> {
    keys.chunks_exact(4)
        .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use adesk_core::{AppId, Position, WindowId};
    // `Proxy` is what provides `id()`, which the tests use to compare a recorded event's
    // surface against the window's own `wl_surface`.
    use wayland_client::Proxy;

    use super::*;
    use crate::runtime::{TestRuntime, TestRuntimeConfig};
    use crate::wayland::{TestWindow, ToplevelSpec, WaylandTestClient};
    use crate::{FillPattern, Size, TestkitError};

    /// Bounded wait for the self-validation tests (far above the cost of one injection).
    const DEADLINE: Duration = Duration::from_secs(10);

    /// App id of the toplevel the self-test drives over the protocol path.
    const APP_ID: &str = "org.example.testkit.input";

    /// Starts a real runtime whose Wayland socket the client can reach.
    ///
    /// `apply_env(false)`: no app is launched, so the runtime releases the process-env lock
    /// as soon as the server is up ([`TestRuntime::wayland_client`] connects by absolute
    /// socket path). The current-thread tokio runtime is returned with it because the
    /// client's public input waiters are synchronous — the async stages (runtime, AGP
    /// client) run through `block_on` around them.
    fn start_runtime() -> (tokio::runtime::Runtime, TestRuntime) {
        let tokio_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("cannot build the tokio runtime for the input self-test");
        let runtime = tokio_rt
            .block_on(TestRuntime::start_with(
                TestRuntimeConfig::new().with_apply_env(false),
            ))
            .expect("the test runtime starts");
        (tokio_rt, runtime)
    }

    /// Maps a toplevel, acknowledges its configure and commits one frame (it is mapped).
    ///
    /// The trailing roundtrip flushes the client's requests and gives the reader a full
    /// cycle, so the seat objects (and the keymap the compositor sends with a fresh
    /// keyboard) are on the wire before a test injects any input. Returns the window handle
    /// together with the runtime's own id for it (read over AGP, never assumed).
    fn mapped_window(
        tokio_rt: &tokio::runtime::Runtime,
        runtime: &TestRuntime,
        wayland: &mut WaylandTestClient,
    ) -> (TestWindow, WindowId) {
        let window = wayland
            .create_toplevel(ToplevelSpec::new(APP_ID, "Input", Size::new(320, 200)))
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
        assert_eq!(
            info.geometry,
            runtime.tiled_rect(),
            "the single visible toplevel is tiled to the whole output"
        );
        (window, info.id)
    }

    #[test]
    fn capability_bits_reads_bitfields_that_are_not_a_single_value() {
        // A seat with both capabilities is *not* `Capability::Pointer`: the generated
        // binding only knows single values, so this is the normal `Unknown` case.
        assert_eq!(
            capability_bits(WEnum::Unknown(POINTER_CAPABILITY | KEYBOARD_CAPABILITY)),
            POINTER_CAPABILITY | KEYBOARD_CAPABILITY
        );
        assert_eq!(
            capability_bits(WEnum::Value(wl_seat::Capability::Pointer)),
            POINTER_CAPABILITY
        );
        assert_eq!(capability_bits(WEnum::Unknown(0)), 0);
    }

    #[test]
    fn unknown_enum_values_are_skipped_instead_of_mis_mapped() {
        assert_eq!(
            button_state(WEnum::Value(wl_pointer::ButtonState::Pressed)),
            Some(ButtonState::Pressed)
        );
        assert_eq!(button_state(WEnum::Unknown(7)), None);
        assert_eq!(
            key_state(WEnum::Value(wl_keyboard::KeyState::Released)),
            Some(KeyState::Released)
        );
        assert_eq!(key_state(WEnum::Unknown(7)), None);
        assert_eq!(
            axis_kind(WEnum::Value(wl_pointer::Axis::HorizontalScroll)),
            Some(AxisKind::Horizontal)
        );
        assert_eq!(axis_kind(WEnum::Unknown(9)), None);
        assert_eq!(
            keymap_format(WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)),
            1
        );
        assert_eq!(keymap_format(WEnum::Unknown(9)), 9);
    }

    #[test]
    fn keys_are_decoded_native_endian_and_a_partial_word_is_dropped() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&KEY_LEFTCTRL.to_ne_bytes());
        bytes.extend_from_slice(&KEY_C.to_ne_bytes());
        assert_eq!(decode_keys(&bytes), vec![KEY_LEFTCTRL, KEY_C]);
        assert_eq!(decode_keys(&[]), Vec::<u32>::new());
        // A malformed trailing word must be dropped, not panic.
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(decode_keys(&bytes), vec![KEY_LEFTCTRL, KEY_C]);
    }

    /// Capstone self-validation: a real runtime, the real seat path, a real injection.
    ///
    /// The client binds `wl_seat`, so the compositor's `pointer_move` must arrive as
    /// `wl_pointer.enter` (the first move *enters* the surface: the protocol delivers the
    /// entry position with `enter`) and `wl_pointer.motion` (the second move), and the
    /// mapped toplevel must get keyboard focus (`wl_keyboard.keymap` when the keyboard is
    /// created, then `enter` + `modifiers`). Expected coordinates come from
    /// [`TestRuntime::tiled_rect`], never from a hard-coded output size.
    #[test]
    fn seat_events_from_agp_input_are_recorded() {
        let (tokio_rt, runtime) = start_runtime();
        let mut wayland = runtime.wayland_client().expect("the client connects");
        let (window, window_id) = mapped_window(&tokio_rt, &runtime, &mut wayland);
        // Two moves: the first produces `enter` at the injected position, the second a
        // genuine `motion`. Both positions are derived from the tiled geometry.
        let tiled = runtime.tiled_rect();
        let entry = (tiled.w / 4, tiled.h / 3);
        let moved = (tiled.w / 2, tiled.h / 2);
        let client = tokio_rt.block_on(runtime.client()).expect("AGP connects");
        tokio_rt
            .block_on(
                client.pointer_move(window_id, Position::pixels(entry.0 as i32, entry.1 as i32)),
            )
            .expect("the first pointer move is injected");

        let surface = window.surface().id();
        wayland
            .wait_for_pointer_event(DEADLINE, "pointer enter on the mapped surface", |event| {
                matches!(
                    event,
                    PointerEvent::Enter { surface: entered, x, y }
                        if *entered == surface
                            && *x == f64::from(entry.0)
                            && *y == f64::from(entry.1)
                )
            })
            .expect("the seat delivered wl_pointer.enter at the injected position");
        assert!(
            wayland.pointer_events().iter().any(|event| matches!(
                event,
                PointerEvent::Enter { surface: entered, .. } if *entered == surface
            )),
            "the history holds an enter for the toplevel surface, got {:?}",
            wayland.pointer_events()
        );

        tokio_rt
            .block_on(
                client.pointer_move(window_id, Position::pixels(moved.0 as i32, moved.1 as i32)),
            )
            .expect("the second pointer move is injected");
        wayland
            .wait_for_pointer_event(DEADLINE, "pointer motion on the mapped surface", |event| {
                matches!(event, PointerEvent::Motion { x, y }
                    if *x == f64::from(moved.0) && *y == f64::from(moved.1))
            })
            .expect("the seat delivered wl_pointer.motion with the injected coordinates");

        // Keyboard capability wiring: the compositor sends the keymap when the client
        // creates the keyboard, and `enter` + `modifiers` when the toplevel takes focus.
        wayland
            .wait_for_keyboard_event(
                DEADLINE,
                "keyboard keymap",
                |event| matches!(event, KeyboardEvent::Keymap { size, .. } if *size > 0),
            )
            .expect("the compositor sent the keymap to the keyboard object");
        wayland
            .wait_for_keyboard_event(DEADLINE, "keyboard enter on the mapped surface", |event| {
                matches!(event, KeyboardEvent::Enter { surface: entered, .. } if *entered == surface)
            })
            .expect("the seat delivered wl_keyboard.enter for the mapped toplevel");
        wayland
            .wait_for_keyboard_event(DEADLINE, "keyboard modifiers", |event| {
                matches!(event, KeyboardEvent::Modifiers(_))
            })
            .expect("the compositor sent the modifier state after entering the surface");
        assert!(
            wayland.last_modifiers().is_some(),
            "last_modifiers reports the recorded modifier state"
        );

        // The recorded serial is the one a later request may answer with: an injecting
        // sequence produced real input events, so at least one serial-carrying event
        // (`wl_pointer.enter`, `wl_pointer.button` or `wl_keyboard.enter`) arrived.
        let entered_serial = crate::wayland::state::lock_client(&wayland.state)
            .latest_input_serial()
            .expect("an entering input event carried a serial");

        // The history is append-only until it is cleared explicitly.
        assert!(!wayland.keyboard_events().is_empty());
        wayland.clear_input_events();
        assert!(wayland.pointer_events().is_empty());
        assert!(wayland.keyboard_events().is_empty());
        assert_eq!(
            wayland.last_modifiers(),
            None,
            "clearing the history clears the recorded modifier state with it"
        );
        assert_eq!(
            crate::wayland::state::lock_client(&wayland.state).latest_input_serial(),
            Some(entered_serial),
            "clearing the history keeps the latest input serial: it belongs to the seat's \
             input stream, not to the recorded history"
        );

        tokio_rt
            .block_on(runtime.shutdown())
            .expect("the runtime shuts down");
    }

    /// A wait for an event that never arrives fails with the canonical bounded error.
    #[test]
    fn a_wait_that_cannot_be_satisfied_times_out() {
        let (tokio_rt, runtime) = start_runtime();
        let wayland = runtime.wayland_client().expect("the client connects");

        let timeout = Duration::from_millis(50);
        let error = wayland
            .wait_for_pointer_event(timeout, "a pointer event that never comes", |_| false)
            .expect_err("no input was injected, so the wait must time out");
        match error {
            TestkitError::Timeout {
                what,
                timeout: reported,
            } => {
                assert_eq!(what, "a pointer event that never comes");
                assert_eq!(reported, timeout);
            }
            other => panic!("expected a Timeout error, got {other:?}"),
        }

        tokio_rt
            .block_on(runtime.shutdown())
            .expect("the runtime shuts down");
    }
}
