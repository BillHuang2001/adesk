//! Capture and temporal observation methods (§5.4).

use adesk_core::{ActionId, Observation, Rect, WindowId, WindowInfo};
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::image::ImagePayload;
use crate::types::{Condition, ImageFormat};

/// Params of `capture_window` (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureWindowParams {
    /// Window to render.
    pub window_id: WindowId,
    /// Window-relative crop; the whole window when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
    /// Downscale so neither dimension exceeds this value, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Encoding of the returned image (default `"png"`).
    #[serde(default)]
    pub format: ImageFormat,
}

/// Params of `capture_region` (§5.4) — `region` is required here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRegionParams {
    /// Window to render.
    pub window_id: WindowId,
    /// Window-relative crop (required).
    pub region: Rect,
    /// Downscale so neither dimension exceeds this value, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Encoding of the returned image (default `"png"`).
    #[serde(default)]
    pub format: ImageFormat,
}

/// Result of `capture_window` and `capture_region` (§5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureResult {
    /// The rendered image.
    pub image: ImagePayload,
    /// Window state at capture time.
    pub window: WindowInfo,
    /// Commit counter of the surface tree the image was rendered from.
    pub commit_seq: u64,
    /// Damage regions accumulated in the window, window-relative.
    pub changed_regions: Vec<Rect>,
}

/// Params of `observe` (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveParams {
    /// Restrict counted events to this window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Only count events with `seq` greater than this action's seq, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_action: Option<ActionId>,
    /// The condition that resolves the observation (required).
    pub until: Condition,
    /// Upper bound on the wait (default `5000`).
    #[serde(default = "defaults::timeout_ms")]
    pub timeout_ms: u64,
    /// Render the window at resolution time and attach the image (default `true`).
    #[serde(default = "defaults::include_image")]
    pub include_image: bool,
    /// Downscale the attached image, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Crop the attached image, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
}

/// Params of `wait_for_change` (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitForChangeParams {
    /// Restrict counted events to this window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Only count commits with `commit_seq` greater than this, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_commit: Option<u64>,
    /// Upper bound on the wait (default `5000`).
    #[serde(default = "defaults::timeout_ms")]
    pub timeout_ms: u64,
    /// Attach an image at resolution time (default `false`).
    #[serde(default)]
    pub include_image: bool,
}

/// Params of `wait_for_quiet` (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitForQuietParams {
    /// Restrict counted events to this window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Milliseconds of surface quiet required (default `250`).
    #[serde(default = "defaults::quiet_ms")]
    pub quiet_ms: u64,
    /// Upper bound on the wait (default `5000`).
    #[serde(default = "defaults::timeout_ms")]
    pub timeout_ms: u64,
    /// Only count events with `seq` greater than this action's seq, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_action: Option<ActionId>,
    /// Attach an image at resolution time (default `false`).
    #[serde(default)]
    pub include_image: bool,
}

/// Result of `observe`, `wait_for_change` and `wait_for_quiet` (§5.4):
/// `{"observation": Observation}`.
///
/// The wire `observation` object is the §4 `Observation` — the core fields plus
/// `image` — so this type serializes as `{"observation": {<core fields>,
/// "image": <ImagePayload|null>}}` rather than as two sibling fields.
/// `image` is present as `null` when no image was requested or attached.
#[derive(Debug, Clone, PartialEq)]
pub struct ObserveResult {
    /// The temporal summary (core §4 fields).
    pub observation: Observation,
    /// Image attached at resolution time, when `include_image` was set.
    pub image: Option<ImagePayload>,
}

impl Serialize for ObserveResult {
    /// Emits `{"observation": {<core observation fields>, "image": <image|null>}}`.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        todo!()
    }
}

impl<'de> Deserialize<'de> for ObserveResult {
    /// Reads the §4 `observation` object and splits the core fields from `image`.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        todo!()
    }
}
