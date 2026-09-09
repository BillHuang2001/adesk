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
    let Some(active) = input.active else {
        return;
    };
    let Some(window) = input.windows.iter().find(|window| window.id == active) else {
        return;
    };
    canvas.outline_rect(window.geometry, style.focus);
    // The focus label is an accent: text in `style.focus`, plate unchanged.
    let label_style = OverlayStyle {
        text: style.focus,
        ..*style
    };
    super::labels::draw(
        canvas,
        window.geometry,
        super::labels::SLOT_FOCUS,
        "focus",
        &label_style,
    );
}
