//! Serving one [`RuntimeCommand`] at a time.
//!
//! [`handle_command`] is the command side of the compositor thread's contract
//! (`docs/architecture.md` §3): it maps every command variant onto exactly one
//! [`State`] method, sends the reply **immediately** after the call returns and
//! never blocks. Commands are served in FIFO order by the `calloop` channel source,
//! which is what gives input actions their causal order.
//!
//! The module is declared from `run.rs` with `#[path = "dispatch.rs"]`, which keeps
//! the file at `src/dispatch.rs` while its module path is `crate::run::dispatch`.
//!
//! Reply types use the `adesk_core` umbrella error; [`CompositorError`] converts
//! into it, so failures reach the AGP layer as structured `ErrorCode`s.
//!
//! [`CompositorError`]: crate::error::CompositorError

use tokio::sync::oneshot;

use crate::{command::RuntimeCommand, state::State};

/// What the event loop should do once a command has been served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandOutcome {
    /// Keep running; the command was served normally.
    Continue,
    /// The command asked the compositor to stop; the loop must be signalled.
    Shutdown,
}

/// Serve `command` against `state` and send its reply.
///
/// Every arm replies exactly once (or drops the reply channel when the caller went
/// away, which is legal) before returning; the only command that changes the loop's
/// lifetime is [`RuntimeCommand::Shutdown`].
pub(crate) fn handle_command(state: &mut State, command: RuntimeCommand) -> CommandOutcome {
    let span = tracing::debug_span!("request", method = command.method());
    let _entered = span.enter();

    match command {
        RuntimeCommand::RenderWindow {
            window_id,
            region,
            max_dimension,
            reply,
        } => {
            let result = state.render_window(window_id, region, max_dimension);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::RenderOutput {
            overlays,
            region,
            max_dimension,
            reply,
        } => {
            let result = state.render_output(&overlays, region, max_dimension);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::QueryState { reply } => {
            // Infallible: a snapshot always exists.
            let _ = reply.send(state.snapshot());
            CommandOutcome::Continue
        }
        RuntimeCommand::ActivateWindow { window_id, reply } => {
            let result = state.activate_window(window_id);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::CloseWindow { window_id, reply } => {
            let result = state.close_window(window_id);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::PointerMove { position, reply } => {
            let result = state.inject_pointer_move(&position);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::PointerButton {
            button,
            state: button_state,
            reply,
        } => {
            let result = state.inject_pointer_button(button, button_state);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::PointerAxis { dx, dy, reply } => {
            let result = state.inject_pointer_axis(dx, dy);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::KeyEvent {
            key,
            state: key_state,
            reply,
        } => {
            let result = state.inject_key(&key, key_state);
            let _ = reply.send(result.map_err(Into::into));
            CommandOutcome::Continue
        }
        RuntimeCommand::Shutdown { reply } => shutdown(reply),
    }
}

/// Acknowledge `Shutdown` and tell the loop to stop.
///
/// The acknowledgement is sent before the loop stops so [`crate::CompositorHandle::shutdown`]
/// resolves as soon as the command is served, not when the thread has finished
/// tearing the display down.
fn shutdown(reply: oneshot::Sender<()>) -> CommandOutcome {
    let _ = reply.send(());
    CommandOutcome::Shutdown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{command::RuntimeCommand, snapshot::RenderedFrame};
    use adesk_core::{Button, ButtonState, KeyState, Position, Rect, WindowId};

    /// One `oneshot::Sender` of the right type for every result-bearing variant.
    fn render_reply() -> oneshot::Sender<adesk_core::Result<RenderedFrame>> {
        oneshot::channel().0
    }

    #[test]
    fn method_names_are_exact_and_unique() {
        let commands = vec![
            (
                RuntimeCommand::RenderWindow {
                    window_id: WindowId(1),
                    region: Some(Rect::new(0, 0, 10, 10)),
                    max_dimension: Some(64),
                    reply: render_reply(),
                },
                "render_window",
            ),
            (
                RuntimeCommand::RenderOutput {
                    overlays: vec![],
                    region: None,
                    max_dimension: None,
                    reply: render_reply(),
                },
                "render_output",
            ),
            (
                RuntimeCommand::QueryState {
                    reply: oneshot::channel().0,
                },
                "query_state",
            ),
            (
                RuntimeCommand::ActivateWindow {
                    window_id: WindowId(2),
                    reply: oneshot::channel().0,
                },
                "activate_window",
            ),
            (
                RuntimeCommand::CloseWindow {
                    window_id: WindowId(2),
                    reply: oneshot::channel().0,
                },
                "close_window",
            ),
            (
                RuntimeCommand::PointerMove {
                    position: Position::normalized(0.5, 0.5),
                    reply: oneshot::channel().0,
                },
                "pointer_move",
            ),
            (
                RuntimeCommand::PointerButton {
                    button: Button::Left,
                    state: ButtonState::Pressed,
                    reply: oneshot::channel().0,
                },
                "pointer_button",
            ),
            (
                RuntimeCommand::PointerAxis {
                    dx: 0.0,
                    dy: -3.0,
                    reply: oneshot::channel().0,
                },
                "pointer_axis",
            ),
            (
                RuntimeCommand::KeyEvent {
                    // `docs/protocol.md` §3: keysym-style names are case-insensitive
                    // and `RETURN` is one of the documented aliases.
                    key: crate::input::KeyCode::parse("RETURN").expect("parseable key name"),
                    state: KeyState::Pressed,
                    reply: oneshot::channel().0,
                },
                "key_event",
            ),
            (
                RuntimeCommand::Shutdown {
                    reply: oneshot::channel().0,
                },
                "shutdown",
            ),
        ];

        let mut names: Vec<&'static str> = Vec::new();
        for (command, expected) in &commands {
            assert_eq!(command.method(), *expected);
            names.push(command.method());
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), commands.len(), "method names must be unique");
    }

    #[test]
    fn shutdown_acknowledges_then_requests_stop() {
        let (reply, acknowledged) = oneshot::channel();
        assert_eq!(shutdown(reply), CommandOutcome::Shutdown);
        assert_eq!(acknowledged.blocking_recv(), Ok(()));
    }

    #[test]
    fn command_outcomes_are_distinguishable() {
        assert_ne!(CommandOutcome::Continue, CommandOutcome::Shutdown);
        assert_eq!(CommandOutcome::Continue, CommandOutcome::Continue);
    }
}
