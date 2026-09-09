//! §5.3 window methods.
//!
//! Window state is read from the compositor (`QueryState`); `activate_window`
//! and `close_window` are runtime-native — they change compositor state
//! directly and never synthesize input (`docs/protocol.md` §5.3).

use adesk_compositor::{CompositorError, RuntimeCommand, StateSnapshot};
use adesk_core::{ErrorCode, WindowId};
use adesk_observer::ActionKind;
use adesk_proto::{
    ActionResult, ActivateWindowParams, CloseWindowParams, GetFocusParams, GetFocusResult,
    GetWindowParams, GetWindowResult, ListWindowsParams, ListWindowsResult,
};
use tokio::sync::oneshot;

use crate::dispatch::RequestContext;
use crate::error::{Result, ServerError};

/// Reads the compositor's current state through `QueryState`.
///
/// `pub(super)` so §5.2's `launch_app` can stamp its `app_launched` event with
/// the compositor's sequence watermark and clock.
///
/// # Errors
///
/// [`ServerError::ShuttingDown`] when the command channel is closed and
/// [`ServerError::Internal`] when the compositor drops the reply.
pub(super) async fn state(ctx: &RequestContext<'_>) -> Result<StateSnapshot> {
    let (reply, answer) = oneshot::channel();
    ctx.server
        .compositor
        .send(RuntimeCommand::QueryState { reply })?;
    answer
        .await
        .map_err(|_| ServerError::Internal("compositor dropped the QueryState reply".to_owned()))
}

/// `list_windows`: all live windows plus the active window id.
pub async fn list_windows(
    ctx: &RequestContext<'_>,
    params: ListWindowsParams,
) -> Result<ListWindowsResult> {
    let _ = params; // `list_windows` has no parameters (§5.3).
    let snapshot = state(ctx).await?;
    Ok(ListWindowsResult {
        windows: snapshot.windows,
        active_window_id: snapshot.active_window_id,
    })
}

/// `get_window`: one window by id (`unknown_window` when absent).
pub async fn get_window(
    ctx: &RequestContext<'_>,
    params: GetWindowParams,
) -> Result<GetWindowResult> {
    let snapshot = state(ctx).await?;
    let window = snapshot
        .window(params.window_id)
        .cloned()
        .ok_or_else(|| ServerError::Compositor(CompositorError::UnknownWindow(params.window_id)))?;
    Ok(GetWindowResult { window })
}

/// `activate_window`: runtime-native focus change, recorded as an action.
pub async fn activate_window(
    ctx: &RequestContext<'_>,
    params: ActivateWindowParams,
) -> Result<ActionResult> {
    // The action is recorded *before* the command: a concurrent observer must be
    // able to correlate the change with the action that caused it, and the
    // operation is runtime-native — never synthesized input (invariant 3).
    let action_id =
        ctx.server
            .observer
            .record_action(ActionKind::ActivateWindow, Some(params.window_id), None);
    let (reply, answer) = oneshot::channel();
    ctx.server.compositor.send(RuntimeCommand::ActivateWindow {
        window_id: params.window_id,
        reply,
    })?;
    match answer.await {
        Ok(Ok(())) => Ok(ActionResult { action_id }),
        Ok(Err(error)) => Err(command_error(params.window_id, error)),
        Err(_) => Err(ServerError::Internal(
            "compositor dropped the activate_window reply".to_owned(),
        )),
    }
}

/// `close_window`: runtime-native close, recorded as an action.
pub async fn close_window(
    ctx: &RequestContext<'_>,
    params: CloseWindowParams,
) -> Result<ActionResult> {
    // Recorded before the command, exactly like `activate_window`.
    let action_id =
        ctx.server
            .observer
            .record_action(ActionKind::CloseWindow, Some(params.window_id), None);
    let (reply, answer) = oneshot::channel();
    ctx.server.compositor.send(RuntimeCommand::CloseWindow {
        window_id: params.window_id,
        reply,
    })?;
    match answer.await {
        Ok(Ok(())) => Ok(ActionResult { action_id }),
        Ok(Err(error)) => Err(command_error(params.window_id, error)),
        Err(_) => Err(ServerError::Internal(
            "compositor dropped the close_window reply".to_owned(),
        )),
    }
}

/// `get_focus`: active window and the surface-level keyboard focus.
pub async fn get_focus(ctx: &RequestContext<'_>, params: GetFocusParams) -> Result<GetFocusResult> {
    let _ = params; // `get_focus` has no parameters (§5.3).
    let snapshot = state(ctx).await?;
    Ok(GetFocusResult {
        window_id: snapshot.active_window_id,
        // `keyboard_focus` is the window whose surface holds the seat's
        // keyboard focus; `surface_focus` reports whether any surface does.
        surface_focus: snapshot.keyboard_focus.is_some(),
    })
}

/// Maps a compositor command failure (`adesk_core::Error`) onto the server error
/// type, keeping the AGP code of the failures §5.3 can produce.
fn command_error(window_id: WindowId, error: adesk_core::Error) -> ServerError {
    match error.code {
        ErrorCode::UnknownWindow => {
            ServerError::Compositor(CompositorError::UnknownWindow(window_id))
        }
        ErrorCode::InvalidRequest => {
            ServerError::Compositor(CompositorError::InvalidRequest(error.message))
        }
        // Anything else is a runtime fault, not a client error.
        _ => ServerError::Internal(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_window_keeps_the_requested_id() {
        let error = command_error(WindowId(7), adesk_core::Error::unknown_window(WindowId(7)));
        assert!(matches!(
            error,
            ServerError::Compositor(CompositorError::UnknownWindow(WindowId(7)))
        ));
    }

    #[test]
    fn invalid_request_stays_a_client_error() {
        let error = command_error(
            WindowId(7),
            adesk_core::Error::invalid_request("no focused window"),
        );
        match error {
            ServerError::Compositor(CompositorError::InvalidRequest(message)) => {
                assert_eq!(message, "no focused window")
            }
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn other_failures_are_internal() {
        let error = command_error(WindowId(7), adesk_core::Error::internal("boom"));
        assert!(matches!(error, ServerError::Internal(message) if message == "boom"));
    }
}
