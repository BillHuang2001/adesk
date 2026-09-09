//! `focus` overlay: outline and label for the active window.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Outlines the active window's geometry with [`OverlayStyle::focus`] and draws
/// the label `"focus"` in slot 2 of its top-left corner (see
/// [`super::labels`]).
///
/// Draws nothing when [`InspectionInput::active`] is `None` or does not match
/// any window.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let _ = (canvas, input, style);
    todo!("Phase 2: find first window with id == input.active, outline + slot-2 label")
}
