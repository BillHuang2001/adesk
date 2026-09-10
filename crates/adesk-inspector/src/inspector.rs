//! The [`Inspector`]: composes overlays onto a base frame.

use adesk_core::{ImageBuffer, OverlayKind};

use crate::canvas::Canvas;
use crate::error::{Error, Result};
use crate::input::InspectionInput;
use crate::paint;
use crate::post;
use crate::source::{InspectionRequest, InspectionSource};
use crate::style::OverlayStyle;

/// Composes debug overlays onto a full-output frame.
///
/// `overlays` is treated as a **set**: composition always follows
/// [`CANONICAL_ORDER`](crate::CANONICAL_ORDER), so caller order and duplicates
/// cannot change the output. `render` clones the input frame, paints each
/// requested overlay, and returns the composed frame.
#[derive(Debug, Clone)]
pub struct Inspector {
    /// Requested overlay kinds; normalized on construction and re-normalized on
    /// every render, so direct mutation cannot break determinism.
    pub overlays: Vec<OverlayKind>,
    /// Drawing style applied to every overlay.
    pub style: OverlayStyle,
}

impl Default for Inspector {
    /// The protocol default overlay set (`docs/protocol.md` §5.7):
    /// `window_ids`, `focus`, `damage`.
    fn default() -> Inspector {
        Inspector::new(vec![
            OverlayKind::WindowIds,
            OverlayKind::Focus,
            OverlayKind::Damage,
        ])
    }
}

impl Inspector {
    /// Creates an inspector for `overlays` (deduplicated, canonical order).
    pub fn new(overlays: Vec<OverlayKind>) -> Inspector {
        Inspector {
            overlays: paint::normalize(&overlays),
            style: OverlayStyle::default(),
        }
    }

    /// The normalized overlay set this inspector draws.
    pub fn overlays(&self) -> &[OverlayKind] {
        &self.overlays
    }

    /// The overlay set re-normalized (canonical order, duplicates dropped).
    pub fn normalized_overlays(&self) -> Vec<OverlayKind> {
        paint::normalize(&self.overlays)
    }

    /// Composes `input`'s frame and returns a new frame of the same size.
    pub fn render(&self, input: &InspectionInput) -> Result<ImageBuffer> {
        let mut out = input.frame.clone();
        tracing::debug!(
            width = input.frame.width,
            height = input.frame.height,
            overlays = self.overlays.len(),
            "composing inspection frame"
        );
        self.render_into(input, &mut out)?;
        Ok(out)
    }

    /// Composes `input` into `out`, which must have the same size as
    /// `input.frame`; `out` is overwritten.
    pub fn render_into(&self, input: &InspectionInput, out: &mut ImageBuffer) -> Result<()> {
        if out.size() != input.frame.size() {
            return Err(Error::InvalidFrame(format!(
                "target {}x{} does not match the {}x{} inspection frame",
                out.width, out.height, input.frame.width, input.frame.height
            )));
        }
        if out.data.len() != input.frame.data.len() {
            return Err(Error::InvalidFrame(format!(
                "target {} bytes does not match the {} byte inspection frame",
                out.data.len(),
                input.frame.data.len()
            )));
        }
        out.data.copy_from_slice(&input.frame.data);
        let mut canvas = Canvas::new(out);
        for kind in self.normalized_overlays() {
            paint::overlay(kind, &mut canvas, input, &self.style);
        }
        Ok(())
    }

    /// Composes `input`, then applies `request` (crop, then downscale) through
    /// `adesk-render`.
    pub fn render_request(
        &self,
        input: &InspectionInput,
        request: &InspectionRequest,
    ) -> Result<ImageBuffer> {
        let composed = self.render(input)?;
        post::apply(composed, request)
    }

    /// Convenience path for the server: snapshot through `source`, then render.
    pub fn render_from_source(&self, source: &dyn InspectionSource) -> Result<ImageBuffer> {
        let input = source.inspection_input(&self.overlays)?;
        self.render(&input)
    }
}
