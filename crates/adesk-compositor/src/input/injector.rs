//! Thin wrappers around the seat's keyboard and pointer handles.
//!
//! [`InputInjector`] owns the handles created by [`State`] startup plus the
//! [`KeymapTable`] used to turn keysyms into keycodes. Every method here does
//! exactly one thing to Smithay: it maps an already-resolved ADesk event onto a
//! single seat call. Deciding *what* to inject (resolving a keysym, pressing
//! Shift for shifted characters, resolving a window-relative position, picking
//! the focus surface) stays in `crate::state`.

use adesk_core::{Button, ButtonState, KeyState};
use smithay::{
    backend::input::{
        Axis, AxisSource, ButtonState as SmithayButtonState, KeyState as SmithayKeyState,
    },
    input::{
        keyboard::{FilterResult, Keycode, KeyboardHandle},
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

    /// Deliver one key press/release for the given xkb `keycode`.
    ///
    /// No compositor key bindings are installed, so the event always reaches the
    /// focused client: the filter returns [`FilterResult::Forward`], which makes
    /// Smithay return `None` — that is the normal outcome, not an error.
    pub(crate) fn keyboard_input(
        &self,
        data: &mut State,
        keycode: u32,
        key_state: KeyState,
        time: u32,
    ) -> Result<()> {
        let state = match key_state {
            KeyState::Pressed => SmithayKeyState::Pressed,
            KeyState::Released => SmithayKeyState::Released,
        };
        let _intercepted: Option<()> = self.keyboard.input(
            data,
            Keycode::new(keycode),
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |_, _, _| FilterResult::Forward,
        );
        Ok(())
    }

    /// Move keyboard focus to `target` (or clear it with `None`).
    pub(crate) fn set_keyboard_focus(
        &self,
        data: &mut State,
        target: Option<WlSurface>,
        serial: Serial,
    ) {
        self.keyboard.set_focus(data, target, serial);
    }

    /// Move the pointer to `location` (output/global coordinates) with an
    /// optional focus surface.
    ///
    /// The single visible toplevel is tiled at the output origin and window
    /// coordinates *are* output coordinates (`docs/protocol.md` §2), so the
    /// focus surface's origin is `(0, 0)`; Smithay subtracts it to compute the
    /// surface-local pointer position.
    pub(crate) fn pointer_motion(
        &self,
        data: &mut State,
        location: Point<f64, Logical>,
        focus: Option<WlSurface>,
        serial: Serial,
        time: u32,
    ) {
        let target = focus.map(|surface| (surface, Point::from((0.0, 0.0))));
        self.pointer.motion(
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
        &self,
        data: &mut State,
        button: Button,
        button_state: ButtonState,
        serial: Serial,
        time: u32,
    ) {
        let state = match button_state {
            ButtonState::Pressed => SmithayButtonState::Pressed,
            ButtonState::Released => SmithayButtonState::Released,
        };
        self.pointer.button(
            data,
            &ButtonEvent {
                serial,
                time,
                button: button_code(button),
                state,
            },
        );
    }

    /// Scroll by `dx`/`dy` at the current pointer location.
    ///
    /// A zero delta on an axis is omitted; the frame is always terminated so
    /// clients see a complete `wl_pointer.frame`.
    pub(crate) fn pointer_axis(&self, data: &mut State, dx: f64, dy: f64, time: u32) {
        let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
        if dx != 0.0 {
            frame = frame.value(Axis::Horizontal, dx);
        }
        if dy != 0.0 {
            frame = frame.value(Axis::Vertical, dy);
        }
        self.pointer.axis(data, frame);
        self.pointer.frame(data);
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
}
