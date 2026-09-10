//! Per-overlay painters.
//!
//! Every painter has the same signature — `(&mut Canvas, &InspectionInput,
//! &OverlayStyle)` — and is independent: it reads only the input and the style,
//! draws through the canvas, and never touches another overlay's state.
//! [`overlay`] dispatches by kind; [`CANONICAL_ORDER`] fixes the bottom-to-top
//! composition order used by [`Inspector::render`](crate::Inspector::render).
//!
//! Painters preserve input order (windows, damage rects and action markers are
//! drawn in the order the server supplies them) so composition is deterministic
//! for a given [`InspectionInput`].

mod actions;
mod app_ids;
mod commit_timing;
mod common;
mod cursor;
mod damage;
mod focus;
mod labels;
mod surface_bounds;
mod window_ids;

pub use actions::paint as actions;
pub use app_ids::paint as app_ids;
pub use commit_timing::paint as commit_timing;
pub use cursor::paint as cursor;
pub use damage::paint as damage;
pub use focus::paint as focus;
pub use surface_bounds::paint as surface_bounds;
pub use window_ids::paint as window_ids;

use adesk_core::OverlayKind;

use crate::canvas::Canvas;
use crate::input::InspectionInput;
use crate::style::OverlayStyle;

/// Canonical bottom-to-top composition order of all overlays.
///
/// Translucent fills come first, window geometry next, labels after geometry,
/// and the HUD-style annotations on top. This order is independent of the
/// caller's `overlays` vector.
pub const CANONICAL_ORDER: [OverlayKind; 8] = [
    OverlayKind::Damage,
    OverlayKind::SurfaceBounds,
    OverlayKind::WindowIds,
    OverlayKind::AppIds,
    OverlayKind::Focus,
    OverlayKind::Cursor,
    OverlayKind::Actions,
    OverlayKind::CommitTiming,
];

/// Position of `kind` in [`CANONICAL_ORDER`].
///
/// Derived from [`CANONICAL_ORDER`] so the canonical order has a single source
/// of truth. Every [`OverlayKind`] variant appears in the array, so the scan
/// always returns; the trailing `len` is an unreachable total-function fallback.
pub const fn order_index(kind: OverlayKind) -> usize {
    let mut index = 0;
    while index < CANONICAL_ORDER.len() {
        if CANONICAL_ORDER[index] as usize == kind as usize {
            return index;
        }
        index += 1;
    }
    CANONICAL_ORDER.len()
}

/// Normalizes an overlay set: duplicates removed, canonical order.
pub fn normalize(overlays: &[OverlayKind]) -> Vec<OverlayKind> {
    let mut kinds = overlays.to_vec();
    kinds.sort_by_key(|kind| order_index(*kind));
    kinds.dedup();
    kinds
}

/// Paints one overlay kind through `canvas`.
pub fn overlay(
    kind: OverlayKind,
    canvas: &mut Canvas<'_>,
    input: &InspectionInput,
    style: &OverlayStyle,
) {
    match kind {
        OverlayKind::Damage => damage(canvas, input, style),
        OverlayKind::SurfaceBounds => surface_bounds(canvas, input, style),
        OverlayKind::WindowIds => window_ids(canvas, input, style),
        OverlayKind::AppIds => app_ids(canvas, input, style),
        OverlayKind::Focus => focus(canvas, input, style),
        OverlayKind::Cursor => cursor(canvas, input, style),
        OverlayKind::Actions => actions(canvas, input, style),
        OverlayKind::CommitTiming => commit_timing(canvas, input, style),
    }
}
