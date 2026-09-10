//! `window_ids` overlay: a `win <id>` label per window.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Draws `"win {id}"` in slot 0 of every window's top-left corner (slot layout
/// is shared in `paint::labels`) in input order.
///
/// The label is elided to the window's inner width and clipped to the window;
/// windows too small to show anything draw nothing.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    for window in &input.windows {
        let text = format!("win {}", window.id);
        super::labels::draw(
            canvas,
            window.geometry,
            super::labels::SLOT_WINDOW_IDS,
            &text,
            style,
        );
    }
}
