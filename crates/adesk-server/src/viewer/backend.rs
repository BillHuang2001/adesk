//! The runtime's [`ViewerBackend`] implementation over [`ServerContext`].
//!
//! One backend serves every viewer connection of a runtime: the trait is
//! `Send + Sync` and the session only needs shared access. The backend owns the
//! single [`ChangeSignal`] (fed by one background pump reading the compositor's
//! event broadcast) it hands to every session, the advisory input-control owner,
//! and the [`InputQueue`] that keeps viewer input in submission order.
//!
//! Viewer input is never a special path: every mutation records an `ActionId` on
//! the observer **before** the compositor command (exactly like AGP §5.5) and
//! reuses the same `pub(crate)` seat helpers `crate::dispatch::input` exposes.

use std::sync::Mutex;

use adesk_compositor::{CompositorError, KeyCode, RendererName, RuntimeCommand};
use adesk_core::{ActionId, ButtonState, KeyState, Position, Rect, RuntimeEvent, Size};
use adesk_observer::ActionKind;
use adesk_viewer::{ChangeSignal, Result as ViewerResult, ViewerBackend, ViewerError, ViewerInput};
use adesk_viewer_proto::{
    ControlOwner, CursorState, DesktopState, KeyAction, ServerHello, ViewerFrame,
};
use tokio::sync::broadcast;

use crate::context::ServerContext;
use crate::error::ServerError;
use crate::session::InputQueue;

/// The runtime side of every viewer connection.
pub(crate) struct ViewerBackendImpl {
    /// Runtime state: compositor handle, observer, cursor tracker, shutdown.
    context: ServerContext,
    /// Advisory input owner announced in the handshake and updated by the
    /// session; poisoned locks are recovered, never panicked on.
    control: Mutex<ControlOwner>,
    /// The desktop-changed source handed to every session.
    change: ChangeSignal,
    /// Orders viewer input like `crate::session::InputQueue` orders §5.5.
    input: InputQueue,
}

impl ViewerBackendImpl {
    /// Builds the backend for `context` and spawns its event pump.
    ///
    /// The pump is the backend's only background task and lives as long as the
    /// runtime's event broadcast; **this must be called inside a tokio runtime**
    /// (it always is: [`crate::viewer::start`] runs from
    /// [`crate::Server::start`]).
    pub(crate) fn new(context: ServerContext) -> ViewerBackendImpl {
        let change = ChangeSignal::new();
        spawn_change_pump(&context, change.clone());
        ViewerBackendImpl {
            context,
            control: Mutex::new(ControlOwner::Ai),
            change,
            input: InputQueue::new(),
        }
    }

    /// The pointer position and visibility as normalized output fractions.
    ///
    /// Pixels never cross VAP (`docs/viewer.md` §4): the tracked output point is
    /// divided by the last pixel of the output and clamped to `0.0..=1.0`.
    fn cursor_state(&self) -> CursorState {
        let output = self.context.compositor.output_size();
        match self.context.cursor.get() {
            Some(point) => {
                CursorState::at(normalize(point.x, output.w), normalize(point.y, output.h))
            }
            None => CursorState::hidden(),
        }
    }
}

/// Spawns the one change pump: it fans the compositor's event broadcast into
/// `signal`, coalescing everything into "the desktop changed".
fn spawn_change_pump(context: &ServerContext, signal: ChangeSignal) {
    let mut events = context.compositor.subscribe();
    let shutdown = context.shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                received = events.recv() => match received {
                    Ok(event) => {
                        if is_desktop_change(&event) {
                            signal.notify();
                        }
                    }
                    // A lagged receiver missed events; something certainly
                    // changed, so render rather than pretend nothing did.
                    Err(broadcast::error::RecvError::Lagged(_)) => signal.notify(),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    });
}

/// Whether `event` changes what a viewer would see.
///
/// The semantic content a viewer renders is the composited output, so commits,
/// activation, window create/destroy and title changes all count; high-frequency
/// rendering stays the session's decision (`ChangeSignal` collapses them).
fn is_desktop_change(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::SurfaceCommit { .. }
            | RuntimeEvent::WindowActivated { .. }
            | RuntimeEvent::WindowCreated { .. }
            | RuntimeEvent::WindowDestroyed { .. }
            | RuntimeEvent::TitleChanged { .. }
    )
}

/// Maps a raw pixel coordinate onto the normalized `0.0..=1.0` output fraction.
///
/// The last pixel of an `extent`-pixel axis is `extent - 1`, so the final pixel
/// maps to `1.0`; a degenerate axis is treated as one pixel wide.
fn normalize(pixel: i32, extent: u32) -> f64 {
    let last = extent.saturating_sub(1).max(1);
    (pixel as f64 / last as f64).clamp(0.0, 1.0)
}

