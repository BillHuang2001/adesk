//! Unit-level behaviour of the drawing primitives ([`Canvas`]) and the built-in
//! font/text layout. These back every overlay pixel assertion.

mod common;

use std::borrow::Cow;

use adesk_core::{Point, Rect, Size};
use adesk_inspector::canvas::blend_over;
use adesk_inspector::font;
use adesk_inspector::text;
use adesk_inspector::{Canvas, Color, OverlayStyle};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn pixel_blend_alpha_255_replaces_and_alpha_0_is_noop() {
    let mut frame = common::filled_frame(2, 1, [10, 20, 30, 255]);
    let mut canvas = Canvas::new(&mut frame);
    canvas.pixel(Point { x: 0, y: 0 }, Color::rgb(200, 100, 50));
    canvas.pixel(Point { x: 1, y: 0 }, Color::rgba(200, 100, 50, 0));
    assert_eq!(common::px(&frame, 0, 0), [200, 100, 50, 255]);
    assert_eq!(common::px(&frame, 1, 0), [10, 20, 30, 255]);
}

#[test]
fn pixel_blend_half_alpha_matches_documented_formula() {
    let blended = blend_over(Color::rgba(255, 0, 0, 128), [0, 0, 0, 255]);
    assert_eq!(blended, [128, 0, 0, 255]);
    // The formula is symmetric in the destination: a=128 keeps half of it.
    assert_eq!(
        blend_over(Color::rgba(0, 0, 255, 128), [0, 0, 0, 255]),
        [0, 0, 128, 255]
    );
    assert_eq!(
        blend_over(Color::rgba(255, 0, 0, 0), [7, 8, 9, 200]),
        [7, 8, 9, 200]
    );
    assert_eq!(
        blend_over(Color::rgba(1, 2, 3, 255), [7, 8, 9, 200]),
        [1, 2, 3, 255]
    );
}

#[test]
fn fill_rect_clips_to_canvas_and_clip() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.with_clip(common::rect(2, 2, 4, 4), |canvas| {
        canvas.fill_rect(common::rect(-2, -2, 8, 8), Color::WHITE);
    });
    for y in 0..8 {
        for x in 0..8 {
            let inside = (2..=5).contains(&x) && (2..=5).contains(&y);
            let expected = if inside { WHITE } else { BLACK };
            assert_eq!(common::px(&frame, x, y), expected, "pixel ({x},{y})");
        }
    }
    assert_eq!(common::count_pixels(&frame, WHITE), 16);
}

#[test]
fn with_clip_restores_previous_clip() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    let outer = canvas.clip();
    canvas.with_clip(common::rect(0, 0, 2, 2), |_| {});
    assert_eq!(canvas.clip(), outer);
    assert_eq!(canvas.clip(), common::rect(0, 0, 8, 8));
}

#[test]
fn outline_rect_draws_boundary_pixels_only() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.outline_rect(common::rect(1, 1, 4, 3), Color::WHITE);
    // (1,1,4,3) covers x 1..=4 and y 1..=3; its boundary is the 10 perimeter
    // pixels (the todo said 12, which is the full area, not the perimeter).
    for y in 1..=3 {
        for x in 1..=4 {
            let boundary = y == 1 || y == 3 || x == 1 || x == 4;
            let expected = if boundary { WHITE } else { BLACK };
            assert_eq!(common::px(&frame, x, y), expected, "pixel ({x},{y})");
        }
    }
    assert_eq!(common::px(&frame, 2, 2), BLACK);
    assert_eq!(common::px(&frame, 3, 2), BLACK);
    assert_eq!(common::count_pixels(&frame, WHITE), 10);
}

#[test]
fn hline_and_vline_include_both_endpoints() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.hline(3, 1, 5, Color::WHITE);
    canvas.vline(6, 0, 2, Color::WHITE);
    for x in 1..=5 {
        assert_eq!(common::px(&frame, x, 3), WHITE, "hline pixel ({x},3)");
    }
    for y in 0..=2 {
        assert_eq!(common::px(&frame, 6, y), WHITE, "vline pixel (6,{y})");
    }
    assert_eq!(common::count_pixels(&frame, WHITE), 8);
    assert_eq!(common::px(&frame, 0, 3), BLACK);
    assert_eq!(common::px(&frame, 7, 3), BLACK);
    assert_eq!(common::px(&frame, 6, 3), BLACK);
}

#[test]
fn line_endpoints_are_included() {
    let mut frame = common::frame(8, 8);
    let mut canvas = Canvas::new(&mut frame);
    canvas.line(Point { x: 0, y: 0 }, Point { x: 7, y: 7 }, Color::WHITE);
    assert_eq!(common::px(&frame, 0, 0), WHITE);
    assert_eq!(common::px(&frame, 7, 7), WHITE);
    for step in 0..8 {
        assert_eq!(
            common::px(&frame, step, step),
            WHITE,
            "diagonal ({step},{step})"
        );
    }
    assert_eq!(common::count_pixels(&frame, WHITE), 8);
}

