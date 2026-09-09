//! Seat wrappers: the keysym table, handle accessors and one-call injections.
//!
//! [`InputInjector`] owns the keyboard and pointer handles created by [`State`]
//! startup plus the [`KeymapTable`] used to turn keysyms into keycodes.
//!
//! The injection helpers are **associated functions** taking a cloned handle and
//! `&mut State`:
//!
//! ```ignore
//! let keyboard = self.input.keyboard();
//! InputInjector::keyboard_input(&keyboard, self, keycode, key_state, time)?;
//! ```
//!
//! They cannot be methods: `State` owns the injector, so `self.input.keyboard_input(self, ..)`
//! would borrow `self` immutably (for `self.input`) and mutably (as the seat data)
//! at the same time (E0502). Cloning the handle out of the seat first keeps exactly
//! one place per seat call while still letting `State` pass itself as the seat's
//! user data. The handle clones are cheap (`Arc` clones) and always valid: they are
//! created in [`InputInjector::new`] and the seat outlives them.
//!
//! Every helper maps an already-resolved ADesk event onto a single Smithay call.
//! Deciding *what* to inject (resolving a keysym, pressing Shift for shifted
//! characters, resolving a window-relative position, picking the focus surface)
//! stays in `crate::state`.

use adesk_core::{Button, ButtonState, KeyState};
use smithay::{
    backend::input::{
        Axis, AxisSource, ButtonState as SmithayButtonState, KeyState as SmithayKeyState,
    },
    input::{
        keyboard::{FilterResult, KeyboardHandle, Keycode},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle},
        Seat,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
};

use crate::{config::XkbSettings, error::CompositorError, state::State, Result};

use super::keymap::KeymapTable;

/// Key repeat delay in milliseconds before the first repeat.
const KEY_REPEAT_DELAY_MS: i32 = 200;
/// Key repeat rate in keys per second after the delay.
const KEY_REPEAT_RATE_HZ: i32 = 25;

/// Linux evdev button code for the primary mouse button.
const BTN_LEFT: u32 = 0x110;
/// Linux evdev button code for the secondary mouse button.
const BTN_RIGHT: u32 = 0x111;
/// Linux evdev button code for the middle mouse button (wheel click).
const BTN_MIDDLE: u32 = 0x112;
/// Linux evdev button code for the first side button.
const BTN_SIDE: u32 = 0x113;
/// Linux evdev button code for the second side button.
const BTN_EXTRA: u32 = 0x114;

/// The seat's keyboard and pointer handles plus the keysym table.
///
/// Created once during [`State`] construction; the handles are clones of the
/// seat's, so they stay valid for the lifetime of the compositor.
pub(crate) struct InputInjector {
    /// Keyboard handle created from the configured xkb keymap.
    keyboard: KeyboardHandle<State>,
    /// The seat's single pointer handle.
    pointer: PointerHandle<State>,
    /// Keysym → keycode/level table compiled from the same xkb settings.
    keymap: KeymapTable,
}

impl InputInjector {
    /// Add the keyboard and pointer to `seat` and compile the keysym table.
    ///
    /// Fails with [`CompositorError::Keyboard`] if libxkbcommon rejects the
    /// configured keymap (the seat's own keymap is compiled by Smithay and fails
    /// the same way).
    pub(crate) fn new(seat: &Seat<State>, settings: &XkbSettings) -> Result<InputInjector> {
        let keymap = KeymapTable::new(settings)?;

        // `add_keyboard`/`add_pointer` take `&mut self`; `Seat` is a handle, so
        // the clone refers to the very same seat.
        let mut seat = seat.clone();
        let keyboard = seat
            .add_keyboard(
                settings.to_xkb_config(),
                KEY_REPEAT_DELAY_MS,
                KEY_REPEAT_RATE_HZ,
            )
            .map_err(|error| CompositorError::Keyboard(error.to_string()))?;
        let pointer = seat.add_pointer();

        Ok(InputInjector {
            keyboard,
            pointer,
            keymap,
        })
    }

    /// The keysym table used to resolve injected keys.
    pub(crate) fn keymap(&self) -> &KeymapTable {
        &self.keymap
    }

    /// A clone of the seat's keyboard handle, for the injection helpers below.
    pub(crate) fn keyboard(&self) -> KeyboardHandle<State> {
        self.keyboard.clone()
    }

    /// A clone of the seat's pointer handle, for the injection helpers below.
    pub(crate) fn pointer(&self) -> PointerHandle<State> {
        self.pointer.clone()
    }

