//! `actions` overlay: markers for positioned actions, a HUD for the rest.

use adesk_core::{Point, Rect};

use crate::canvas::Canvas;
use crate::color::Color;
use crate::font;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;
use crate::text;

use super::common::{crosshair, ink_origin, ARM};

/// Side length in px of the filled centre square of a plus marker.
const CENTRE: u32 = 3;

/// Paints [`InspectionInput::actions`] in input order (later markers draw over
/// earlier ones).
///
/// - A marker with a position draws a plus-shaped glyph in
///   [`OverlayStyle::action`] — a 3x3 filled square centred on the position
///   plus 1 px arms extending 4 px from the centre (9 px span) — followed by a
///   label whose ink top-left is one arm length past the centre
///   (`(position.x + 4, position.y + 4)`) with text
///   `"{kind} #{action_id} +{age_ms}ms"`, drawn in the action accent colour.
/// - A marker without a position is listed in a HUD anchored to the canvas
///   clip's top-left corner: the `i`-th positionless marker's plate top-left is
///   `(clip.x + pad, clip.y + pad + i * (line_height + pad))`, same text, same
///   accent colour. `i` counts positionless markers only.
pub fn paint(canvas: &mut Canvas<'_>, input: &InspectionInput, style: &OverlayStyle) {
    // Actions are an accent overlay: their labels use the action colour rather
    // than the default text colour.
    let mut accent = *style;
    accent.text = style.action;
    let pad = style.pad();
    let step = font::line_height(style.scale()).saturating_add(pad);
    let mut hud_line: i32 = 0;

    for marker in &input.actions {
        let label = format!(
            "{} #{} +{}ms",
            marker.kind.as_str(),
            marker.action_id,
            marker.age_ms
        );
        match marker.position {
            Some(position) => {
                plus(canvas, position, style.action);
                let origin = ink_origin(position, ARM);
                text::draw_label(canvas, origin, &label, &accent);
            }
            None => {
                let clip = canvas.clip();
                let plate = Point {
                    x: clip.x.saturating_add(pad),
                    y: clip
                        .y
                        .saturating_add(pad)
                        .saturating_add(hud_line.saturating_mul(step)),
                };
                let origin = ink_origin(plate, pad);
                text::draw_label(canvas, origin, &label, &accent);
                hud_line = hud_line.saturating_add(1);
            }
        }
    }
}

/// Draws the plus-shaped marker centred on `position` in `color`: a 9 px
/// horizontal arm, a 9 px vertical arm and a 3x3 centre square.
fn plus(canvas: &mut Canvas<'_>, position: Point, color: Color) {
    crosshair(canvas, position, color);
    canvas.fill_rect(
        Rect::new(
            position.x.saturating_sub(1),
            position.y.saturating_sub(1),
            CENTRE,
            CENTRE,
        ),
        color,
    );
}
