//! The runtime's [`ViewerBackend`] implementation over [`ServerContext`].
//!
//! One backend serves every viewer connection of a runtime: the trait is
//! `Send + Sync` and the session only needs shared access. The backend owns the
//! single [`ChangeSignal`] (fed by one background pump reading the compositor's
//! event broadcast) it hands to every session, the advisory input-control owner,
//! the [`InputQueue`] that keeps viewer input in submission order, and the
//! runtime-scoped screen recording.
//!
//! Viewer input is never a special path: every mutation records an `ActionId` on
//! the observer **before** the compositor command (exactly like AGP §5.5) and
//! reuses the same `pub(crate)` seat helpers `crate::dispatch::input` exposes.
//!
//! Screen recording (`docs/viewer.md` §4, §5) is **runtime-scoped, not
//! connection-scoped**: one recording is active per runtime at a time, it
//! survives the viewer that started it disconnecting, and any viewer may query or
//! stop it. Capture is **on demand** — a background task renders the full output
//! only while the recording runs, pushing frames into an
//! [`adesk_recorder::RecordingSession`] that owns its own OS thread, so the
//! encoder never blocks the compositor.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adesk_compositor::{CompositorError, KeyCode, RendererName, RuntimeCommand};
use adesk_core::{
    ActionId, ButtonState, ErrorCode, KeyState, Position, Rect, RuntimeEvent, Size, WindowId,
};
use adesk_observer::ActionKind;
use adesk_recorder::{RecorderConfig, RecordingSession};
use adesk_viewer::{
    ChangeSignal, RecordRequest, Result as ViewerResult, ViewerBackend, ViewerError, ViewerInput,
};
use adesk_viewer_proto::{
    ControlOwner, CursorState, DesktopState, KeyAction, RecordingStatus, ServerHello, ViewerFrame,
};
use tokio::sync::{broadcast, watch};

use crate::context::ServerContext;
use crate::dispatch::RequestContext;
use crate::error::ServerError;
use crate::session::{InputQueue, Session};

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
    /// The runtime-scoped recording state (one active recording at most).
    recording: Mutex<RecordingState>,
}

/// Runtime-scoped screen-recording state.
///
/// `active` is the recording in progress (at most one, whatever viewer started
/// it); `last` is the most recently *finished* recording's status, so
/// `recording_status` reports where the last file landed instead of reverting to
/// idle the moment a recording stops. Both are empty until a recording runs.
#[derive(Default)]
struct RecordingState {
    /// The recording in progress, if any.
    active: Option<ActiveRecording>,
    /// The status of the most recently finished recording, if any.
    last: Option<RecordingStatus>,
}

/// A recording in progress: its session is owned by the capture task, so this
/// handle only carries what the status query and the stop path need.
struct ActiveRecording {
    /// Destination the recorder writes to.
    path: PathBuf,
    /// Resolved encoder backend name (`Recorder::encoder_name`).
    encoder: String,
    /// Requested frame rate (metadata; pacing is real).
    fps: u32,
    /// Monotonic ms the recording started (`ServerContext::now_ms`).
    started_ms: u64,
    /// Frames the capture task has queued so far, updated by the task.
    frames: Arc<AtomicU64>,
    /// Set to `true` to make the capture task finalize and return.
    stop: watch::Sender<bool>,
    /// Resolves with the finalized status once the task has stopped the session.
    task: tokio::task::JoinHandle<RecordingStatus>,
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
            recording: Mutex::new(RecordingState::default()),
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

    /// Applies a viewer `activate_window` (`docs/viewer.md` §5).
    ///
    /// This reuses the runtime-native AGP §5.3 path verbatim
    /// ([`crate::dispatch::windows::activate_window`]): it changes compositor
    /// window state directly, records the `ActionId` and never synthesizes input.
    /// The call runs through the shared [`InputQueue`], so it stays ordered with
    /// the connection's other input. The throwaway [`Session`] is unused by
    /// `activate_window` (it reads no `ctx.session` field).
    async fn activate_window(&self, window_id: WindowId) -> ViewerResult<Option<ActionId>> {
        let session = Session::new(0);
        let ctx = RequestContext {
            server: &self.context,
            session: &session,
        };
        let action = self
            .input
            .run(crate::dispatch::windows::activate_window(
                &ctx,
                adesk_proto::ActivateWindowParams { window_id },
            ))
            .await
            .map_err(backend_error)?;
        Ok(Some(action.action_id))
    }

