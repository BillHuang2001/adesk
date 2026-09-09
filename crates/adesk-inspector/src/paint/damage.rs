//! `damage` overlay: translucent fills plus 1 px outlines for damage rects.

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Paints every rect in [`InspectionInput::damage`] in input order: fill with
/// [`OverlayStyle::fill`], then outline with [`OverlayStyle::outline`].
///
/// Empty rects are skipped; overlapping rects blend repeatedly (damage is
/// evidence, not a mask).
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let _ = (canvas, input, style);
    todo!("Phase 2: for each damage rect, fill_rect(fill) then outline_rect(outline)")
}
