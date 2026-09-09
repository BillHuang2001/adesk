//! §5.7 human inspector methods.
//!
//! Both methods refresh [`crate::inspection::InspectionCache`] (full-output
//! render + `QueryState`), build an `InspectionInput` through the
//! `InspectionSource` impl, and let `adesk-inspector` composite overlays.
//! `inspect_capture` encodes the result as PNG; `inspect_subscribe` pushes
//! `inspect_frame` events, one `Inspector` reused per subscription and
//! throttled by `min_interval_ms`.

use std::time::Duration;

use adesk_core::{Rect, Size};
use adesk_inspector::{InspectionRequest, InspectionSource, Inspector};
use adesk_proto::{
    EventFrame, EventKind, EventPayload, Frame, ImagePayload, InspectCaptureParams,
    InspectCaptureResult, InspectFrameEvent, InspectSubscribeParams, InspectSubscribeResult,
};
use tokio::sync::mpsc::error::TrySendError;

use crate::context::ServerContext;
use crate::dispatch::RequestContext;
use crate::error::Result;
use crate::images;
use crate::inspection::{self, InspectionSnapshot};
use crate::subscriptions::{EventSink, SubscriptionId};

/// `inspect_capture`: one overlay-composited frame (PNG), crop/downscale applied
/// after overlays.
pub async fn inspect_capture(
    ctx: &RequestContext<'_>,
    params: InspectCaptureParams,
) -> Result<InspectCaptureResult> {
    // `Inspector::new` canonicalizes the overlay set (dedup, canonical order); an
    // empty set is legal and renders the plain composition.
    let inspector = Inspector::new(params.overlays);
    let snapshot = refresh(ctx.server).await?;
    let input = ctx.server.inspection.inspection_input(inspector.overlays())?;
    let request = InspectionRequest {
        region: params.region,
        max_dimension: params.max_dimension,
    };
    // Overlays are composited at full resolution; crop and downscale happen here,
    // inside the inspector, so overlay coordinates never need translating.
    let buffer = inspector.render_request(&input, &request)?;
    let source = composed_source(snapshot.frame.rect(), params.region);
    let scale = super::capture::scale_from(source, &buffer);
    let png = images::encode_png(&buffer)?;
    Ok(InspectCaptureResult {
        image: ImagePayload::from_png(buffer.width, buffer.height, &png, scale),
    })
}

/// `inspect_subscribe`: stream overlay frames as `inspect_frame` events.
pub async fn inspect_subscribe(
    ctx: &RequestContext<'_>,
    params: InspectSubscribeParams,
) -> Result<InspectSubscribeResult> {
    // One `Inspector` per subscription: the overlay set is normalized once and
    // reused for every pushed frame.
    let inspector = Inspector::new(params.overlays.clone());
    let sink = super::session_sink(ctx.session)?;
    let subscription_id = ctx.server.inspect_subscriptions.subscribe(
        ctx.session.id(),
        params.overlays,
        params.min_interval_ms,
        sink.clone(),
    );
    ctx.session.add_subscription(subscription_id);
    // The loop outlives the request; it stops when the connection closes, the
    // subscription is removed, or the compositor can no longer render.
    tokio::spawn(inspect_loop(
        ctx.server.clone(),
        inspector,
        subscription_id,
        params.min_interval_ms,
        sink,
    ));
    Ok(InspectSubscribeResult { subscription_id })
}

