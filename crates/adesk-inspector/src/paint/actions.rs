//! `actions` overlay: markers for positioned actions, a HUD for the rest.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Paints [`InspectionInput::actions`] in input order (later markers draw over
/// earlier ones).
///
/// - A marker with a position draws a plus-shaped glyph in
///   [`OverlayStyle::action`] — a 3x3 filled square centred on the position
///   plus 1 px arms extending 3 px in each direction — followed by a label at
///   `(position.x + 4, position.y + 4)` (plate padding applied by the label)
///   with text `"{kind} #{action_id} +{age_ms}ms"`.
/// - A marker without a position is listed in a HUD at the canvas top-left:
///   one line per positionless marker, `i`-th line at
///   `(pad, pad + i * (line_height + pad))`, same text.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let _ = (canvas, input, style);
    todo!("Phase 2: plus markers + labels for positioned actions, top-left HUD otherwise")
}