/// Resolves an optional VAP position onto the active window.
///
/// A viewer sends both fractions or neither (`docs/viewer.md` §4); "neither"
/// means "act at the current pointer position", which the seat helpers resolve.
fn viewer_position(x: Option<f64>, y: Option<f64>, output: Size, rect: Rect) -> Option<Position> {
    match (x, y) {
        (Some(x), Some(y)) => Some(crate::dispatch::input::output_fraction_position(
            x, y, output, rect,
        )),
        _ => None,
    }
}

/// Wraps a server failure as a VAP backend failure.
///
/// VAP carries only an `ErrorCode` + message, so the runtime's classification
/// travels as the message text (`ViewerError::Backend`).
fn backend_error(error: ServerError) -> ViewerError {
    ViewerError::Backend(error.to_string())
}

/// The failure a viewer input hits when no window can receive it.
///
/// Mirrors §5.5's no-keyboard-focus case: an `invalid_request`, never a panic.
fn no_active_window() -> ServerError {
    ServerError::Compositor(CompositorError::InvalidRequest(
        "no window is active".to_owned(),
    ))
}

/// The failure a released key that is not exactly one key hits.
fn not_a_single_key() -> ServerError {
    ServerError::Compositor(CompositorError::InvalidRequest(
        "a released key must be a single key".to_owned(),
    ))
}

#[async_trait::async_trait]
impl ViewerBackend for ViewerBackendImpl {
    fn display(&self) -> ServerHello {
        let renderer = self
            .context
            .compositor
            .renderer()
            .unwrap_or(RendererName::Pixman);
        ServerHello {
            protocol_version: adesk_viewer_proto::PROTOCOL_VERSION,
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            output: self.context.compositor.output_size(),
            renderer: crate::translate::proto_renderer(renderer),
            cursor: self.cursor_state(),
            control: *self
                .control
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        }
    }

    async fn render_frame(&self) -> ViewerResult<ViewerFrame> {
        let snapshot = crate::inspection::refresh(&self.context)
            .await
            .map_err(backend_error)?;
        let png = crate::images::encode_png(&snapshot.frame).map_err(backend_error)?;
        let image = adesk_proto::ImagePayload::from_png(
            snapshot.frame.width,
            snapshot.frame.height,
            &png,
            1.0,
        );
        // The frame's `seq` belongs to the single global monotonic domain, so it
        // is reserved from the compositor's counter — never read off the
        // `QueryState` watermark (`snapshot.seq`).
        let seq = crate::dispatch::windows::reserve_seq(&self.context)
            .await
            .map_err(backend_error)?;
        Ok(ViewerFrame {
            seq,
            ts_ms: self.context.now_ms(),
            image,
            cursor: self.cursor_state(),
            active_window_id: snapshot.active,
        })
    }

    async fn desktop_state(&self) -> ViewerResult<DesktopState> {
        let snapshot = crate::dispatch::windows::state(&self.context)
            .await
            .map_err(backend_error)?;
        Ok(DesktopState {
            active_window_id: snapshot.active_window_id,
            windows: snapshot.windows,
        })
    }