#[test]
fn measure_matches_drawn_ink_bounds() {
    let mut frame = common::frame(64, 16);
    let mut canvas = Canvas::new(&mut frame);
    let ink = text::draw(&mut canvas, Point::ORIGIN, "win 7", 1, Color::WHITE);
    let size = text::measure("win 7", 1);
    assert_eq!(
        ink,
        Rect {
            x: 0,
            y: 0,
            w: 29,
            h: 7
        }
    );
    assert_eq!(size, Size { w: 29, h: 7 });
    assert_eq!(ink.size(), size);
    assert_eq!(text::measure("", 1), Size { w: 0, h: 0 });
    // Five glyphs advance 5 * 12 = 60 px at scale 2, minus the 2 px trailing gap.
    assert_eq!(text::measure("win 7", 2), Size { w: 58, h: 14 });
}

#[test]
fn elide_appends_double_dot_and_returns_none_when_nothing_fits() {
    let wide = text::measure("org.mozilla.firefox", 1).w as i32;
    assert_eq!(wide, 113);
    assert_eq!(
        text::elide("org.mozilla.firefox", 40, 1).as_deref(),
        Some("org...")
    );
    assert_eq!(text::elide("org.mozilla.firefox", 4, 1).as_deref(), None);
    assert_eq!(text::elide("org.mozilla.firefox", 10, 1).as_deref(), None);
    assert_eq!(
        text::elide("org.mozilla.firefox", 11, 1).as_deref(),
        Some("..")
    );
    assert_eq!(text::elide("win 7", 29, 1).as_deref(), Some("win 7"));
    assert!(matches!(
        text::elide("win 7", 29, 1),
        Some(Cow::Borrowed("win 7"))
    ));
}

#[test]
fn draw_label_plate_includes_padding_and_border() {
    let mut frame = common::frame(64, 16);
    let mut canvas = Canvas::new(&mut frame);
    let style = OverlayStyle::default();
    let plate = text::draw_label(&mut canvas, Point { x: 3, y: 3 }, "win 7", &style);
    assert_eq!(
        plate,
        Rect {
            x: 1,
            y: 1,
            w: 33,
            h: 11
        }
    );
    assert_eq!(plate.size(), Size { w: 33, h: 11 });
    // The plate boundary is the outline colour.
    for x in plate.x..plate.right() {
        for y in [plate.y, plate.bottom() - 1] {
            assert_eq!(
                common::px(&frame, x as u32, y as u32),
                style.outline.to_array(),
                "plate border ({x},{y})"
            );
        }
    }
    for y in plate.y..plate.bottom() {
        for x in [plate.x, plate.right() - 1] {
            assert_eq!(
                common::px(&frame, x as u32, y as u32),
                style.outline.to_array(),
                "plate border ({x},{y})"
            );
        }
    }
    // The text ink area (origin 3,3 + 29x7) carries text-coloured pixels.
    let ink = Rect {
        x: 3,
        y: 3,
        w: 29,
        h: 7,
    };
    let mut ink_pixels = 0;
    for y in ink.y..ink.bottom() {
        for x in ink.x..ink.right() {
            if common::px(&frame, x as u32, y as u32) == style.text.to_array() {
                ink_pixels += 1;
            }
        }
    }
    assert!(
        ink_pixels > 0,
        "label text must be drawn inside the ink rect"
    );
    // Everything outside the plate keeps the base frame colour.
    for y in 0..16 {
        for x in 0..64 {
            if plate.contains(Point { x, y }) {
                continue;
            }
            assert_eq!(
                common::px(&frame, x as u32, y as u32),
                BLACK,
                "outside ({x},{y})"
            );
        }
    }
}

#[test]
fn font_covers_printable_ascii_and_falls_back() {
    for c in ' '..='~' {
        let glyph = font::glyph(c).unwrap_or_else(|| panic!("missing glyph for {c:?}"));
        assert_eq!(glyph.rows.len(), usize::from(font::FONT_HEIGHT));
        if c != ' ' {
            assert!(
                (0..usize::from(font::FONT_HEIGHT))
                    .any(|y| (0..usize::from(font::FONT_WIDTH)).any(|x| glyph.ink(x, y))),
                "glyph {c:?} must have ink"
            );
        }
    }
    assert!(font::glyph('\n').is_none());
    assert!(font::glyph('é').is_none());
    assert_eq!(font::glyph(' ').unwrap().rows, [0; 7]);
    assert_eq!(font::fallback(), font::glyph('?').unwrap());
    assert!(
        (0..usize::from(font::FONT_HEIGHT))
            .any(|y| (0..usize::from(font::FONT_WIDTH)).any(|x| font::fallback().ink(x, y))),
        "fallback glyph must have ink"
    );
}

#[test]
fn font_metrics_scale_linearly() {
    assert_eq!(font::advance(2), 12);
    assert_eq!(font::line_height(2), 16);
    assert_eq!(font::glyph_width(2), 10);
    assert_eq!(font::glyph_height(2), 14);
    assert_eq!(font::clamp_scale(0), 1);
    assert_eq!(font::clamp_scale(9), 8);
}
