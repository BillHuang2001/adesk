//! §5.3 window methods.
//!
//! Window state is read from the compositor (`QueryState`); `activate_window`
//! and `close_window` are runtime-native — they change compositor state
//! directly and never synthesize input (`docs/protocol.md` §5.3).
//!
//! This module is also the canonical home of the compositor-bridge helpers the
//! sibling dispatch groups share: `state` (§5.2/§5.3/§5.4/§5.5 state reads),
//! `reserve_seq` (sequences of server-synthesized events), `command_error`
//! (command failures) and `unknown_window`. `apps.rs`, `capture.rs`, `input.rs`
//! and `inspect.rs` import them from here so each has exactly one implementation.

use adesk_compositor::{CompositorError, RuntimeCommand, StateSnapshot};
use adesk_core::{ErrorCode, WindowId};
use adesk_observer::ActionKind;
use adesk_proto::{
    ActionResult, ActivateWindowParams, CloseWindowParams, GetFocusParams, GetFocusResult,
    GetWindowParams, GetWindowResult, ListWindowsParams, ListWindowsResult,
};
use tokio::sync::oneshot;

use crate::context::ServerContext;
use crate::dispatch::RequestContext;
use crate::error::{Result, ServerError};

/// Reads the compositor's current state through `QueryState`.
///
/// The canonical state read of every dispatch group: `pub(crate)` so §5.2's
/// `launch_app` (the compositor clock), §5.4's captures and §5.5's
/// window-relative coordinates all use this one implementation, and so the
/// viewer endpoint can reuse it.
///
/// # Errors
///
/// [`ServerError::ShuttingDown`] when the command channel is closed and when the
/// compositor drops the reply — `QueryState` is infallible, so a dropped reply
/// can only mean the compositor thread is gone, i.e. the runtime is shutting
/// down (`crates/adesk-server/CONTEXT.md`, error mapping).
pub(crate) async fn state(server: &ServerContext) -> Result<StateSnapshot> {
    let (reply, answer) = oneshot::channel();
    server
        .compositor
        .send(RuntimeCommand::QueryState { reply })?;
    answer.await.map_err(|_| ServerError::ShuttingDown)
}

/// Reserves the next event sequence number from the compositor's counter.
///
/// `seq` has one global monotonic domain covering compositor- **and**
/// server-emitted events (`docs/protocol.md` §1), and the compositor owns the
/// only counter. Server-synthesized events (`AppLaunched` §5.2, `inspect_frame`
/// §5.7) therefore take their `seq` from here instead of deriving it from the
/// observed watermark: the reservation advances the shared counter and emits
/// nothing, so a later compositor event can never reuse the number. Gaps are
/// allowed (a reserved number may go unused), reuse is not.
///
/// # Errors
///
/// [`ServerError::ShuttingDown`] when the command channel is closed and when the
/// compositor drops the reply — `ReserveSeq` is infallible, so a dropped reply
/// can only mean the compositor thread is gone, i.e. the runtime is shutting
/// down (`crates/adesk-server/CONTEXT.md`, error mapping).
pub(super) async fn reserve_seq(server: &ServerContext) -> Result<u64> {
    let (reply, answer) = oneshot::channel();
    server
        .compositor
        .send(RuntimeCommand::ReserveSeq { reply })?;
    answer.await.map_err(|_| ServerError::ShuttingDown)
}

/// `list_windows`: all live windows plus the active window id.
pub async fn list_windows(
    ctx: &RequestContext<'_>,
    params: ListWindowsParams,
) -> Result<ListWindowsResult> {
    let _ = params; // `list_windows` has no parameters (§5.3).
    let snapshot = state(ctx.server).await?;
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
    let snapshot = state(ctx.server).await?;
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
        Ok(Err(error)) => Err(command_error(Some(params.window_id), error)),
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
        Ok(Err(error)) => Err(command_error(Some(params.window_id), error)),
        Err(_) => Err(ServerError::Internal(
            "compositor dropped the close_window reply".to_owned(),
        )),
    }
}

/// `get_focus`: active window and the surface-level keyboard focus.
pub async fn get_focus(ctx: &RequestContext<'_>, params: GetFocusParams) -> Result<GetFocusResult> {
    let _ = params; // `get_focus` has no parameters (§5.3).
    let snapshot = state(ctx.server).await?;
    Ok(GetFocusResult {
        window_id: snapshot.active_window_id,
        // `keyboard_focus` is the window whose surface holds the seat's
        // keyboard focus; `surface_focus` reports whether any surface does.
        surface_focus: snapshot.keyboard_focus.is_some(),
    })
}

/// Maps a compositor command failure into [`ServerError`].
///
/// Command replies carry [`adesk_core::Error`], which already holds the AGP code
/// the compositor chose. The server re-wraps it in the [`CompositorError`]
/// variant that maps back to that code, so `unknown_window`, `invalid_request`,
/// `render_failed` and `shutting_down` survive the boundary
/// (`crates/adesk-server/CONTEXT.md`, error mapping) instead of collapsing into
/// `internal`. This is the single mapping used by every dispatch group (§5.3,
/// §5.4, §5.5).
///
/// `window_id` is the window the command targeted, used to describe an
/// `unknown_window` failure; `None` keeps the original message.
pub(crate) fn command_error(window_id: Option<WindowId>, error: adesk_core::Error) -> ServerError {
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
        // Anything else is a runtime fault, not a client error.
        _ => CompositorError::Internal(error.message),
    };
    ServerError::Compositor(error)
}

/// The `unknown_window` failure for a window the compositor does not know.
pub(crate) fn unknown_window(window_id: WindowId) -> ServerError {
    ServerError::Compositor(CompositorError::UnknownWindow(window_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_window_keeps_the_requested_id() {
        // `CompositorError::UnknownWindow` is the variant that maps to the AGP
        // `unknown_window` code; the mapping itself lives in `crate::error`.
        for window_id in [WindowId(7), WindowId(99)] {
            assert!(matches!(
                unknown_window(window_id),
                ServerError::Compositor(CompositorError::UnknownWindow(id)) if id == window_id
            ));
        }
    }

    #[test]
    fn unknown_window_command_failure_keeps_the_requested_id() {
        let error = command_error(
            Some(WindowId(7)),
            adesk_core::Error::unknown_window(WindowId(7)),
        );
        assert!(matches!(
            error,
            ServerError::Compositor(CompositorError::UnknownWindow(WindowId(7)))
        ));
    }

    #[test]
    fn invalid_request_stays_a_client_error() {
        let error = command_error(
            Some(WindowId(7)),
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
    fn render_and_shutdown_failures_keep_their_code() {
        assert!(matches!(
            command_error(
                None,
                adesk_core::Error::new(ErrorCode::RenderFailed, "boom")
            ),
            ServerError::Compositor(CompositorError::Render(_))
        ));
        assert!(matches!(
            command_error(
                None,
                adesk_core::Error::new(ErrorCode::CaptureFailed, "boom")
            ),
            ServerError::Compositor(CompositorError::Render(_))
        ));
        assert!(matches!(
            command_error(
                Some(WindowId(7)),
                adesk_core::Error::new(ErrorCode::ShuttingDown, "stopping"),
            ),
            ServerError::Compositor(CompositorError::Stopped)
        ));
    }

    #[test]
    fn other_failures_are_internal() {
        // An unmapped code must not be silently reported as a client error.
        let error = command_error(Some(WindowId(7)), adesk_core::Error::internal("boom"));
        assert!(matches!(
            error,
            ServerError::Compositor(CompositorError::Internal(message)) if message == "boom"
        ));
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
