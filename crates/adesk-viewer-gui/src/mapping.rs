//! Pure widget ↔ normalized coordinate mapping (the letterbox math).
//!
//! VAP coordinates are **normalized** `0.0..=1.0` output fractions, never
//! pixels (`docs/viewer.md` §3, §4). To turn a click inside the frame widget
//! into a VAP pointer position the viewer must (a) find where the displayed
//! desktop image actually sits inside the widget — an aspect-preserving,
//! centered rectangle — and (b) express the click as a fraction of that
//! rectangle, clamping anything outside it to the edges.
//!
//! This module is GTK-free so the arithmetic is unit-testable without a display.

/// The displayed image's rectangle inside the frame widget, in widget pixels.
///
/// The rectangle is centered and preserves the image's aspect ratio: a widget
/// taller than the image yields pillarboxing (bars left/right), a wider widget
/// yields letterboxing (bars top/bottom).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayRect {
    /// Left edge in widget pixels.
    pub x: f64,
    /// Top edge in widget pixels.
    pub y: f64,
    /// Width in widget pixels.
    pub w: f64,
    /// Height in widget pixels.
    pub h: f64,
}

/// Computes the aspect-preserving, centered rectangle of an `image` inside a
/// `widget` (both given as `(width, height)`), in widget pixels.
///
/// The scale is `min(widget.w / image.w, widget.h / image.h)` and the offset is
/// `(widget_dim - image_dim * scale) / 2`, so the image fills one axis and is
/// centered on the other.
///
/// Returns `None` when any input dimension is zero or non-finite (there is no
/// meaningful rectangle to compute for a collapsed widget or an empty image).
#[must_use]
pub fn letterbox(widget: (f64, f64), image: (u32, u32)) -> Option<DisplayRect> {
    let (widget_w, widget_h) = widget;
    let (image_w, image_h) = image;

    if !widget_w.is_finite() || !widget_h.is_finite() || widget_w <= 0.0 || widget_h <= 0.0 {
        return None;
    }
    if image_w == 0 || image_h == 0 {
        return None;
    }

    let scale = (widget_w / f64::from(image_w)).min(widget_h / f64::from(image_h));
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }

    let w = f64::from(image_w) * scale;
    let h = f64::from(image_h) * scale;
    Some(DisplayRect {
        x: (widget_w - w) / 2.0,
        y: (widget_h - h) / 2.0,
        w,
        h,
    })
}

