//! §5.4 capture and observation methods.
//!
//! Captures render on demand (`RuntimeCommand::RenderWindow`); observation
//! methods await the observer **first** and only then render, so the attached
//! image matches the observation the client receives (`docs/architecture.md` §6).
//! Waits time out as *observations* (`timed_out: true`), never as errors.
//!
//! The command/state helpers here are shared with the sibling dispatch groups:
//! [`query_state`] and [`command_error`] by `dispatch::input` (pointer
//! coordinates and command failures) and [`scale_from`] by `dispatch::inspect`
//! (the downscale factor reported in `ImagePayload::scale`).

use adesk_compositor::{CompositorError, RenderedFrame, RuntimeCommand, StateSnapshot};
use adesk_core::{ErrorCode, ImageBuffer, Rect, Size, WindowId, WindowInfo};
use adesk_observer::{ObserveSpec, QuietSpec, WaitSpec};
use adesk_proto::{
    CaptureRegionParams, CaptureResult, CaptureWindowParams, ImageFormat, ObserveParams,
    ObserveResult, WaitForChangeParams, WaitForQuietParams,
};
use tokio::sync::oneshot;

use crate::dispatch::RequestContext;
use crate::error::{Result, ServerError};
use crate::images;
use crate::translate;

/// `capture_window`: render a window's current pixels (crop/downscale optional).
pub async fn capture_window(
    ctx: &RequestContext<'_>,
    params: CaptureWindowParams,
) -> Result<CaptureResult> {
    let frame = render_window(ctx, params.window_id, params.region, params.max_dimension).await?;
    capture_result(ctx, params.window_id, params.region, params.format, frame).await
}

/// `capture_region`: render a required sub-rect of a window.
pub async fn capture_region(
    ctx: &RequestContext<'_>,
    params: CaptureRegionParams,
) -> Result<CaptureResult> {
    // `region` is not optional in `CaptureRegionParams`, so a request without one
    // never reaches this handler: the proto decoder rejects it as
    // `invalid_request` (§5.4, §6).
    let region = Some(params.region);
    let frame = render_window(ctx, params.window_id, region, params.max_dimension).await?;
    capture_result(ctx, params.window_id, region, params.format, frame).await
}

/// `observe`: wait for a condition, then optionally attach an image.
pub async fn observe(ctx: &RequestContext<'_>, params: ObserveParams) -> Result<ObserveResult> {
    let mut spec = ObserveSpec::new(translate::observer_condition(params.until))
        .timeout_ms(params.timeout_ms);
    if let Some(window_id) = params.window_id {
        spec = spec.window(window_id);
    }
    if let Some(after_action) = params.after_action {
        spec = spec.after_action(after_action);
    }

    // Wait first, render second: the image must match the observation (§5.4).
    let observation = ctx.server.observer.observe(spec).await?;
    let image = if params.include_image {
        observation_image(
            ctx,
            observation.window_id,
            params.region,
            params.max_dimension,
        )
        .await?
    } else {
        None
    };
    Ok(translate::observe_result(observation, image))
}

/// `wait_for_change`: resolve on the first counted commit/lifecycle event.
pub async fn wait_for_change(
    ctx: &RequestContext<'_>,
    params: WaitForChangeParams,
) -> Result<ObserveResult> {
    let mut spec = WaitSpec::new().timeout_ms(params.timeout_ms);
    if let Some(window_id) = params.window_id {
        spec = spec.window(window_id);
    }
    if let Some(since_commit) = params.since_commit {
        spec = spec.since_commit(since_commit);
    }

    let observation = ctx.server.observer.wait_for_change(spec).await?;
    let image = if params.include_image {
        observation_image(ctx, observation.window_id, None, None).await?
    } else {
        None
    };
    Ok(translate::observe_result(observation, image))
}

/// `wait_for_quiet`: resolve once the window has been quiet for `quiet_ms`.
pub async fn wait_for_quiet(
    ctx: &RequestContext<'_>,
    params: WaitForQuietParams,
) -> Result<ObserveResult> {
    let mut spec = QuietSpec::new()
        .quiet_ms(params.quiet_ms)
        .timeout_ms(params.timeout_ms);
    if let Some(window_id) = params.window_id {
        spec = spec.window(window_id);
    }
    if let Some(after_action) = params.after_action {
        spec = spec.after_action(after_action);
    }

    let observation = ctx.server.observer.wait_for_quiet(spec).await?;
    let image = if params.include_image {
        observation_image(ctx, observation.window_id, None, None).await?
    } else {
        None
    };
    Ok(translate::observe_result(observation, image))
}

