//! `commit_timing` overlay: commit watermark HUD.

use adesk_core::Point;

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;
use crate::text;

use super::common::ink_origin;

/// Draws a right-aligned HUD in the canvas' top-right corner when
/// [`InspectionInput::commit`] is `Some`: text
/// `"commit {commit_seq} +{age_ms}ms"` in [`OverlayStyle::timing`] on the
/// standard label plate, with the plate's right edge (exclusive) at
/// `canvas.clip().right() - pad` and its top at `canvas.clip().y + pad`.
///
/// Draws nothing when no commit has been observed.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    let Some(commit) = input.commit else {
        return;
    };
    // The HUD is an accent overlay: its label uses the timing colour.
    let mut accent = *style;
    accent.text = style.timing;
    let label = format!("commit {} +{}ms", commit.commit_seq, commit.age_ms);
    let pad = style.pad();
    let size = text::measure(&label, style.scale());
    // `draw_label` derives the plate as the ink rect inflated by `pad`, so
    // anchoring the plate's top-left with its right edge (exclusive) at
    // `clip.right() - pad` puts the drawn plate exactly there.
    let plate_width = size.w.saturating_add(2 * pad as u32) as i32;
    let clip = canvas.clip();
    let plate = Point {
        x: clip.right().saturating_sub(plate_width).saturating_sub(pad),
        y: clip.y.saturating_add(pad),
    };
    let origin = ink_origin(plate, pad);
    text::draw_label(canvas, origin, &label, &accent);
}
