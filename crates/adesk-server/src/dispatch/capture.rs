//! §5.4 capture and observation methods.
//!
//! Captures render on demand (`RuntimeCommand::RenderWindow`); observation
//! methods await the observer **first** and only then render, so the attached
//! image matches the observation the client receives (`docs/architecture.md` §6).
//! Waits time out as *observations* (`timed_out: true`), never as errors.

use adesk_proto::{
    CaptureRegionParams, CaptureResult, CaptureWindowParams, ObserveParams, ObserveResult,
    WaitForChangeParams, WaitForQuietParams,
};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `capture_window`: render a window's current pixels (crop/downscale optional).
pub async fn capture_window(
    ctx: &RequestContext<'_>,
    params: CaptureWindowParams,
) -> Result<CaptureResult> {
    todo!()
}

/// `capture_region`: render a required sub-rect of a window.
pub async fn capture_region(
    ctx: &RequestContext<'_>,
    params: CaptureRegionParams,
) -> Result<CaptureResult> {
    todo!()
}

/// `observe`: wait for a condition, then optionally attach an image.
pub async fn observe(ctx: &RequestContext<'_>, params: ObserveParams) -> Result<ObserveResult> {
    todo!()
}

/// `wait_for_change`: resolve on the first counted commit/lifecycle event.
pub async fn wait_for_change(
    ctx: &RequestContext<'_>,
    params: WaitForChangeParams,
) -> Result<ObserveResult> {
    todo!()
}

/// `wait_for_quiet`: resolve once the window has been quiet for `quiet_ms`.
pub async fn wait_for_quiet(
    ctx: &RequestContext<'_>,
    params: WaitForQuietParams,
) -> Result<ObserveResult> {
    todo!()
}