/// Renders one window's surface tree offscreen.
///
/// Rendering happens only because this command asks for it (design invariant 5);
/// there is no frame loop and no screenshot cache. The compositor resolves the
/// window and reports `UnknownWindow` for an id it does not know.
async fn render_window(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    region: Option<Rect>,
    max_dimension: Option<u32>,
) -> Result<RenderedFrame> {
    let (reply, response) = oneshot::channel();
    ctx.server.compositor.send(RuntimeCommand::RenderWindow {
        window_id,
        region,
        max_dimension,
        reply,
    })?;
    let frame = response.await.map_err(|_| {
        ServerError::Internal(format!(
            "compositor dropped the render_window reply for window {window_id}"
        ))
    })?;
    frame.map_err(|error| command_error(Some(window_id), error))
}

/// Builds the `capture_window`/`capture_region` result from a rendered frame.
///
/// The window description comes from a `QueryState` snapshot — a
/// [`RenderedFrame`] carries pixels and commit information, never window state.
async fn capture_result(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    region: Option<Rect>,
    format: ImageFormat,
    frame: RenderedFrame,
) -> Result<CaptureResult> {
    let snapshot = query_state(ctx).await?;
    let window = snapshot
        .window(window_id)
        .cloned()
        .ok_or_else(|| unknown_window(window_id))?;
    let scale = scale_from(source_size(&window, region), &frame.image);
    Ok(CaptureResult {
        image: images::encode(&frame.image, format, scale)?,
        window,
        commit_seq: frame.commit_seq,
        changed_regions: frame.damage,
    })
}

/// Renders the image attached to an observation (§5.4).
///
/// `window_id` is the observation's own window; when the observation was global
/// the active window is used. Without any window there is nothing to render and
/// the observation is returned without an image.
async fn observation_image(
    ctx: &RequestContext<'_>,
    window_id: Option<WindowId>,
    region: Option<Rect>,
    max_dimension: Option<u32>,
) -> Result<Option<adesk_proto::ImagePayload>> {
    let Some(window_id) = observed_window(ctx, window_id).await? else {
        tracing::debug!("observation has no window to render");
        return Ok(None);
    };

    let frame = render_window(ctx, window_id, region, max_dimension).await?;
    let snapshot = query_state(ctx).await?;
    let scale = match snapshot.window(window_id) {
        Some(window) => scale_from(source_size(window, region), &frame.image),
        // The window disappeared between the render and the state query; the
        // pixels are still valid, only the downscale factor is unknown.
        None => 1.0,
    };
    Ok(Some(images::encode(&frame.image, ImageFormat::Png, scale)?))
}

/// The window an observation's image is rendered from.
///
/// A scoped observation names its window; a global one falls back to the active
/// window (never a hard-coded id).
async fn observed_window(
    ctx: &RequestContext<'_>,
    window_id: Option<WindowId>,
) -> Result<Option<WindowId>> {
    if window_id.is_some() {
        return Ok(window_id);
    }
    let snapshot = query_state(ctx).await?;
    Ok(snapshot.active_window_id.or(snapshot.keyboard_focus))
}

/// Current compositor state (window list, focus, sequence watermark).
///
/// Shared with `dispatch::input`, which resolves window-relative coordinates
/// through the window model's geometry.
pub(super) async fn query_state(ctx: &RequestContext<'_>) -> Result<StateSnapshot> {
    let (reply, response) = oneshot::channel();
    ctx.server.compositor.send(RuntimeCommand::QueryState { reply })?;
    // `QueryState` is infallible: only a vanished compositor can drop the reply.
    response.await.map_err(|_| ServerError::ShuttingDown)
}