    /// Deliver one key press/release for the given xkb `keycode`.
    ///
    /// No compositor key bindings are installed, so the event always reaches the
    /// focused client: the filter returns [`FilterResult::Forward`], which makes
    /// Smithay return `None` — that is the normal outcome, not an error.
    pub(crate) fn keyboard_input(
        keyboard: &KeyboardHandle<State>,
        data: &mut State,
        keycode: u32,
        key_state: KeyState,
        time: u32,
    ) -> Result<()> {
        let _forwarded: Option<()> = keyboard.input(
            data,
            Keycode::new(keycode),
            smithay_key_state(key_state),
            SERIAL_COUNTER.next_serial(),
            time,
            |_, _, _| FilterResult::Forward,
        );
        Ok(())
    }

    /// Move the pointer to `location` (output/global coordinates) with an
    /// optional focus surface.
    ///
    /// The single visible toplevel is tiled at the output origin and window
    /// coordinates *are* output coordinates (`docs/protocol.md` §2), so the
    /// focus surface's origin is `(0, 0)`; Smithay subtracts it to compute the
    /// surface-local pointer position.
    pub(crate) fn pointer_motion(
        pointer: &PointerHandle<State>,
        data: &mut State,
        location: Point<f64, Logical>,
        focus: Option<WlSurface>,
        serial: Serial,
        time: u32,
    ) {
        let target = focus.map(|surface| (surface, Point::from((0.0, 0.0))));
        pointer.motion(
            data,
            target,
            &MotionEvent {
                location,
                serial,
                time,
            },
        );
    }

    /// Press or release a pointer button at the current pointer location.
    pub(crate) fn pointer_button(
        pointer: &PointerHandle<State>,
        data: &mut State,
        button: Button,
        button_state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        pointer.button(
            data,
            &ButtonEvent {
                serial,
                time,
                button: button_code(button),
                state: smithay_button_state(button_state),
            },
        );
    }

    /// Scroll by `dx`/`dy` at the current pointer location.
    ///
    /// A zero delta on an axis is omitted; the frame is always terminated so
    /// clients see a complete `wl_pointer.frame`.
    pub(crate) fn pointer_axis(
        pointer: &PointerHandle<State>,
        data: &mut State,
        dx: f64,
        dy: f64,
        time: u32,
    ) {
        pointer.axis(data, axis_frame(time, dx, dy));
        pointer.frame(data);
    }
}

/// The Smithay key state matching an ADesk [`KeyState`].
fn smithay_key_state(state: KeyState) -> SmithayKeyState {
    match state {
        KeyState::Pressed => SmithayKeyState::Pressed,
        KeyState::Released => SmithayKeyState::Released,
    }
}

/// The Smithay button state matching an ADesk [`ButtonState`].
fn smithay_button_state(state: ButtonState) -> SmithayButtonState {
    match state {
        ButtonState::Pressed => SmithayButtonState::Pressed,
        ButtonState::Released => SmithayButtonState::Released,
    }
}

/// The evdev button code for an ADesk [`Button`].
fn button_code(button: Button) -> u32 {
    match button {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
        Button::Side => BTN_SIDE,
        Button::Extra => BTN_EXTRA,
    }
}

/// A wheel axis frame carrying only the non-zero deltas.
///
/// The frame is created unconditionally: even a zero/zero scroll is a complete
/// `wl_pointer.frame`, which is what clients expect.
fn axis_frame(time: u32, dx: f64, dy: f64) -> AxisFrame {
    let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
    if dx != 0.0 {
        frame = frame.value(Axis::Horizontal, dx);
    }
    if dy != 0.0 {
        frame = frame.value(Axis::Vertical, dy);
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_evdev_codes() {
        assert_eq!(button_code(Button::Left), 0x110);
        assert_eq!(button_code(Button::Right), 0x111);
        assert_eq!(button_code(Button::Middle), 0x112);
        assert_eq!(button_code(Button::Side), 0x113);
        assert_eq!(button_code(Button::Extra), 0x114);
    }

    #[test]
    fn key_states_map_to_smithay() {
        assert_eq!(
            smithay_key_state(KeyState::Pressed),
            SmithayKeyState::Pressed
        );
        assert_eq!(
            smithay_key_state(KeyState::Released),
            SmithayKeyState::Released
        );
    }

    #[test]
    fn button_states_map_to_smithay() {
        assert_eq!(
            smithay_button_state(ButtonState::Pressed),
            SmithayButtonState::Pressed
        );
        assert_eq!(
            smithay_button_state(ButtonState::Released),
            SmithayButtonState::Released
        );
    }

    #[test]
    fn axis_frame_keeps_the_source_and_omits_zero_deltas() {
        let vertical = axis_frame(42, 0.0, -3.0);
        assert_eq!(vertical.time, 42);
        assert_eq!(vertical.source, Some(AxisSource::Wheel));
        assert_eq!(vertical.axis, (0.0, -3.0));

        let both = axis_frame(7, 1.5, 2.5);
        assert_eq!(both.axis, (1.5, 2.5));

        let empty = axis_frame(0, 0.0, 0.0);
        assert_eq!(empty.axis, (0.0, 0.0));
        assert_eq!(empty.source, Some(AxisSource::Wheel));
    }
}