/// Maps a widget-local `point` to a normalized desktop fraction relative to
/// `display`.
///
/// The result is clamped into `0.0..=1.0`, so a click in the letterbox/
/// pillarbox bars — or outside the widget entirely — resolves to the nearest
/// edge. A zero-size `display` yields `(0.0, 0.0)` instead of a division by
/// zero (there is nothing to point at).
#[must_use]
pub fn widget_to_normalized(point: (f64, f64), display: &DisplayRect) -> (f64, f64) {
    let (px, py) = point;
    let x = if display.w > 0.0 {
        (px - display.x) / display.w
    } else {
        0.0
    };
    let y = if display.h > 0.0 {
        (py - display.y) / display.h
    } else {
        0.0
    };
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

/// Whether `point` (widget-local pixels) lies within `display` (edges
/// inclusive).
///
/// A click on the displayed desktop image is delivered to the runtime, while one
/// in the letterbox/pillarbox bars has no desktop target and is dropped — unlike
/// pointer motion, which [`widget_to_normalized`] clamps to the nearest edge.
#[must_use]
pub fn contains(display: &DisplayRect, point: (f64, f64)) -> bool {
    point.0 >= display.x
        && point.0 <= display.x + display.w
        && point.1 >= display.y
        && point.1 <= display.y + display.h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts two floats are within a small tolerance.
    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn exact_aspect_ratio_yields_an_identity_rect() {
        let display = letterbox((800.0, 600.0), (800, 600)).unwrap();
        close(display.x, 0.0);
        close(display.y, 0.0);
        close(display.w, 800.0);
        close(display.h, 600.0);
    }

    #[test]
    fn a_tall_widget_pillarboxes() {
        // Widget 400x800, image 800x600: scale = min(0.5, 1.333..) = 0.5.
        let display = letterbox((400.0, 800.0), (800, 600)).unwrap();
        close(display.w, 400.0);
        close(display.h, 300.0);
        close(display.x, 0.0);
        close(display.y, 250.0);
    }

    #[test]
    fn a_wide_widget_letterboxes() {
        // Widget 1600x600, image 800x600: scale = min(2.0, 1.0) = 1.0.
        let display = letterbox((1600.0, 600.0), (800, 600)).unwrap();
        close(display.w, 800.0);
        close(display.h, 600.0);
        close(display.x, 400.0);
        close(display.y, 0.0);
    }

    #[test]
    fn the_corners_map_to_the_normalized_corners() {
        let display = letterbox((400.0, 800.0), (800, 600)).unwrap();
        let (x, y) = widget_to_normalized((display.x, display.y), &display);
        close(x, 0.0);
        close(y, 0.0);

        let (x, y) = widget_to_normalized((display.x + display.w, display.y + display.h), &display);
        close(x, 1.0);
        close(y, 1.0);
    }

    #[test]
    fn a_center_point_maps_to_the_center() {
        let display = letterbox((1600.0, 600.0), (800, 600)).unwrap();
        let (x, y) = widget_to_normalized(
            (display.x + display.w / 2.0, display.y + display.h / 2.0),
            &display,
        );
        close(x, 0.5);
        close(y, 0.5);
    }

    #[test]
    fn a_point_outside_the_image_rect_clamps() {
        let display = DisplayRect {
            x: 100.0,
            y: 50.0,
            w: 200.0,
            h: 100.0,
        };
        let (x, y) = widget_to_normalized((0.0, 0.0), &display);
        close(x, 0.0);
        close(y, 0.0);

        let (x, y) = widget_to_normalized((1000.0, 1000.0), &display);
        close(x, 1.0);
        close(y, 1.0);
    }

    #[test]
    fn zero_sized_inputs_have_no_rectangle() {
        assert_eq!(letterbox((0.0, 600.0), (800, 600)), None);
        assert_eq!(letterbox((800.0, 0.0), (800, 600)), None);
        assert_eq!(letterbox((800.0, 600.0), (0, 600)), None);
        assert_eq!(letterbox((800.0, 600.0), (800, 0)), None);
        assert_eq!(letterbox((f64::NAN, 600.0), (800, 600)), None);
        assert_eq!(letterbox((f64::INFINITY, 600.0), (800, 600)), None);
    }

    #[test]
    fn a_zero_size_display_rect_does_not_divide_by_zero() {
        let display = DisplayRect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        let (x, y) = widget_to_normalized((10.0, 10.0), &display);
        close(x, 0.0);
        close(y, 0.0);
    }

    /// An arbitrary displayed-image rectangle.
    fn rect(x: f64, y: f64, w: f64, h: f64) -> DisplayRect {
        DisplayRect { x, y, w, h }
    }

    #[test]
    fn a_point_inside_the_display_rect_is_contained() {
        assert!(contains(&rect(100.0, 50.0, 200.0, 100.0), (200.0, 100.0)));
    }

    #[test]
    fn the_edges_of_the_display_rect_are_inclusive() {
        let display = rect(100.0, 50.0, 200.0, 100.0);
        // The four corners.
        assert!(contains(&display, (100.0, 50.0)));
        assert!(contains(&display, (300.0, 150.0)));
        // A point on each edge.
        assert!(contains(&display, (100.0, 100.0)));
        assert!(contains(&display, (300.0, 100.0)));
        assert!(contains(&display, (200.0, 50.0)));
        assert!(contains(&display, (200.0, 150.0)));
    }

    #[test]
    fn a_point_just_outside_on_each_side_is_not_contained() {
        let display = rect(100.0, 50.0, 200.0, 100.0);
        assert!(!contains(&display, (99.999, 100.0)));
        assert!(!contains(&display, (300.001, 100.0)));
        assert!(!contains(&display, (200.0, 49.999)));
        assert!(!contains(&display, (200.0, 150.001)));
    }

    #[test]
    fn a_click_in_the_letterbox_bars_is_not_contained() {
        // A wide widget (1600x600) showing an 800x600 image: the image sits at
        // x = 400, so anything in a side bar is outside it.
        let display = letterbox((1600.0, 600.0), (800, 600)).unwrap();
        assert!(contains(&display, (display.x, display.y)));
        assert!(contains(
            &display,
            (display.x + display.w, display.y + display.h)
        ));
        assert!(!contains(&display, (0.0, 300.0)));
    }

    #[test]
    fn a_zero_size_rect_contains_only_its_origin() {
        let display = rect(10.0, 10.0, 0.0, 0.0);
        assert!(contains(&display, (10.0, 10.0)));
        assert!(!contains(&display, (10.001, 10.0)));
    }
}
