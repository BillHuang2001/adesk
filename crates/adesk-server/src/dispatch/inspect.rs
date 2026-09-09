//! §5.7 human inspector methods.
//!
//! Both methods refresh [`crate::inspection::InspectionCache`] (full-output
//! render + `QueryState`), build an `InspectionInput` through the
//! `InspectionSource` impl, and let `adesk-inspector` composite overlays.
//! `inspect_capture` encodes the result as PNG; `inspect_subscribe` pushes
//! `inspect_frame` events, one `Inspector` reused per subscription and
//! throttled by `min_interval_ms`.

use adesk_proto::{InspectCaptureParams, InspectCaptureResult, InspectSubscribeParams, InspectSubscribeResult};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `inspect_capture`: one overlay-composited frame (PNG), crop/downscale applied
/// after overlays.
pub async fn inspect_capture(
    ctx: &RequestContext<'_>,
    params: InspectCaptureParams,
) -> Result<InspectCaptureResult> {
    todo!()
}

/// `inspect_subscribe`: stream overlay frames as `inspect_frame` events.
pub async fn inspect_subscribe(
    ctx: &RequestContext<'_>,
    params: InspectSubscribeParams,
) -> Result<InspectSubscribeResult> {
    todo!()
}
