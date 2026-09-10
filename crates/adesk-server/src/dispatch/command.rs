//! The compositor-command seam shared by the dispatch groups.
//!
//! Every §5 group builds a `RuntimeCommand` around a `oneshot` reply channel,
//! sends it and awaits it. Two reply shapes exist: an infallible value
//! (`QueryState`, `ReserveSeq`) and an `adesk_core::Result<T>` (the seat and
//! window commands). `send_infallible` and `send_result` each implement one
//! shape, so a handler never repeats the channel/send/await boilerplate.
//!
//! The classification of a *dropped* reply is deliberately not uniform — the seat
//! helpers and the state reads report `shutting_down`, the runtime-native
//! `activate_window`/`close_window`/`render_window` report `internal` — so it is a
//! caller-supplied closure rather than a fixed mapping (`crates/adesk-server/CONTEXT.md`,
//! error mapping). A failure reply always maps through `super::windows::command_error`.

use adesk_compositor::RuntimeCommand;
use adesk_core::WindowId;
use tokio::sync::oneshot;

use crate::context::ServerContext;
use crate::error::{Result, ServerError};

use super::windows::command_error;

/// Sends a command whose reply is infallible (no inner `Result`) and awaits it.
///
/// `QueryState` and `ReserveSeq` are always answered while the compositor runs, so
/// a dropped reply can only mean the thread is gone: [`ServerError::ShuttingDown`].
pub(crate) async fn send_infallible<T>(
    server: &ServerContext,
    make: impl FnOnce(oneshot::Sender<T>) -> RuntimeCommand,
) -> Result<T> {
    let (reply, response) = oneshot::channel();
    server.compositor.send(make(reply))?;
    response.await.map_err(|_| ServerError::ShuttingDown)
}

/// Sends a command whose reply carries `adesk_core::Result<T>` and awaits it.
///
/// `window_id` names the window an `unknown_window` failure is reported against
/// (see `super::windows::command_error`); `dropped` classifies a dropped reply —
/// the seat helpers report `shutting_down`, `activate_window`/`close_window`/
/// `render_window` report `internal`.
pub(crate) async fn send_result<T>(
    server: &ServerContext,
    window_id: Option<WindowId>,
    dropped: impl FnOnce() -> ServerError,
    make: impl FnOnce(oneshot::Sender<adesk_core::Result<T>>) -> RuntimeCommand,
) -> Result<T> {
    let (reply, response) = oneshot::channel();
    server.compositor.send(make(reply))?;
    match response.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(command_error(window_id, error)),
        Err(_) => Err(dropped()),
    }
}