    async fn apply_input(&self, input: ViewerInput) -> ViewerResult<Option<ActionId>> {
        let server = &self.context;
        // Every viewer input targets the window the compositor's `inject_*`
        // resolves against: the keyboard focus, else the active window.
        let snapshot = crate::dispatch::windows::state(server)
            .await
            .map_err(backend_error)?;
        let target = snapshot
            .keyboard_focus
            .or(snapshot.active_window_id)
            .ok_or_else(|| backend_error(no_active_window()))?;
        let rect = snapshot
            .window(target)
            .map(|window| window.geometry)
            .ok_or_else(|| backend_error(crate::dispatch::windows::unknown_window(target)))?;
        let output = server.compositor.output_size();

        match input {
            ViewerInput::PointerMove { x, y } => {
                // `pointer_move` always carries both fractions.
                let position = crate::dispatch::input::output_fraction_position(x, y, output, rect);
                let action_id = server.observer.record_action(
                    ActionKind::PointerMove,
                    Some(target),
                    Some(position),
                );
                self.input
                    .run(async {
                        crate::dispatch::input::move_pointer(server, target, position, rect).await
                    })
                    .await
                    .map_err(backend_error)?;
                Ok(Some(action_id))
            }
            ViewerInput::PointerButton {
                button,
                state,
                x,
                y,
            } => {
                let position = viewer_position(x, y, output, rect);
                let kind = match state {
                    ButtonState::Pressed => ActionKind::MouseDown,
                    ButtonState::Released => ActionKind::MouseUp,
                };
                let action_id = server.observer.record_action(kind, Some(target), position);
                self.input
                    .run(async {
                        // The pointer must land before the button: the compositor
                        // delivers buttons at the current pointer position.
                        move_before(server, target, position, rect).await?;
                        crate::dispatch::input::button_event(server, target, button, state).await
                    })
                    .await
                    .map_err(backend_error)?;
                Ok(Some(action_id))
            }
            ViewerInput::Scroll { dx, dy, x, y } => {
                let position = viewer_position(x, y, output, rect);
                let action_id =
                    server
                        .observer
                        .record_action(ActionKind::Scroll, Some(target), position);
                self.input
                    .run(async {
                        move_before(server, target, position, rect).await?;
                        crate::dispatch::input::send_unit(server, Some(target), |reply| {
                            RuntimeCommand::PointerAxis { dx, dy, reply }
                        })
                        .await
                    })
                    .await
                    .map_err(backend_error)?;
                Ok(Some(action_id))
            }
            ViewerInput::Key { keys, action } => {
                let (key, state, kind) = match action {
                    KeyAction::Tap => (
                        parse_key_chord(&keys, target)?,
                        KeyState::Pressed,
                        ActionKind::Keypress,
                    ),
                    KeyAction::Pressed => (
                        parse_key_chord(&keys, target)?,
                        KeyState::Pressed,
                        ActionKind::KeyDown,
                    ),
                    KeyAction::Released => {
                        // A chord cannot be released half-applied, so a released
                        // key must name exactly one key (§5.5).
                        let [only] = keys.keys() else {
                            return Err(backend_error(not_a_single_key()));
                        };
                        (
                            KeyCode::parse(only).map_err(|error| {
                                backend_error(crate::dispatch::windows::command_error(
                                    Some(target),
                                    error,
                                ))
                            })?,
                            KeyState::Released,
                            ActionKind::KeyUp,
                        )
                    }
                };
                let action_id = server.observer.record_action(kind, Some(target), None);
                self.input
                    .run(async {
                        // Keyboard input activates the target first: the seat
                        // needs a keyboard focus to deliver the key to.
                        crate::dispatch::input::activate_if_needed(server, Some(target)).await?;
                        crate::dispatch::input::send_unit(server, Some(target), |reply| {
                            RuntimeCommand::KeyEvent { key, state, reply }
                        })
                        .await
                    })
                    .await
                    .map_err(backend_error)?;
                Ok(Some(action_id))
            }
            ViewerInput::Text { text } => {
                let action_id =
                    server
                        .observer
                        .record_action(ActionKind::TypeText, Some(target), None);
                let skipped = self
                    .input
                    .run(async {
                        crate::dispatch::input::activate_if_needed(server, Some(target)).await?;
                        let mut skipped = 0usize;
                        for character in text.chars() {
                            if !crate::dispatch::input::type_character(server, character).await? {
                                skipped += 1;
                            }
                        }
                        Ok::<usize, ServerError>(skipped)
                    })
                    .await
                    .map_err(backend_error)?;
                if skipped > 0 {
                    // Never log the text itself.
                    tracing::debug!(
                        skipped,
                        "viewer text: characters the keymap cannot produce were skipped"
                    );
                }
                Ok(Some(action_id))
            }
        }
    }

    fn change_signal(&self) -> ChangeSignal {
        self.change.clone()
    }

    async fn set_control(&self, owner: ControlOwner) -> ViewerResult<()> {
        *self
            .control
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = owner;
        Ok(())
    }
}

/// Moves the pointer to `position` when one was given, else to the §2 default
/// (the current pointer when inside the window, else the window center).
async fn move_before(
    server: &ServerContext,
    target: adesk_core::WindowId,
    position: Option<Position>,
    rect: Rect,
) -> Result<(), ServerError> {
    match position {
        Some(position) => {
            crate::dispatch::input::move_pointer(server, target, position, rect).await
        }
        None => crate::dispatch::input::move_to(server, target, None).await,
    }
}

/// Parses a viewer key/chord, mapping the parse failure like §5.5 does.
fn parse_key_chord(
    keys: &adesk_proto::KeySpec,
    target: adesk_core::WindowId,
) -> ViewerResult<KeyCode> {
    KeyCode::parse_chord(keys.keys()).map_err(|error| {
        backend_error(crate::dispatch::windows::command_error(Some(target), error))
    })
}
