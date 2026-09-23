//! Pure remote-cursor placement for the frame view's overlay (GTK-free).
//!
//! A VAP frame (and the handshake) carries the remote pointer position as
//! **normalized** `0.0..=1.0` output fractions (`CursorState`), never pixels
//! (`docs/viewer.md` §3). To draw that pointer on top of the displayed desktop
//! the viewer must resolve the fraction against the same aspect-preserving,
//! centered rectangle the click mapping uses — so this module reuses
//! [`crate::mapping::DisplayRect`] instead of repeating the letterbox math.
//!
//! [`overlay_position`] does that arithmetic; [`CursorOverlay`] holds the state
//! it is applied to, merging the local pointer motion (which the runtime does not
//! echo back in a frame) with the server's per-frame `CursorState`.
//!
//! Nothing here is GTK-aware, so every case (identity, letterbox, pillarbox,
//! clamping, hidden, non-finite, motion-versus-frame precedence) is unit-tested
//! without a display.

use adesk_viewer_proto::CursorState;

use crate::mapping::DisplayRect;

/// The widget-pixel point the remote pointer's tip is drawn at, or [`None`]
/// when there is nothing to draw (a hidden cursor, a non-finite position or a
/// collapsed displayed-image rectangle).
///
/// `display` is the displayed desktop image's rectangle inside the frame widget
/// (in widget pixels); the cursor's fraction is resolved against it and clamped
/// into `0.0..=1.0`, so a runtime reporting an out-of-range fraction still
/// yields a point inside the picture rather than one in the letterbox bars.
#[must_use]
pub(crate) fn overlay_position(display: &DisplayRect, cursor: &CursorState) -> Option<(f64, f64)> {
    if !cursor.visible {
        return None;
    }
    if !cursor.x.is_finite() || !cursor.y.is_finite() {
        return None;
    }
    if !display.w.is_finite() || !display.h.is_finite() || display.w <= 0.0 || display.h <= 0.0 {
        return None;
    }
    Some((
        display.x + cursor.x.clamp(0.0, 1.0) * display.w,
        display.y + cursor.y.clamp(0.0, 1.0) * display.h,
    ))
}

/// Owns the remote-pointer state the overlay currently draws.
///
/// The drawn pointer has two sources and **the latest update wins**: a local
/// pointer motion (placed at the normalized fraction the click mapping produced)
/// and a pushed frame's server-reported [`CursorState`]. The runtime pushes a
/// frame only on a desktop change — never on pointer motion — so without the
/// local source the drawn pointer would stay frozen between frames while the
/// runtime's own pointer moved.
///
/// The in/out of range arithmetic is still [`overlay_position`]; this type only
/// decides *which* [`CursorState`] that math is applied to, and whether a change
/// is worth a repaint. It is GTK-free so both decisions are unit-tested without a
/// display.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CursorOverlay {
    /// The cursor state currently drawn (hidden until the first update).
    state: CursorState,
}

impl CursorOverlay {
    /// A new overlay that draws nothing (the pointer starts hidden).
    pub(crate) fn new() -> CursorOverlay {
        CursorOverlay {
            state: CursorState::hidden(),
        }
    }