    /// The live status of a recording in progress (`docs/viewer.md` §4).
    ///
    /// `frames` is read from the shared counter the capture task updates and
    /// `duration_ms` is measured from the runtime's monotonic clock, so the
    /// status reports progress rather than a frozen initial snapshot.
    fn active_status(&self, active: &ActiveRecording) -> RecordingStatus {
        RecordingStatus {
            recording: true,
            path: Some(active.path.display().to_string()),
            encoder: Some(active.encoder.clone()),
            fps: active.fps,
            frames: active.frames.load(Ordering::SeqCst),
            duration_ms: self.context.now_ms().saturating_sub(active.started_ms),
            error: None,
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

/// Wraps a server failure as a VAP backend failure, preserving its AGP code.
///
/// VAP reports a backend failure as an `error` message whose `code` reuses the AGP
/// [`adesk_core::ErrorCode`] vocabulary (`docs/viewer.md` §6), so the runtime's
/// classification travels both as that code and as the message text
/// ([`ServerError::code`]).
fn backend_error(error: ServerError) -> ViewerError {
    ViewerError::Backend {
        code: error.code(),
        message: error.to_string(),
    }
}

/// Maps a recorder failure to a VAP backend failure, preserving its AGP code.
///
/// [`adesk_recorder::RecorderError::code`] owns the classification, so an
/// unavailable `gpu` encoder arrives as `not_supported` and an unwritable path
/// as `internal`.
fn recorder_error(error: adesk_recorder::RecorderError) -> ViewerError {
    ViewerError::backend(error.code(), error.to_string())
}

/// The frame-capture half of a recording: renders the full output on a paced
/// interval and pushes each frame into the recorder session.
///
/// Built by `start_recording` and driven by the tokio task it spawns; the task
/// ends on an explicit stop or the runtime's shutdown token, then finalizes the
/// session and resolves with the recording's final status.
struct CaptureTask {
    /// Runtime state (for rendering and the shutdown token).
    context: ServerContext,
    /// The recorder session; owned here so the encode never touches the backend.
    session: RecordingSession,
    /// `true` once the recording has been stopped from the backend.
    stop: watch::Receiver<bool>,
    /// Frames queued so far, shared with the backend's status query.
    frames: Arc<AtomicU64>,
    /// Destination the recorder writes to.
    path: PathBuf,
    /// Resolved encoder backend name.
    encoder: String,
    /// Requested frame rate.
    fps: u32,
    /// Monotonic ms the recording started.
    started_ms: u64,
}

impl CaptureTask {
    /// Captures frames until stopped, then finalizes and returns the status.
    ///
    /// Every captured frame is a plain full-output render (no overlays) via
    /// [`crate::inspection::refresh`], the runtime's existing on-demand render
    /// seam; nothing renders while no recording runs.
    async fn run(mut self) -> RecordingStatus {
        let period = Duration::from_millis(u64::from(1000u32 / self.fps.max(1)));
        let mut ticker = tokio::time::interval(period);
        // A slow encode must not queue a burst of catch-up frames.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut failure: Option<String> = None;

        loop {
            tokio::select! {
                biased;
                // Shutdown wins over a pending stop so teardown stays ordered.
                () = self.context.shutdown.cancelled() => break,
                changed = self.stop.changed() => {
                    // A closed sender means the backend dropped the handle; either
                    // way this task is done.
                    if changed.is_err() || *self.stop.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    match crate::inspection::refresh(&self.context).await {
                        Ok(snapshot) => {
                            let ts_ms = self.context.now_ms();
                            if let Err(error) = self.session.push(snapshot.frame, ts_ms) {
                                failure = Some(error.to_string());
                                break;
                            }
                            self.frames.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
            }
        }

        if let Some(reason) = &failure {
            // Never log pixel payloads; the reason is a render/encoder message.
            tracing::debug!(reason, "viewer recording capture stopped early");
        }

        let path = self.path.clone();
        let encoder = self.encoder.clone();
        let fps = self.fps;
        let frames = self.frames.load(Ordering::SeqCst);
        let duration_ms = self.context.now_ms().saturating_sub(self.started_ms);
        let mut session = self.session;
        // `stop` joins the recorder's own OS thread, so it runs on a blocking
        // thread and never occupies a tokio worker.
        match tokio::task::spawn_blocking(move || session.stop()).await {
            Ok(Ok(summary)) => RecordingStatus {
                recording: false,
                path: Some(summary.path.display().to_string()),
                encoder: Some(summary.encoder),
                fps,
                frames: summary.frames,
                duration_ms: summary.duration_ms,
                error: None,
            },
            Ok(Err(error)) => RecordingStatus {
                recording: false,
                path: Some(path.display().to_string()),
                encoder: Some(encoder),
                fps,
                frames,
                duration_ms,
                error: Some(error.to_string()),
            },
            Err(join_error) => RecordingStatus {
                recording: false,
                path: Some(path.display().to_string()),
                encoder: Some(encoder),
                fps,
                frames,
                duration_ms,
                error: Some(format!("recording task failed: {join_error}")),
            },
        }
    }
}

/// Resolves the window a pointer/key/text viewer input targets, its geometry and
/// the output size.
///
/// Every such input targets the window the compositor's `inject_*` resolves
/// against: the keyboard focus, else the active window (`docs/viewer.md` §5). No
/// candidate window is `invalid_request`; a candidate that vanished is
/// `unknown_window`.
async fn input_target(server: &ServerContext) -> ViewerResult<(WindowId, Rect, Size)> {
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
    Ok((target, rect, server.compositor.output_size()))
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

        match input {
            ViewerInput::ActivateWindow { window_id } => self.activate_window(window_id).await,
            ViewerInput::PointerMove { x, y } => {
                let (target, rect, output) = input_target(server).await?;
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
                let (target, rect, output) = input_target(server).await?;
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
                let (target, rect, output) = input_target(server).await?;
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
                let (target, _, _) = input_target(server).await?;
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
                let (target, _, _) = input_target(server).await?;
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

    async fn start_recording(&self, request: RecordRequest) -> ViewerResult<RecordingStatus> {
        // The whole start path is synchronous (create dir, open the file, spawn
        // the recorder thread and the capture task), so the check-and-set is
        // atomic under the lock and no guard is held across an await.
        let mut state = self
            .recording
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.active.is_some() {
            return Err(ViewerError::backend(
                ErrorCode::InvalidRequest,
                "a recording is already in progress",
            ));
        }

        // Resolve the encoder first so the extension matches the backend the
        // recorder will really use (`detect` folds `Auto` onto the platform).
        let requested = crate::translate::recorder_encoder(request.encoder);
        let kind = adesk_recorder::detect(requested);
        let path = match &request.path {
            Some(path) => path.clone(),
            None => {
                let dir = self.context.config.recordings_dir();
                std::fs::create_dir_all(&dir).map_err(|error| {
                    ViewerError::backend(
                        ErrorCode::Internal,
                        format!("cannot create recordings dir `{}`: {error}", dir.display()),
                    )
                })?;
                dir.join(format!(
                    "recording-{}{}",
                    self.context.now_ms(),
                    adesk_recorder::suggest_extension(kind)
                ))
            }
        };

        let config = RecorderConfig::new(&path)
            .with_fps(request.fps)
            .with_encoder(kind);
        // `RecordingSession::start` opens the file (so an unwritable path fails
        // here) and owns the encode on its own OS thread.
        let session = RecordingSession::start(config).map_err(recorder_error)?;
        let encoder = session.encoder_name().to_owned();

        let started_ms = self.context.now_ms();
        let frames = Arc::new(AtomicU64::new(0));
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(
            CaptureTask {
                context: self.context.clone(),
                session,
                stop: stop_rx,
                frames: Arc::clone(&frames),
                path: path.clone(),
                encoder: encoder.clone(),
                fps: request.fps,
                started_ms,
            }
            .run(),
        );

        state.active = Some(ActiveRecording {
            path: path.clone(),
            encoder: encoder.clone(),
            fps: request.fps,
            started_ms,
            frames,
            stop,
            task,
        });

        Ok(RecordingStatus {
            recording: true,
            path: Some(path.display().to_string()),
            encoder: Some(encoder),
            fps: request.fps,
            frames: 0,
            duration_ms: 0,
            error: None,
        })
    }

    async fn stop_recording(&self) -> ViewerResult<RecordingStatus> {
        let active = {
            let mut state = self
                .recording
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match state.active.take() {
                Some(active) => active,
                None => {
                    return Err(ViewerError::backend(
                        ErrorCode::InvalidRequest,
                        "no recording is in progress",
                    ))
                }
            }
        };

        // Ask the capture task to finalize; it owns the session. A closed signal
        // means the task already ended (e.g. shutdown) — its join handle still
        // carries the final status.
        let _ = active.stop.send(true);
        let status = active.task.await.map_err(|error| {
            ViewerError::backend(
                ErrorCode::Internal,
                format!("recording task failed: {error}"),
            )
        })?;

        let mut state = self
            .recording
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.last = Some(status.clone());
        Ok(status)
    }

    async fn recording_status(&self) -> ViewerResult<RecordingStatus> {
        let state = self
            .recording
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &state.active {
            Some(active) => Ok(self.active_status(active)),
            None => Ok(state.last.clone().unwrap_or_else(RecordingStatus::idle)),
        }
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
