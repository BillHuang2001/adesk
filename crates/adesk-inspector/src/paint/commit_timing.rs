//! `commit_timing` overlay: commit watermark HUD.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Draws a right-aligned HUD in the canvas' top-right corner when
/// [`InspectionInput::commit`] is `Some`: text
/// `"commit {commit_seq} +{age_ms}ms"` in [`OverlayStyle::timing`] on the
/// standard label plate, with the plate's right edge at
/// `canvas.clip().right() - pad` and its top at `canvas.clip().y + pad`.
///
/// Draws nothing when no commit has been observed.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let _ = (canvas, input, style);
    todo!("Phase 2: right-align the commit HUD in the top-right corner")
}
