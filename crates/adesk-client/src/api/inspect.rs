//! AGP §5.7 — human inspector.
//!
//! Overlays are debug-only: agent-facing `capture_*` never includes them
//! (protocol §5.7). Use these methods to *see* what the runtime sees when
//! debugging an agent, not to feed pixels to an agent.

use adesk_core::{OverlayKind, Rect};
use serde::{Deserialize, Serialize};

use crate::events::InspectStream;
use crate::{Client, ImagePayload, Result};

/// The three overlays the protocol enables by default (protocol §5.7).
pub const DEFAULT_OVERLAYS: [OverlayKind; 3] = [
    OverlayKind::WindowIds,
    OverlayKind::Focus,
    OverlayKind::Damage,
];

/// `inspect_capture` params (protocol §5.7).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct InspectCaptureRequest {
    /// Overlays to draw.
    pub overlays: Vec<OverlayKind>,
    /// Optional output-relative crop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
    /// Optional downscale bound for the longer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
}

impl Default for InspectCaptureRequest {
    /// `window_ids` + `focus` + `damage` over the whole output.
    fn default() -> Self {
        Self {
            overlays: DEFAULT_OVERLAYS.to_vec(),
            region: None,
            max_dimension: None,
        }
    }
}

impl InspectCaptureRequest {
    /// Request a specific overlay set.
    pub fn overlays(overlays: impl IntoIterator<Item = OverlayKind>) -> Self {
        Self {
            overlays: overlays.into_iter().collect(),
            region: None,
            max_dimension: None,
        }
    }

    /// Crop the composed output.
    pub fn region(mut self, region: Rect) -> Self {
        self.region = Some(region);
        self
    }

    /// Downscale so the longer edge is at most `max_dimension`.
    pub fn max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }
}

/// `inspect_subscribe` params (protocol §5.7).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct InspectSubscribeRequest {
    /// Overlays to draw on every pushed frame.
    pub overlays: Vec<OverlayKind>,
    /// Minimum interval between pushed frames.
    pub min_interval_ms: u64,
}

impl InspectSubscribeRequest {
    /// Default interval (100 ms) with the given overlays.
    pub fn new(overlays: impl IntoIterator<Item = OverlayKind>) -> Self {
        Self {
            overlays: overlays.into_iter().collect(),
            min_interval_ms: 100,
        }
    }

    /// Override the push interval.
    pub fn min_interval_ms(mut self, min_interval_ms: u64) -> Self {
        self.min_interval_ms = min_interval_ms;
        self
    }
}

/// `inspect_capture` result.
#[derive(Debug, Clone, Deserialize)]
struct InspectCaptureResult {
    image: ImagePayload,
}

impl Client {
    /// `inspect_capture` — one composed output frame with debug overlays.
    pub async fn inspect_capture(&self, request: InspectCaptureRequest) -> Result<ImagePayload> {
        let result: InspectCaptureResult = self.request("inspect_capture", &request).await?;
        Ok(result.image)
    }

    /// `inspect_subscribe` — periodic composed frames with debug overlays.
    ///
    /// Frames arrive as `inspect_frame` events; the returned [`InspectStream`]
    /// yields them typed and unsubscribes on drop.
    pub async fn inspect_subscribe(
        &self,
        request: InspectSubscribeRequest,
    ) -> Result<InspectStream> {
        let result: crate::api::subscribe::SubscribeResult =
            self.request("inspect_subscribe", &request).await?;
        Ok(InspectStream::new(
            self.inner.subscribe(),
            result.subscription_id,
            self.inner.clone(),
        ))
    }
}
