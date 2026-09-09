//! AGP §5.4 — capture and temporal observation.
//!
//! Observation semantics (binding, protocol §5.4 and `docs/architecture.md` §6):
//! a filter point (`after_action` / `since_commit`) and an optional window
//! select which events count; the result reports what accumulated and whether
//! the condition was met. Surface quietness is **evidence**, never a promise of
//! semantic completion — the agent reasons about it.

use adesk_core::{ActionId, Observation, Rect, WindowId, WindowInfo};
use serde::{Deserialize, Serialize};

use crate::{Client, ImageFormat, ImagePayload, Result};

/// Default `timeout_ms` for `observe` / `wait_for_*` (protocol §5.4).
pub const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// `capture_window` params (protocol §5.4).
///
/// `region` crops inside the window; `max_dimension` box-downscales so the
/// longer edge is at most that many pixels; `format` selects PNG (default) or
/// raw RGBA8.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct CaptureRequest {
    /// Window to capture.
    pub window_id: WindowId,
    /// Optional window-relative crop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
    /// Optional downscale bound for the longer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Wire format of the returned payload.
    pub format: ImageFormat,
}

impl CaptureRequest {
    /// Capture the whole window as PNG.
    pub fn window(window_id: WindowId) -> Self {
        Self { window_id, region: None, max_dimension: None, format: ImageFormat::Png }
    }

    /// Crop to `region` (window-relative).
    pub fn region(mut self, region: Rect) -> Self {
        self.region = Some(region);
        self
    }

    /// Downscale so the longer edge is at most `max_dimension`.
    pub fn max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }

    /// Request a specific wire format.
    pub fn format(mut self, format: ImageFormat) -> Self {
        self.format = format;
        self
    }
}

/// `capture_region` params (protocol §5.4) — the region is mandatory here.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct CaptureRegionRequest {
    /// Window to capture.
    pub window_id: WindowId,
    /// Window-relative region to capture.
    pub region: Rect,
    /// Optional downscale bound for the longer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Wire format of the returned payload.
    pub format: ImageFormat,
}

impl CaptureRegionRequest {
    /// Capture `region` of `window_id` as PNG.
    pub fn new(window_id: WindowId, region: Rect) -> Self {
        Self { window_id, region, max_dimension: None, format: ImageFormat::Png }
    }

    /// Downscale so the longer edge is at most `max_dimension`.
    pub fn max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }

    /// Request a specific wire format.
    pub fn format(mut self, format: ImageFormat) -> Self {
        self.format = format;
        self
    }
}

/// Result of `capture_window` / `capture_region` (protocol §5.4).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct CaptureResult {
    /// Rendered pixels.
    pub image: ImagePayload,
    /// Window state at capture time.
    pub window: WindowInfo,
    /// Commit watermark the frame was rendered from.
    pub commit_seq: u64,
    /// Damage regions accumulated since the previous capture (window-relative).
    #[serde(default)]
    pub changed_regions: Vec<Rect>,
}

impl CaptureResult {
    /// Decode [`CaptureResult::image`] into an
    /// [`ImageBuffer`](adesk_core::ImageBuffer).
    pub fn decode_image(&self) -> Result<adesk_core::ImageBuffer> {
        crate::decode_image(&self.image)
    }
}

/// Observation condition for `observe` (protocol §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Condition {
    /// Return once `quiet_ms` elapsed with no counted surface commit.
    Quiet {
        /// Required quiet period in milliseconds.
        quiet_ms: u64,
    },
    /// Return on the first counted commit or window lifecycle event.
    Change,
    /// Wait the full `timeout_ms` and report what accumulated.
    Timeout,
}

/// `observe` params (protocol §5.4).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ObserveRequest {
    /// Restrict counting to one window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Count only events after this action (protocol §5.4 filter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_action: Option<ActionId>,
    /// What to wait for.
    pub until: Condition,
    /// Upper bound on the wait.
    pub timeout_ms: u64,
    /// Render the window once the condition resolves.
    pub include_image: bool,
    /// Optional downscale bound for the returned image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    /// Optional crop for the returned image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
}

impl ObserveRequest {
    /// Observe until the window is quiet for `quiet_ms`.
    pub fn quiet(quiet_ms: u64) -> Self {
        Self::new(Condition::Quiet { quiet_ms })
    }

    /// Observe until the next counted change.
    pub fn change() -> Self {
        Self::new(Condition::Change)
    }

    /// Observe for the whole timeout (sampling animations).
    pub fn timeout() -> Self {
        Self::new(Condition::Timeout)
    }

    fn new(until: Condition) -> Self {
        Self {
            window_id: None,
            after_action: None,
            until,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            include_image: true,
            max_dimension: None,
            region: None,
        }
    }

    /// Restrict to one window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Count only events causally after `action_id`.
    pub fn after_action(mut self, action_id: ActionId) -> Self {
        self.after_action = Some(action_id);
        self
    }

    /// Override the wait bound.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Enable/disable the returned image.
    pub fn include_image(mut self, include_image: bool) -> Self {
        self.include_image = include_image;
        self
    }

    /// Downscale the returned image's longer edge.
    pub fn max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }

    /// Crop the returned image.
    pub fn region(mut self, region: Rect) -> Self {
        self.region = Some(region);
        self
    }
}

/// Result of `observe` (protocol §5.4 + `adesk-proto::ObserveResult`).
///
/// The protocol puts the optional image inside the `Observation` object
/// (§4); the client splits it out for ergonomics and accepts either layout.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ObserveResult {
    /// Temporal summary of what happened.
    pub observation: Observation,
    /// Rendered window at resolution time, when `include_image` was set.
    #[serde(default)]
    pub image: Option<ImagePayload>,
}