/// The throttled push loop of one `inspect_subscribe` subscription.
async fn inspect_loop(
    context: ServerContext,
    inspector: Inspector,
    subscription_id: SubscriptionId,
    min_interval_ms: u64,
    sink: EventSink,
) {
    loop {
        if sink.is_closed() || !subscription_alive(&context, subscription_id) {
            return;
        }
        match render_frame(&context, &inspector).await {
            Ok((image, seq, ts_ms)) => {
                let frame = Frame::Event(EventFrame::new(
                    EventKind::InspectFrame,
                    seq,
                    ts_ms,
                    EventPayload::InspectFrame(InspectFrameEvent { subscription_id, image }),
                ));
                match sink.try_send(frame) {
                    Ok(()) => {}
                    // A slow consumer loses frames, never responses (§5.6).
                    Err(TrySendError::Full(_)) => tracing::debug!(
                        subscription = subscription_id,
                        "inspect frame dropped: outbound queue is full"
                    ),
                    Err(TrySendError::Closed(_)) => return,
                }
            }
            Err(error) => {
                // The compositor or the encoder is gone: nothing left to stream.
                tracing::debug!(
                    subscription = subscription_id,
                    %error,
                    "inspect stream stopped"
                );
                return;
            }
        }
        // `0` means "push every refresh" (no throttle); yielding still keeps the
        // loop cooperative, so a zero-interval subscription cannot starve the
        // rest of the runtime.
        if min_interval_ms > 0 {
            tokio::time::sleep(Duration::from_millis(min_interval_ms)).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}

/// Refreshes the inspection cache and returns the fresh snapshot.
///
/// `adesk-inspector` is synchronous and never blocks the compositor, so the
/// cache is the only bridge: the snapshot is stored before
/// `InspectionSource::inspection_input` reads it.
async fn refresh(context: &ServerContext) -> Result<InspectionSnapshot> {
    let snapshot = inspection::refresh(context).await?;
    context.inspection.store(snapshot.clone());
    Ok(snapshot)
}

/// Renders one composed inspection frame: refresh, overlay, encode PNG.
///
/// Returns the payload plus the snapshot's `seq`/`ts_ms`, which stamp the
/// `inspect_frame` event (§1).
async fn render_frame(
    context: &ServerContext,
    inspector: &Inspector,
) -> Result<(ImagePayload, u64, u64)> {
    let snapshot = refresh(context).await?;
    let input = context.inspection.inspection_input(inspector.overlays())?;
    let buffer = inspector.render_request(&input, &InspectionRequest::IDENTITY)?;
    let scale = super::capture::scale_from(snapshot.frame.size(), &buffer);
    let png = images::encode_png(&buffer)?;
    Ok((
        ImagePayload::from_png(buffer.width, buffer.height, &png, scale),
        snapshot.seq,
        snapshot.ts_ms,
    ))
}

/// Whether the subscription is still registered, so `unsubscribe_events` and a
/// disconnect stop the loop.
fn subscription_alive(context: &ServerContext, subscription_id: SubscriptionId) -> bool {
    context
        .inspect_subscriptions
        .list()
        .iter()
        .any(|subscription| subscription.id == subscription_id)
}

/// The size a composed frame is derived from: the requested crop clipped to the
/// output (the inspector crops the same way), else the whole output.
fn composed_source(frame: Rect, region: Option<Rect>) -> Size {
    region
        .and_then(|region| frame.intersect(&region))
        .map(|clipped| clipped.size())
        .unwrap_or_else(|| frame.size())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ImageBuffer;

    #[test]
    fn the_scale_source_is_the_clipped_crop_else_the_whole_output() {
        let frame = Rect::new(0, 0, 1280, 800);
        assert_eq!(composed_source(frame, None), Size::new(1280, 800));
        assert_eq!(
            composed_source(frame, Some(Rect::new(10, 20, 100, 50))),
            Size::new(100, 50)
        );
        // A crop running off the frame is clipped exactly like the inspector's
        // post-processing, so the reported scale stays truthful.
        assert_eq!(
            composed_source(frame, Some(Rect::new(1200, 700, 400, 400))),
            Size::new(80, 100)
        );
    }

    #[test]
    fn the_reported_scale_is_the_downscale_applied_to_the_crop() {
        let frame = Rect::new(0, 0, 1280, 800);
        let source = composed_source(frame, Some(Rect::new(10, 20, 100, 50)));
        let buffer = ImageBuffer::new_rgba(50, 25);
        assert!((super::super::capture::scale_from(source, &buffer) - 0.5).abs() < f64::EPSILON);
    }
}
