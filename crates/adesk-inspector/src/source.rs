//! The bridge between the runtime and the inspector.
//!
//! `adesk-server` implements [`InspectionSource`] over its shared state
//! (compositor handle, action registry, observer). This crate only ever calls
//! the trait, which is why it never depends on `adesk-compositor` or
//! `adesk-server`.

use adesk_core::{OverlayKind, Rect};

use crate::error::Result;
use crate::input::InspectionInput;

/// Post-processing parameters of an inspection frame, mirroring the
/// `inspect_capture` params in `docs/protocol.md` §5.7.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InspectionRequest {
    /// Window-relative (output-relative) crop region.
    pub region: Option<Rect>,
    /// Downscale so the largest dimension is at most this many pixels.
    pub max_dimension: Option<u32>,
}

impl InspectionRequest {
    /// A request that performs no post-processing.
    pub const IDENTITY: InspectionRequest = InspectionRequest {
        region: None,
        max_dimension: None,
    };

    /// A request with no post-processing.
    pub const fn new() -> InspectionRequest {
        InspectionRequest::IDENTITY
    }

    /// Sets the crop region.
    pub const fn region(mut self, region: Rect) -> Self {
        self.region = Some(region);
        self
    }

    /// Sets the downscale bound.
    pub const fn max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }

    /// Whether this request leaves the composed frame untouched.
    pub const fn is_identity(&self) -> bool {
        self.region.is_none() && self.max_dimension.is_none()
    }
}

/// Source of inspection state, implemented by `adesk-server`.
///
/// Implementations must be cheap to call and must not block the compositor:
/// they typically clone a state snapshot and return it. The returned frame is
/// the **full virtual output**, rendered without crop, so overlay coordinates
/// need no translation.
pub trait InspectionSource: Send + Sync {
    /// Snapshot the base frame and the overlay state needed for `overlays`.
    ///
    /// `overlays` is the set requested by the client; implementations may skip
    /// collecting state for overlays that are not present.
    fn inspection_input(&self, overlays: &[OverlayKind]) -> Result<InspectionInput>;
}
