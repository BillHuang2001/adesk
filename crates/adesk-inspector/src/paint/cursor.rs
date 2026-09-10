//! `cursor` overlay: a crosshair at the pointer position.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

use super::common::crosshair;

/// Draws a crosshair centred on [`InspectionInput::cursor`] in
/// [`OverlayStyle::cursor`]: a horizontal line from `x - 4` to `x + 4` at `y`
/// and a vertical line from `y - 4` to `y + 4` at `x` (9 px arms, centre
/// included). Clipped to the canvas; draws nothing when the cursor is unknown.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let Some(cursor) = input.cursor else {
        return;
    };
    crosshair(canvas, cursor, style.cursor);
}
