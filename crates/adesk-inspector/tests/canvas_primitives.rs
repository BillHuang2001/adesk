//! Unit-level behaviour of the drawing primitives ([`Canvas`]) and the built-in
//! font/text layout. These back every overlay pixel assertion.

mod common;

use adesk_core::Point;
use adesk_inspector::canvas::blend_over;
use adesk_inspector::font;
use adesk_inspector::text;
use adesk_inspector::{Canvas, Color, OverlayStyle};

#[test]
fn pixel_blend_alpha_255_replaces_and_alpha_0_is_noop() {
    let mut frame = common::filled_frame(2, 1, [10, 20, 30, 255]);
    let mut canvas = Canvas::new(&mut frame);
    canvas.pixel(Point { x: 0, y: 0 }, Color::rgb(200, 100, 50));
    canvas.pixel(Point { x: 1, y: 0 }, Color::rgba(200, 100, 50, 0));
    todo!("assert (0,0) == [200,100,50,255] and (1,0) is unchanged");
}

#[test]
fn pixel_blend_half_alpha_matches_documented_formula() {
    let _blended = blend_over(Color::rgba(255, 0, 0, 128), [0, 0, 0, 255]);
    todo!("assert blended == [128,0,0,255] per (src*a + dst*(255-a) + 127)/255");
}

#[test]
fn fill_rect_clips_to_canvas_and_clip() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.with_clip(common::rect(2, 2, 4, 4), |canvas| {
        canvas.fill_rect(common::rect(-2, -2, 8, 8), Color::WHITE);
    });
    todo!("assert exactly the 4x4 clip region is white and the rest is black");
}

#[test]
fn with_clip_restores_previous_clip() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    let outer = canvas.clip();
    canvas.with_clip(common::rect(0, 0, 2, 2), |_| {});
    todo!("assert canvas.clip() == outer ({outer:?}) after with_clip returns");
}

#[test]
fn outline_rect_draws_boundary_pixels_only() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.outline_rect(common::rect(1, 1, 4, 3), Color::WHITE);
    todo!("assert the 12 boundary pixels of (1,1,4,3) are white and the 2 interior pixels are not");
}

#[test]
fn hline_and_vline_include_both_endpoints() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.hline(3, 1, 5, Color::WHITE);
    canvas.vline(6, 0, 2, Color::WHITE);
    todo!("assert (1..=5,3) and (6,0..=2) are white");
}

#[test]
fn line_endpoints_are_included() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.line(Point { x: 0, y: 0 }, Point { x: 7, y: 7 }, Color::WHITE);
    todo!("assert both endpoints are white and the diagonal is 8 pixels");
}

#[test]
fn measure_matches_drawn_ink_bounds() {
    let mut frame = common::frame(64, 16);
    let mut canvas = Canvas::new(&mut frame);
    let _ink = text::draw(&mut canvas, Point::ORIGIN, "win 7", 1, Color::WHITE);
    let _size = text::measure("win 7", 1);
    todo!("assert ink.size() == size == Size {{ w: 29, h: 7 }} (5 chars * 6 - 1)");
}

#[test]
fn elide_appends_double_dot_and_returns_none_when_nothing_fits() {
    let wide = text::measure("org.mozilla.firefox", 1).w as i32;
    let _ = (wide, text::elide("org.mozilla.firefox", 40, 1), text::elide("org.mozilla.firefox", 4, 1));
    todo!("assert elide(...,40) is Some(\"..\")-suffixed prefix, elide(...,4) is None, and exact fit returns the input unchanged");
}

#[test]
fn draw_label_plate_includes_padding_and_border() {
    let mut frame = common::frame(64, 16);
    let mut canvas = Canvas::new(&mut frame);
    let style = OverlayStyle::default();
    let _plate = text::draw_label(&mut canvas, Point { x: 3, y: 3 }, "win 7", &style);
    todo!("assert plate == (1,1,33,11): ink 29x7 inflated by padding 2; border on plate boundary");
}

#[test]
fn font_covers_printable_ascii_and_falls_back() {
    for c in ' '..='~' {
        let _ = font::glyph(c);
    }
    let _ = font::fallback();
    todo!("assert every printable ASCII char has a glyph and non-ASCII has none");
}

#[test]
fn font_metrics_scale_linearly() {
    todo!(
        "assert advance(2) == 12, line_height(2) == 16, glyph_width(2) == 10, clamp_scale(0) == 1, clamp_scale(9) == 8"
    );
}
