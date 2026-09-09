//! Human inspector methods (§5.7). Debug-only; agent-facing `capture_*` never
//! includes overlays.

use adesk_core::{OverlayKind, Rect};
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::image::ImagePayload;

/// Params of `inspect_capture` (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectCaptureParams {
    /// Overlays to draw; defaults to `["window_ids","focus","damage"]`.
    #[serde(default = "defaults::inspect_overlays")]
    pub overlays: Vec<OverlayKind>,
    /// Output-relative crop, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
    /// Downscale so neither dimension exceeds this value, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
}

impl Default for InspectCaptureParams {
    fn default() -> InspectCaptureParams {
        InspectCaptureParams {
            overlays: defaults::inspect_overlays(),
            region: None,
            max_dimension: None,
        }
    }
}

/// Result of `inspect_capture` (§5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectCaptureResult {
    /// The composed output image with overlays.
    pub image: ImagePayload,
}

/// Params of `inspect_subscribe` (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectSubscribeParams {
    /// Overlays to draw on every pushed frame (required).
    pub overlays: Vec<OverlayKind>,
    /// Minimum interval between pushed frames in milliseconds (default `100`).
    #[serde(default = "defaults::min_interval_ms")]
    pub min_interval_ms: u64,
}

/// Result of `inspect_subscribe` (§5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectSubscribeResult {
    /// Id of the subscription pushing `inspect_frame` events.
    pub subscription_id: u64,
}