    /// Records a local pointer motion to the normalized output fraction `(x, y)`.
    ///
    /// The drawn pointer becomes a *visible* cursor at that fraction. A non-finite
    /// fraction is rejected and leaves the drawn state untouched (there is no
    /// point to draw). Returns whether the drawn state changed, so the caller
    /// repaints only on a real change.
    pub(crate) fn motion(&mut self, x: f64, y: f64) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        self.replace(CursorState::at(x, y))
    }

    /// Replaces the drawn pointer with the server's `cursor` for a pushed frame
    /// (which may be hidden). Returns whether the drawn state changed.
    pub(crate) fn frame(&mut self, cursor: &CursorState) -> bool {
        self.replace(cursor.clone())
    }

    /// The [`CursorState`] to draw.
    pub(crate) fn state(&self) -> &CursorState {
        &self.state
    }

    /// Stores `next` and reports whether it differs from the drawn state.
    fn replace(&mut self, next: CursorState) -> bool {
        if self.state == next {
            return false;
        }
        self.state = next;
        true
    }
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

    /// An arbitrary displayed-image rectangle (no letterboxing).
    fn rect(x: f64, y: f64, w: f64, h: f64) -> DisplayRect {
        DisplayRect { x, y, w, h }
    }

    #[test]
    fn an_identity_rect_maps_the_fraction_directly() {
        let point = overlay_position(&rect(0.0, 0.0, 800.0, 600.0), &CursorState::at(0.25, 0.5))
            .expect("a visible cursor has a position");
        close(point.0, 200.0);
        close(point.1, 300.0);
    }

    #[test]
    fn the_corners_are_the_rect_corners() {
        let display = rect(0.0, 0.0, 800.0, 600.0);
        let top_left = overlay_position(&display, &CursorState::at(0.0, 0.0)).unwrap();
        close(top_left.0, 0.0);
        close(top_left.1, 0.0);

        let bottom_right = overlay_position(&display, &CursorState::at(1.0, 1.0)).unwrap();
        close(bottom_right.0, 800.0);
        close(bottom_right.1, 600.0);
    }

    #[test]
    fn a_letterboxed_image_offsets_the_cursor_by_the_bar() {
        // A wide widget (1600x600) showing an 800x600 image: the image sits at
        // x = 400, so the output's left edge is 400 widget pixels in.
        let display = rect(400.0, 0.0, 800.0, 600.0);
        let top_left = overlay_position(&display, &CursorState::at(0.0, 0.0)).unwrap();
        close(top_left.0, 400.0);
        close(top_left.1, 0.0);

        let center = overlay_position(&display, &CursorState::at(0.5, 0.5)).unwrap();
        close(center.0, 800.0);
        close(center.1, 300.0);
    }

    #[test]
    fn a_pillarboxed_image_offsets_the_cursor_by_the_bar() {
        // A tall widget (400x800) showing an 800x600 image: the image sits at
        // y = 250 and is scaled to 400x300.
        let display = rect(0.0, 250.0, 400.0, 300.0);
        let center = overlay_position(&display, &CursorState::at(0.5, 0.5)).unwrap();
        close(center.0, 200.0);
        close(center.1, 400.0);

        let bottom_right = overlay_position(&display, &CursorState::at(1.0, 1.0)).unwrap();
        close(bottom_right.0, 400.0);
        close(bottom_right.1, 550.0);
    }

    #[test]
    fn a_hidden_cursor_has_no_position() {
        assert_eq!(
            overlay_position(&rect(0.0, 0.0, 800.0, 600.0), &CursorState::hidden()),
            None
        );
    }

    #[test]
    fn a_fraction_outside_the_output_clamps_into_the_rect() {
        let display = rect(100.0, 50.0, 200.0, 100.0);

        let top_left = overlay_position(&display, &CursorState::at(-0.5, -3.0)).unwrap();
        close(top_left.0, 100.0);
        close(top_left.1, 50.0);

        let bottom_right = overlay_position(&display, &CursorState::at(1.5, 7.0)).unwrap();
        close(bottom_right.0, 300.0);
        close(bottom_right.1, 150.0);
    }

    #[test]
    fn a_non_finite_position_has_no_position() {
        let display = rect(0.0, 0.0, 800.0, 600.0);
        assert_eq!(
            overlay_position(&display, &CursorState::at(f64::NAN, 0.5)),
            None
        );
        assert_eq!(
            overlay_position(&display, &CursorState::at(0.5, f64::INFINITY)),
            None
        );
    }

    #[test]
    fn a_collapsed_display_rect_has_no_position() {
        let cursor = CursorState::at(0.5, 0.5);
        assert_eq!(overlay_position(&rect(0.0, 0.0, 0.0, 600.0), &cursor), None);
        assert_eq!(overlay_position(&rect(0.0, 0.0, 800.0, 0.0), &cursor), None);
        assert_eq!(
            overlay_position(&rect(0.0, 0.0, f64::NAN, 600.0), &cursor),
            None
        );
    }

    #[test]
    fn a_new_overlay_draws_nothing() {
        assert!(!CursorOverlay::new().state().visible);
    }

    #[test]
    fn a_motion_makes_the_pointer_visible_and_reports_the_change() {
        let mut overlay = CursorOverlay::new();
        assert!(overlay.motion(0.25, 0.5));

        let state = overlay.state();
        assert!(state.visible);
        close(state.x, 0.25);
        close(state.y, 0.5);
    }

    #[test]
    fn a_repeated_identical_motion_reports_no_change() {
        let mut overlay = CursorOverlay::new();
        assert!(overlay.motion(0.25, 0.5));
        assert!(!overlay.motion(0.25, 0.5));
    }

    #[test]
    fn a_frame_replaces_a_local_position() {
        let mut overlay = CursorOverlay::new();
        overlay.motion(0.25, 0.5);

        assert!(overlay.frame(&CursorState::at(0.75, 0.1)));
        let state = overlay.state();
        close(state.x, 0.75);
        close(state.y, 0.1);
    }

    #[test]
    fn a_frame_reporting_a_hidden_cursor_hides_it() {
        let mut overlay = CursorOverlay::new();
        overlay.motion(0.25, 0.5);

        assert!(overlay.frame(&CursorState::hidden()));
        assert!(!overlay.state().visible);
    }

    #[test]
    fn a_motion_after_a_hidden_frame_shows_the_pointer_again() {
        let mut overlay = CursorOverlay::new();
        overlay.frame(&CursorState::hidden());

        assert!(overlay.motion(0.4, 0.6));
        assert!(overlay.state().visible);
    }

    #[test]
    fn an_identical_frame_reports_no_change() {
        let mut overlay = CursorOverlay::new();
        assert!(overlay.frame(&CursorState::at(0.3, 0.7)));
        assert!(!overlay.frame(&CursorState::at(0.3, 0.7)));
    }

    #[test]
    fn a_non_finite_motion_is_rejected() {
        let mut overlay = CursorOverlay::new();
        assert!(!overlay.motion(f64::NAN, 0.5));
        assert!(!overlay.state().visible);
        assert!(!overlay.motion(0.5, f64::INFINITY));
        assert!(!overlay.state().visible);
    }

    #[test]
    fn a_non_finite_motion_leaves_the_previous_state() {
        let mut overlay = CursorOverlay::new();
        overlay.motion(0.2, 0.3);

        assert!(!overlay.motion(f64::NAN, f64::NAN));
        let state = overlay.state();
        assert!(state.visible);
        close(state.x, 0.2);
        close(state.y, 0.3);
    }
}