impl ObserveResult {
    /// Decode [`ObserveResult::image`] into an
    /// [`ImageBuffer`](adesk_core::ImageBuffer), if present.
    pub fn decode_image(&self) -> Option<Result<adesk_core::ImageBuffer>> {
        self.image.as_ref().map(crate::decode_image)
    }
}

/// `wait_for_change` params (protocol §5.4).
///
/// This convenience helper never requests pixels; use
/// [`ObserveRequest::change`] with `include_image(true)` when an image is
/// needed.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WaitForChangeRequest {
    /// Restrict counting to one window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Count only commits after this per-window commit counter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_commit: Option<u64>,
    /// Upper bound on the wait.
    pub timeout_ms: u64,
}

impl Default for WaitForChangeRequest {
    fn default() -> Self {
        Self { window_id: None, since_commit: None, timeout_ms: DEFAULT_TIMEOUT_MS }
    }
}

impl WaitForChangeRequest {
    /// Restrict to one window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Count only commits with `commit_seq > since_commit`.
    pub fn since_commit(mut self, since_commit: u64) -> Self {
        self.since_commit = Some(since_commit);
        self
    }

    /// Override the wait bound.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

/// `wait_for_quiet` params (protocol §5.4).
///
/// Like [`WaitForChangeRequest`], this helper never requests pixels.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WaitForQuietRequest {
    /// Restrict counting to one window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Required quiet period in milliseconds.
    pub quiet_ms: u64,
    /// Upper bound on the wait.
    pub timeout_ms: u64,
    /// Count only events causally after this action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_action: Option<ActionId>,
}

impl Default for WaitForQuietRequest {
    fn default() -> Self {
        Self { window_id: None, quiet_ms: 250, timeout_ms: DEFAULT_TIMEOUT_MS, after_action: None }
    }
}

impl WaitForQuietRequest {
    /// Restrict to one window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Override the required quiet period.
    pub fn quiet_ms(mut self, quiet_ms: u64) -> Self {
        self.quiet_ms = quiet_ms;
        self
    }

    /// Override the wait bound.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Count only events causally after `action_id`.
    pub fn after_action(mut self, action_id: ActionId) -> Self {
        self.after_action = Some(action_id);
        self
    }
}

/// `capture_window` / `capture_region` result envelope (`{"image", "window", ..}`).
#[derive(Debug, Clone, Deserialize)]
struct CaptureEnvelope {
    image: ImagePayload,
    window: WindowInfo,
    commit_seq: u64,
    #[serde(default)]
    changed_regions: Vec<Rect>,
}

/// `observe` result envelope; tolerates `image` inside the observation object.
#[derive(Debug, Clone, Deserialize)]
struct ObserveEnvelope {
    observation: ObservationWithImage,
}

/// Core [`Observation`] plus the optional image the protocol attaches to it.
#[derive(Debug, Clone, Deserialize)]
struct ObservationWithImage {
    #[serde(flatten)]
    observation: Observation,
    #[serde(default)]
    image: Option<ImagePayload>,
}

/// `wait_for_change` / `wait_for_quiet` result envelope.
#[derive(Debug, Clone, Deserialize)]
struct ObservationEnvelope {
    observation: Observation,
}

impl Client {
    /// `capture_window` — render the whole window (or `request.region`).
    ///
    /// Rendering happens on demand; the returned image is a fresh readback, not
    /// a cached frame (design invariant 5).
    pub async fn capture_window(&self, request: CaptureRequest) -> Result<CaptureResult> {
        let envelope: CaptureEnvelope = self.request("capture_window", &request).await?;
        Ok(CaptureResult {
            image: envelope.image,
            window: envelope.window,
            commit_seq: envelope.commit_seq,
            changed_regions: envelope.changed_regions,
        })
    }

    /// `capture_region` — render an explicit window-relative region.
    pub async fn capture_region(&self, request: CaptureRegionRequest) -> Result<CaptureResult> {
        let envelope: CaptureEnvelope = self.request("capture_region", &request).await?;
        Ok(CaptureResult {
            image: envelope.image,
            window: envelope.window,
            commit_seq: envelope.commit_seq,
            changed_regions: envelope.changed_regions,
        })
    }

    /// `observe` — wait for a condition and report causal history since a
    /// filter point.
    ///
    /// The runtime performs the wait; the client only sends the request. When
    /// `include_image` is set the image reflects the settled state, because the
    /// server renders *after* the condition resolves (`docs/architecture.md`
    /// §6).
    pub async fn observe(&self, request: ObserveRequest) -> Result<ObserveResult> {
        let envelope: ObserveEnvelope = self.request("observe", &request).await?;
        Ok(ObserveResult { observation: envelope.observation.observation, image: envelope.observation.image })
    }

    /// `wait_for_change` — resolve on the first counted change.
    ///
    /// `timed_out = true` in the result means the wait expired; it is not an
    /// error.
    pub async fn wait_for_change(&self, request: WaitForChangeRequest) -> Result<Observation> {
        let envelope: ObservationEnvelope = self.request("wait_for_change", &request).await?;
        Ok(envelope.observation)
    }

    /// `wait_for_quiet` — resolve once the window stayed quiet for `quiet_ms`.
    ///
    /// Quiet is surface-level evidence, not proof that the application finished
    /// working (design invariant 4).
    pub async fn wait_for_quiet(&self, request: WaitForQuietRequest) -> Result<Observation> {
        let envelope: ObservationEnvelope = self.request("wait_for_quiet", &request).await?;
        Ok(envelope.observation)
    }
}