/// Maps a compositor command failure into [`ServerError`].
///
/// Command replies carry [`adesk_core::Error`], which already holds the AGP code
/// the compositor chose. The server re-wraps it in the [`CompositorError`]
/// variant that maps back to that code, so `unknown_window`, `invalid_request`
/// and `render_failed` survive the boundary (`crates/adesk-server/CONTEXT.md`,
/// error mapping) instead of collapsing into `internal`.
///
/// `window_id` is the window the command targeted, used to describe an
/// `unknown_window` failure; `None` keeps the original message.
pub(super) fn command_error(window_id: Option<WindowId>, error: adesk_core::Error) -> ServerError {
    let error = match error.code {
        ErrorCode::UnknownWindow => match window_id {
            Some(window_id) => CompositorError::UnknownWindow(window_id),
            None => CompositorError::Internal(error.message),
        },
        ErrorCode::InvalidRequest => CompositorError::InvalidRequest(error.message),
        ErrorCode::RenderFailed | ErrorCode::CaptureFailed => {
            CompositorError::Render(error.message)
        }
        ErrorCode::ShuttingDown => CompositorError::Stopped,
        _ => CompositorError::Internal(error.message),
    };
    ServerError::Compositor(error)
}

/// The `unknown_window` failure for a window the compositor does not know.
pub(super) fn unknown_window(window_id: WindowId) -> ServerError {
    ServerError::Compositor(CompositorError::UnknownWindow(window_id))
}

/// The size the compositor rendered from: the requested crop, else the window.
fn source_size(window: &WindowInfo, region: Option<Rect>) -> Size {
    match region {
        Some(region) => region.size(),
        None => window.geometry.size(),
    }
}

/// The downscale factor the compositor applied, reported as `ImagePayload::scale`
/// (`1.0` = full resolution).
///
/// The render pipeline preserves the aspect ratio, so the width ratio is the
/// factor for both axes.
pub(super) fn scale_from(source: Size, image: &ImageBuffer) -> f64 {
    if source.w == 0 {
        return 1.0;
    }
    f64::from(image.width) / f64::from(source.w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AppId, WindowState};

    fn window(geometry: Rect) -> WindowInfo {
        WindowInfo {
            id: WindowId(7),
            app_id: Some(AppId::from("org.example.app")),
            title: Some("Example".to_owned()),
            geometry,
            state: WindowState::Active,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq: 2,
            popup_count: 0,
        }
    }

    #[test]
    fn source_size_prefers_the_requested_crop() {
        let window = window(Rect::new(0, 0, 1280, 800));
        assert_eq!(source_size(&window, None), Size::new(1280, 800));
        assert_eq!(
            source_size(&window, Some(Rect::new(10, 20, 100, 50))),
            Size::new(100, 50)
        );
    }

    #[test]
    fn scale_reports_the_downscale_the_compositor_applied() {
        let source = Size::new(1280, 800);
        assert!((scale_from(source, &ImageBuffer::new_rgba(1280, 800)) - 1.0).abs() < f64::EPSILON);
        assert!((scale_from(source, &ImageBuffer::new_rgba(640, 400)) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn scale_of_an_empty_source_is_full_resolution() {
        assert!((scale_from(Size::ZERO, &ImageBuffer::new_rgba(0, 0)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_window_keeps_the_unknown_window_variant() {
        // `CompositorError::UnknownWindow` is the variant that maps to the AGP
        // `unknown_window` code; the mapping itself lives in `crate::error`.
        assert!(matches!(
            unknown_window(WindowId(99)),
            ServerError::Compositor(CompositorError::UnknownWindow(WindowId(99)))
        ));
    }

    #[test]
    fn command_errors_keep_the_compositor_variant() {
        assert!(matches!(
            command_error(
                Some(WindowId(99)),
                adesk_core::Error::new(ErrorCode::UnknownWindow, "unknown window 99"),
            ),
            ServerError::Compositor(CompositorError::UnknownWindow(WindowId(99)))
        ));
        assert!(matches!(
            command_error(
                None,
                adesk_core::Error::new(ErrorCode::InvalidRequest, "bad key name"),
            ),
            ServerError::Compositor(CompositorError::InvalidRequest(_))
        ));
        assert!(matches!(
            command_error(
                None,
                adesk_core::Error::new(ErrorCode::RenderFailed, "boom"),
            ),
            ServerError::Compositor(CompositorError::Render(_))
        ));
        // An unmapped code must not be silently reported as a client error.
        assert!(matches!(
            command_error(None, adesk_core::Error::new(ErrorCode::Busy, "busy")),
            ServerError::Compositor(CompositorError::Internal(_))
        ));
    }

    #[test]
    fn command_error_without_a_window_keeps_the_message() {
        let error = command_error(
            None,
            adesk_core::Error::new(ErrorCode::UnknownWindow, "unknown window 42"),
        );
        assert!(error.to_string().contains("unknown window 42"));
    }
}
