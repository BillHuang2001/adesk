//! `app_ids` overlay: an `app <app_id>` label per window.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Draws `"app {app_id}"` in slot 1 of every window's top-left corner (see
/// [`super::labels`]) in input order; windows without an app id draw
/// `"app ?"` (correlation failure is reported, never guessed).
///
/// The label is elided to the window's inner width and clipped to the window.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let _ = (canvas, input, style);
    todo!("Phase 2: labels::draw(canvas, window.geometry, SLOT_APP_IDS, text, style)")
}
