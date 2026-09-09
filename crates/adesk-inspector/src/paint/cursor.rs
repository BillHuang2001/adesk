//! `cursor` overlay: a crosshair at the pointer position.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Half-length of each crosshair arm in px (the centre is shared, so the
/// crosshair spans `2 * ARM + 1 = 9` px per axis).
const ARM: i32 = 4;

/// Draws a crosshair centred on [`InspectionInput::cursor`] in
/// [`OverlayStyle::cursor`]: a horizontal line from `x - 4` to `x + 4` at `y`
/// and a vertical line from `y - 4` to `y + 4` at `x` (9 px arms, centre
/// included). Clipped to the canvas; draws nothing when the cursor is unknown.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let Some(cursor) = input.cursor else {
        return;
    };
    // Saturating arithmetic keeps extreme coordinates from panicking; the
    // canvas clips both lines to `clip ∩ buffer`.
    canvas.hline(
        cursor.y,
        cursor.x.saturating_sub(ARM),
        cursor.x.saturating_add(ARM),
        style.cursor,
    );
    canvas.vline(
        cursor.x,
        cursor.y.saturating_sub(ARM),
        cursor.y.saturating_add(ARM),
        style.cursor,
    );
}
