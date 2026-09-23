//! Pure pointer press/release pairing for the frame view (GTK-free).
//!
//! Every button press the frame view forwards to the runtime must eventually be
//! matched by a release, or the remote pointer stays stuck down. Two GTK facts
//! make that easy to get wrong:
//! - clicking an unfocused frame view *both* takes keyboard control and must be
//!   delivered (the focus change must never swallow the press);
//! - a drag can start inside the desktop and end over a letterbox bar, where the
//!   release no longer maps onto the image.
//!
//! [`PointerState`] therefore owns both decisions: a press is forwarded only
//! when it lands on the displayed image, and is remembered; a release is
//! forwarded whenever its press was (with a clamped position) and dropped
//! otherwise. The GTK layer only supplies the already-mapped geometry.

use adesk_core::Button;

/// The decision for one pointer button event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Send the event to the runtime.
    Forward,
    /// Do nothing (there is nothing to press, or nothing is pressed).
    Drop,
}

/// Remembers which buttons the viewer has forwarded a press for.
///
/// Only the buttons this process forwarded are tracked, so an unrelated release
/// (e.g. one that arrives after a press in the letterbox bars was dropped) never
/// reaches the runtime.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PointerState {
    /// The buttons currently held down, in press order.
    down: Vec<Button>,
}

impl PointerState {
    /// Creates a state with no button held.
    pub(crate) fn new() -> PointerState {
        PointerState::default()
    }

    /// Decides a press landing at a point that is (`inside`) or is not within
    /// the displayed desktop image.
    ///
    /// A press on the image is always forwarded — including the first click on
    /// an unfocused view, which takes keyboard control *and* reaches the remote
    /// app. A press in the letterbox bars has no target and is dropped.
    pub(crate) fn press(&mut self, button: Button, inside: bool) -> Delivery {
        if !inside {
            return Delivery::Drop;
        }
        if !self.down.contains(&button) {
            self.down.push(button);
        }
        Delivery::Forward
    }

    /// Decides a release for `button`.
    ///
    /// Forwarded exactly when the matching press was, so the runtime's button
    /// state always returns to rest even if the pointer left the image while
    /// held; a release that was never pressed here is dropped.
    pub(crate) fn release(&mut self, button: Button) -> Delivery {
        match self.down.iter().position(|held| *held == button) {
            Some(index) => {
                self.down.remove(index);
                Delivery::Forward
            }
            None => Delivery::Drop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_on_the_image_is_forwarded() {
        let mut state = PointerState::new();
        assert_eq!(state.press(Button::Left, true), Delivery::Forward);
    }

    #[test]
    fn a_press_in_the_bars_is_dropped() {
        let mut state = PointerState::new();
        assert_eq!(state.press(Button::Left, false), Delivery::Drop);
        // ... and its release must not be forwarded either: nothing is held.
        assert_eq!(state.release(Button::Left), Delivery::Drop);
    }

    #[test]
    fn a_release_matches_its_forwarded_press() {
        let mut state = PointerState::new();
        state.press(Button::Left, true);
        assert_eq!(state.release(Button::Left), Delivery::Forward);
        // The button is no longer held, so a stray second release is dropped.
        assert_eq!(state.release(Button::Left), Delivery::Drop);
    }

    #[test]
    fn a_drag_that_ends_outside_the_image_still_releases() {
        // The press landed on the image, so the release is forwarded with a
        // clamped position even when it lands in a letterbox bar.
        let mut state = PointerState::new();
        assert_eq!(state.press(Button::Right, true), Delivery::Forward);
        assert_eq!(state.release(Button::Right), Delivery::Forward);
    }

    #[test]
    fn buttons_pair_independently() {
        let mut state = PointerState::new();
        state.press(Button::Left, true);
        state.press(Button::Middle, true);
        assert_eq!(state.release(Button::Middle), Delivery::Forward);
        assert_eq!(state.release(Button::Left), Delivery::Forward);
    }

    #[test]
    fn a_repeated_press_does_not_double_record() {
        let mut state = PointerState::new();
        state.press(Button::Left, true);
        state.press(Button::Left, true);
        assert_eq!(state.release(Button::Left), Delivery::Forward);
        assert_eq!(state.release(Button::Left), Delivery::Drop);
    }

    #[test]
    fn a_release_without_a_press_is_dropped() {
        let mut state = PointerState::new();
        assert_eq!(state.release(Button::Side), Delivery::Drop);
        assert_eq!(state.release(Button::Extra), Delivery::Drop);
    }
}
