//! The tokio ↔ GLib bridge: a background OS thread drives [`ViewerClient`] and
//! marshals to/from the GTK main thread over unbounded channels.
//!
//! The GTK main loop must never block on the network, and the tokio worker must
//! never touch GTK objects. So this module owns both seams:
//! - the **worker** runs a single-thread tokio runtime that connects, performs
//!   the handshake, streams frames and applies input commands; it sends
//!   [`UiEvent`]s to the GTK thread and receives [`InputCommand`]s from it;
//! - the GTK side gets a cheap, cloneable [`InputHandle`] to enqueue commands and
//!   an [`UnboundedReceiver`] of events to drive a `glib::spawn_future_local`
//!   loop.
//!
//! The channels are plain `tokio::sync::mpsc` channels; the worker does **not**
//! use `glib`, and the GTK side does **not** block. Message bodies and pixel
//! payloads are never logged, and human text is never logged at all.
//!
//! # Input is never starved by a request
//!
//! A viewer's seat input must not wait for a server round trip: a `state`, a
//! render, a recording query or an app-list reply can take arbitrarily long, and
//! a click or a keystroke queued behind one would feel laggy or lost. So the
//! worker's loop applies seat input itself (see [`apply_seat`]), while every
//! request/response call is handed to a dedicated [`request_worker`] task that
//! shares the same one [`ViewerClient`]. The client's write half is behind a
//! single lock, so whichever side originates a message the connection keeps its
//! **single writer** and the NDJSON stream stays aligned.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use adesk_core::{AppId, Button, ButtonState, WindowId};
use adesk_proto::KeySpec;
use adesk_viewer::{RecordRequest, ViewerClient, ViewerError, ViewerTarget};
use adesk_viewer_proto::{
    AppEntry, ControlOwner, CursorState, DesktopState, KeyAction, LaunchOutcome, RecordingStatus,
    ServerHello, ViewerFrame,
};

use crate::address::{connect_failure, unexpected_close};
use crate::image::{decode, DecodedImage};

/// How often the task bar is refreshed even without a change (`docs/viewer.md` §3).
const STATE_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// One human input action, produced by the GTK layer and applied by the worker.
///
/// Positions are **normalized** `0.0..=1.0` output fractions (never pixels).
/// `Text` deliberately has no `Debug` implementation so its contents can never
/// end up in a log line.
pub(crate) enum InputCommand {
    /// Move the pointer to the normalized position.
    Move {
        /// Horizontal output fraction.
        x: f64,
        /// Vertical output fraction.
        y: f64,
    },
    /// Press or release a mouse button at the normalized position.
    Button {
        /// Which button.
        button: Button,
        /// Whether it went down or up.
        state: ButtonState,
        /// Horizontal output fraction.
        x: f64,
        /// Vertical output fraction.
        y: f64,
    },
    /// Scroll by a delta at the normalized position.
    Scroll {
        /// Horizontal scroll delta.
        dx: f64,
        /// Vertical scroll delta.
        dy: f64,
        /// Horizontal output fraction.
        x: f64,
        /// Vertical output fraction.
        y: f64,
    },
    /// Press or release a seat-native key (or chord).
    Key {
        /// The key(s) to apply.
        keys: KeySpec,
        /// Whether to press, release or tap.
        action: KeyAction,
    },
    /// Type committed text.
    Text(String),
    /// Activate a window — the runtime-native `activate_window` path, never
    /// synthesized input.
    ActivateWindow(WindowId),
    /// Close a window — the runtime-native `close_window` path, which the
    /// runtime answers with an ack or an `unknown_window` error (`docs/viewer.md`
    /// §4, §5).
    CloseWindow(WindowId),
    /// Fetch the runtime's launchable applications (one round trip; the launcher
    /// filters the result locally).
    ListApps,
    /// Launch an application through the runtime's app registry.
    LaunchApp(AppId),
    /// Start a screen recording with the given request (`docs/viewer.md` §4).
    StartRecording(RecordRequest),
    /// Stop the active screen recording.
    StopRecording,
}

/// One event the worker reports to the GTK thread.
#[derive(Debug)]
pub(crate) enum UiEvent {
    /// The handshake succeeded; carries the server's display metadata.
    Connected {
        /// The resolved endpoint, for display.
        target: String,
        /// The server handshake payload (output size, renderer, cursor, control).
        hello: ServerHello,
    },
    /// A fresh desktop state (window list + active window).
    State(DesktopState),
    /// A decoded desktop frame.
    Frame {
        /// Frame sequence in the runtime's monotonic domain.
        seq: u64,
        /// Monotonic milliseconds since runtime start.
        ts_ms: u64,
        /// The decoded desktop image.
        image: DecodedImage,
        /// The remote pointer's position and visibility at render time
        /// (normalized `0.0..=1.0` output fractions).
        cursor: CursorState,
        /// The window the frame targets, when one is active.
        active_window_id: Option<WindowId>,
    },
    /// The endpoint could not be connected to at all; carries a
    /// human-readable reason naming the dialed endpoint.
    ConnectFailed(String),
    /// The connection ended; carries a human-readable reason naming the
    /// endpoint it happened on.
    Disconnected(String),
    /// A screen-recording status (or the failure of a recording command).
    ///
    /// The `Err` arm carries the failure's `Display` so it can be surfaced in
    /// the UI without leaking the error type into the GTK layer.
    Recording(Result<RecordingStatus, String>),
    /// The runtime's launchable applications (or the failure of the `list_apps`
    /// request), for the application launcher.
    Apps(Result<Vec<AppEntry>, String>),
    /// The outcome of a `launch_app` request.
    ///
    /// The `Err` arm carries the failure's `Display` (e.g. `unknown_app`), so a
    /// refusal is surfaced like the successful reply.
    Launch(Result<LaunchOutcome, String>),
    /// The outcome of a `close_window` command.
    ///
    /// `Ok(())` once the runtime acknowledged the close; the `Err` arm carries
    /// the failure's `Display` — notably `unknown_window` for a window that was
    /// already gone. The connection stays open either way.
    WindowClosed {
        /// The window the close was requested for.
        window_id: WindowId,
        /// The outcome, or the failure's `Display`.
        result: Result<(), String>,
    },
    /// A non-fatal notice (e.g. a frame that could not be decoded).
    Notice(String),
}

/// A cheap, cloneable handle to enqueue [`InputCommand`]s for the worker.
///
/// The GTK widgets all share one handle. Dropping it — or calling [`close`]
/// explicitly on window teardown — drops the single underlying sender, which
/// ends the worker thread (its `recv()` returns `None`).
///
/// [`close`]: InputHandle::close
#[derive(Clone)]
pub(crate) struct InputHandle {
    sender: Rc<RefCell<Option<UnboundedSender<InputCommand>>>>,
}

impl InputHandle {
    /// Enqueues `command`, ignoring the send when the worker has already gone.
    pub(crate) fn send(&self, command: InputCommand) {
        match self.sender.borrow().as_ref() {
            Some(sender) => {
                if sender.send(command).is_err() {
                    tracing::debug!("viewer input channel is already closed");
                }
            }
            None => tracing::debug!("viewer input channel is closed"),
        }
    }

    /// Closes the input channel, ending the worker thread.
    pub(crate) fn close(&self) {
        self.sender.borrow_mut().take();
    }
}

/// The GTK-side end of the bridge: the input handle plus the event receiver.
pub(crate) struct Bridge {
    /// Enqueues input for the worker.
    input: InputHandle,
    /// Receives events from the worker.
    events: UnboundedReceiver<UiEvent>,
}

impl Bridge {
    /// Spawns the worker thread and returns the GTK-side bridge.
    ///
    /// The worker is a plain OS thread running a single-thread tokio runtime, so
    /// connecting never blocks the GTK main loop. A failure to *connect* arrives
    /// later as [`UiEvent::Disconnected`]; a failure to *start the thread* is
    /// reported the same way.
    pub(crate) fn connect(target: ViewerTarget) -> Bridge {
        let (input_tx, input_rx) = unbounded_channel();
        let (event_tx, event_rx) = unbounded_channel();
        let worker_events = event_tx.clone();

        let worker_target = target.clone();
        let spawned = std::thread::Builder::new()
            .name("adesk-viewer-gui-bridge".to_owned())
            .spawn(move || worker(worker_target, input_rx, worker_events));

        if let Err(error) = spawned {
            // The thread never started, so report a terminal disconnect and drop
            // the (unused) input receiver with the closure.
            let _ = event_tx.send(UiEvent::Disconnected(format!(
                "could not start the viewer bridge thread (viewer socket {target}): {error}"
            )));
        }

        Bridge {
            input: InputHandle {
                sender: Rc::new(RefCell::new(Some(input_tx))),
            },
            events: event_rx,
        }
    }

    /// A cloneable handle for enqueuing input commands.
    pub(crate) fn input(&self) -> InputHandle {
        self.input.clone()
    }

    /// Consumes the bridge and yields the event receiver for the GTK loop.
    ///
    /// Callers must take an [`input`](Bridge::input) handle *before* this, since
    /// it drops the bridge's own input sender.
    pub(crate) fn into_events(self) -> UnboundedReceiver<UiEvent> {
        self.events
    }
}

/// The worker entry point: builds a current-thread runtime and drives the
/// session. Never panics.
fn worker(
    target: ViewerTarget,
    input: UnboundedReceiver<InputCommand>,
    events: UnboundedSender<UiEvent>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = events.send(UiEvent::Disconnected(format!(
                "could not start the viewer async runtime (viewer socket {target}): {error}"
            )));
            return;
        }
    };

    let mut input = input;
    runtime.block_on(session(target, &mut input, &events));
}

/// Connects, then runs the frame/input/refresh loop until the connection or the
/// GTK side goes away. Best-effort throughout: no failure is fatal.
///
/// The loop applies seat input itself, so a queued click or keystroke is never
/// delayed by a slow server round trip; every request/response call is handed to
/// a [`request_worker`] task that shares the same one [`ViewerClient`], so the
/// connection keeps its single writer (`docs/viewer.md` §5).
async fn session(
    target: ViewerTarget,
    input: &mut UnboundedReceiver<InputCommand>,
    events: &UnboundedSender<UiEvent>,
) {
    let client = match ViewerClient::connect(target.clone()).await {
        Ok(client) => client,
        Err(error) => {
            let _ = events.send(UiEvent::ConnectFailed(connect_failure(&target, &error)));
            return;
        }
    };

    let _ = events.send(UiEvent::Connected {
        target: target.to_string(),
        hello: client.hello().clone(),
    });

    // Advisory ownership handshake: announce that a human is driving.
    if let Err(error) = client.set_control(ControlOwner::Human).await {
        tracing::debug!(%error, "viewer control handshake failed");
    }

    // The one connection is shared by this loop and the requester task. Both go
    // through the client's single write half, so the NDJSON stream keeps its one
    // writer and stays aligned whichever side originates a message.
    let client = Arc::new(client);
    let (jobs_tx, jobs_rx) = unbounded_channel();
    let requester = tokio::spawn(request_worker(Arc::clone(&client), jobs_rx, events.clone()));

    // Seed the task bar and the view through the requester, so these round trips
    // never hold up the input this loop starts applying at once. The recording
    // status is read too, so a viewer attaching mid-recording learns about it and
    // the requester can gate its periodic status refresh.
    let _ = jobs_tx.send(RequestJob::State);
    let _ = jobs_tx.send(RequestJob::Frame);
    let _ = jobs_tx.send(RequestJob::Recording);

    let mut frames = std::pin::pin!(client.frames());
    let mut last_active: Option<WindowId> = None;

    // Refresh the task bar on a modest timer as well as on an active-window
    // change. The requester serves each refresh after the previous one answered,
    // so at most one `request_state` is ever in flight (inherent coalescing) and
    // create/destroy/title changes converge. The recording status is piggybacked
    // on the same tick, but only while a recording is active, so an idle viewer
    // sends no extra traffic.
    let mut ticker = tokio::time::interval(STATE_REFRESH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the interval's immediately-ready first tick (the initial state
    // request above already ran).
    ticker.tick().await;

    loop {
        tokio::select! {
            command = input.recv() => match command {
                Some(command) => match route(command) {
                    // Seat input is applied on this path: its only await is the
                    // socket write, so a clicking or typing human is never left
                    // waiting behind a state, recording or app-list reply.
                    Routed::Seat(command) => apply_seat(&client, command, events).await,
                    // A request/response call is served off this path, in
                    // submission order, by the requester task.
                    Routed::Request(job) => {
                        let _ = jobs_tx.send(job);
                    }
                },
                None => break,
            },
            frame = frames.next() => match frame {
                Some(Ok(frame)) => {
                    emit_frame(&frame, events);
                    if frame.active_window_id != last_active {
                        last_active = frame.active_window_id;
                        let _ = jobs_tx.send(RequestJob::State);
                    }
                }
                Some(Err(error)) => {
                    tracing::debug!(%error, "viewer frame stream error");
                    let _ = events.send(UiEvent::Notice(format!("frame stream error: {error}")));
                }
                None => {
                    let _ = events.send(UiEvent::Disconnected(unexpected_close(
                        &target,
                        ViewerError::Closed,
                    )));
                    break;
                }
            },
            _ = ticker.tick() => {
                let _ = jobs_tx.send(RequestJob::Tick);
            },
        }
    }

    // Stop the requester first — aborting a request still parked in it — and only
    // then take the connection back for a clean close. Awaiting the aborted task
    // drops its `Arc`, which is what lets `Arc::try_unwrap` succeed.
    drop(jobs_tx);
    requester.abort();
    let _ = requester.await;
    match Arc::try_unwrap(client) {
        Ok(client) => {
            if let Err(error) = client.close().await {
                tracing::debug!(%error, "viewer close failed");
            }
        }
        // Unreachable: the requester (the only other holder) is gone. Dropping
        // the connection still stops the dispatcher and wakes every stream.
        Err(_) => tracing::debug!("viewer connection was still shared at shutdown"),
    }
}

/// Decodes `frame` and forwards it, turning a decode failure into a notice.
///
/// The frame's remote cursor rides along, so the GTK layer can draw the pointer
/// on top of the pixels it just painted.
fn emit_frame(frame: &ViewerFrame, events: &UnboundedSender<UiEvent>) {
    match decode(&frame.image) {
        Ok(image) => {
            let _ = events.send(UiEvent::Frame {
                seq: frame.seq,
                ts_ms: frame.ts_ms,
                image,
                cursor: frame.cursor.clone(),
                active_window_id: frame.active_window_id,
            });
        }
        Err(error) => {
            let _ = events.send(UiEvent::Notice(format!(
                "could not decode a desktop frame: {error}"
            )));
        }
    }
}

/// Requests the desktop state and forwards it, logging (never fatal) on error.
async fn refresh_state(client: &ViewerClient, events: &UnboundedSender<UiEvent>) {
    match client.request_state().await {
        Ok(state) => {
            let _ = events.send(UiEvent::State(state));
        }
        Err(error) => tracing::debug!(%error, "viewer desktop state refresh failed"),
    }
}

/// Requests the current recording status and forwards it; returns whether a
/// recording is active. Logs (never fatal) on error.
async fn refresh_recording(client: &ViewerClient, events: &UnboundedSender<UiEvent>) -> bool {
    match client.request_recording().await {
        Ok(status) => {
            let recording = status.recording;
            let _ = events.send(UiEvent::Recording(Ok(status)));
            recording
        }
        Err(error) => {
            tracing::debug!(%error, "viewer recording status refresh failed");
            false
        }
    }
}

/// Forwards a recording command's outcome; returns whether a recording is now
/// active, so the worker can gate its periodic status refresh.
fn emit_recording(
    result: Result<RecordingStatus, String>,
    events: &UnboundedSender<UiEvent>,
) -> bool {
    match result {
        Ok(status) => {
            let recording = status.recording;
            let _ = events.send(UiEvent::Recording(Ok(status)));
            recording
        }
        Err(message) => {
            tracing::debug!(%message, "viewer recording command failed");
            let _ = events.send(UiEvent::Recording(Err(message)));
            false
        }
    }
}

/// One request/response call the worker serves **off** its input path.
///
/// Every call here awaits a server reply, so applying one inline would leave the
/// pointer and keyboard input queued behind it unread for as long as the round
/// trip takes. [`session`] hands each to [`request_worker`] instead, which owns
/// the same one [`ViewerClient`] the loop applies seat input with — so the
/// connection keeps its single writer.
#[derive(Debug)]
enum RequestJob {
    /// `request_state`: refresh the desktop metadata (the periodic tick and the
    /// active-window change).
    State,
    /// The periodic tick: refresh the state and, while a recording is active, the
    /// recording status too.
    Tick,
    /// `request_frame`: render one frame, for the initial view.
    Frame,
    /// `request_recording`: report the recording status without changing it (the
    /// initial detection, so a viewer attaching mid-recording knows).
    Recording,
    /// `list_apps`: the application launcher's registry fetch.
    Apps,
    /// `launch_app`, followed by a `request_state` so the launched window is
    /// discovered (the reply never carries it).
    Launch(AppId),
    /// `start_recording`.
    StartRecording(RecordRequest),
    /// `stop_recording`.
    StopRecording,
}

/// Where one [`InputCommand`] is applied.
enum Routed {
    /// Applied on the input path by [`apply_seat`], whose only await is the socket
    /// write — never a reply.
    Seat(InputCommand),
    /// Handed to [`request_worker`] as a [`RequestJob`], so awaiting its reply
    /// cannot delay the input queued behind it.
    Request(RequestJob),
}

/// Routes `command` onto the input path or to the requester task
/// (`docs/viewer.md` §4, §5).
///
/// Seat input — pointer, key and text — and the runtime-native window actions
/// (`activate_window`, `close_window`) are applied immediately, in submission
/// order. Every command whose point is its *reply* — the app-registry listing,
/// the app launch and the recording transitions — becomes a [`RequestJob`].
fn route(command: InputCommand) -> Routed {
    match command {
        InputCommand::ListApps => Routed::Request(RequestJob::Apps),
        InputCommand::LaunchApp(app_id) => Routed::Request(RequestJob::Launch(app_id)),
        InputCommand::StartRecording(request) => {
            Routed::Request(RequestJob::StartRecording(request))
        }
        InputCommand::StopRecording => Routed::Request(RequestJob::StopRecording),
        command => Routed::Seat(command),
    }
}

/// Serves the [`RequestJob`]s [`session`] hands it, one at a time, off the input
/// path.
///
/// It shares the one [`ViewerClient`] with the loop, so every write still goes
/// through the client's single write half; only *where the reply is awaited* moved
/// off the loop. The session aborts this task when it ends.
async fn request_worker(
    client: Arc<ViewerClient>,
    mut jobs: UnboundedReceiver<RequestJob>,
    events: UnboundedSender<UiEvent>,
) {
    // Whether a recording is active, so the periodic tick only asks for the
    // status while one runs (an idle viewer sends no extra traffic).
    let mut recording = false;
    while let Some(job) = jobs.recv().await {
        match job {
            RequestJob::State => refresh_state(&client, &events).await,
            RequestJob::Tick => {
                refresh_state(&client, &events).await;
                if recording {
                    recording = refresh_recording(&client, &events).await;
                }
            }
            RequestJob::Frame => match client.request_frame().await {
                Ok(frame) => emit_frame(&frame, &events),
                Err(error) => tracing::debug!(%error, "initial viewer frame request failed"),
            },
            RequestJob::Recording => {
                recording = refresh_recording(&client, &events).await;
            }
            RequestJob::Apps => {
                let result = client
                    .list_apps(None)
                    .await
                    .map_err(|error| error.to_string());
                let _ = events.send(UiEvent::Apps(result));
            }
            RequestJob::Launch(app_id) => {
                let result = client
                    .launch_app(app_id)
                    .await
                    .map_err(|error| error.to_string());
                let launched = result.is_ok();
                let _ = events.send(UiEvent::Launch(result));
                if launched {
                    // The reply never carries the launched window, so the window
                    // is discovered by re-reading the desktop state right away
                    // instead of waiting up to a refresh tick for it to appear in
                    // the task bar.
                    refresh_state(&client, &events).await;
                }
            }
            RequestJob::StartRecording(request) => {
                let result = client
                    .start_recording(request)
                    .await
                    .map_err(|error| error.to_string());
                recording = emit_recording(result, &events);
            }
            RequestJob::StopRecording => {
                let result = client
                    .stop_recording()
                    .await
                    .map_err(|error| error.to_string());
                recording = emit_recording(result, &events);
            }
        }
    }
}

/// Applies one seat input command on the input path, logging and noticing (never
/// failing) on error.
///
/// Every arm is a fire-and-forget write: the runtime acknowledges seat input and
/// `close_window` with protocol messages the worker does not await, so the only
/// await here is the socket write itself — never a reply. That is what keeps a
/// queued click or keystroke from waiting behind a slow request
/// (`docs/viewer.md` §5). The request/response commands never reach this path
/// ([`route`] sends them to [`request_worker`]).
async fn apply_seat(
    client: &ViewerClient,
    command: InputCommand,
    events: &UnboundedSender<UiEvent>,
) {
    let result = match command {
        InputCommand::CloseWindow(window_id) => {
            let result = client
                .close_window(window_id)
                .await
                .map_err(|error| error.to_string());
            let _ = events.send(UiEvent::WindowClosed { window_id, result });
            return;
        }
        InputCommand::Move { x, y } => client.pointer_move(x, y).await,
        InputCommand::Button {
            button,
            state,
            x,
            y,
        } => client.pointer_button(button, state, Some((x, y))).await,
        InputCommand::Scroll { dx, dy, x, y } => client.scroll(dx, dy, Some((x, y))).await,
        InputCommand::Key { keys, action } => client.key(keys, action).await,
        InputCommand::Text(text) => client.text(text).await,
        InputCommand::ActivateWindow(window_id) => client.activate_window(window_id).await,
        // Unreachable: `route` sends these to the requester task. Handled like a
        // no-op rather than a panic, so a mis-route can never take the worker down.
        InputCommand::ListApps
        | InputCommand::LaunchApp(_)
        | InputCommand::StartRecording(_)
        | InputCommand::StopRecording => {
            tracing::debug!("a request command reached the seat input path");
            return;
        }
    };

    if let Err(error) = result {
        // Never log the text body; `ViewerError` never carries it.
        tracing::debug!(%error, "viewer input command failed");
        let _ = events.send(UiEvent::Notice(format!("input not delivered: {error}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_core::{ActionId, Size};
    use adesk_proto::{ImagePayload, RendererKind};
    use adesk_viewer::{PeerInfo, ViewerBackend, ViewerInput, ViewerServer};
    use adesk_viewer_proto::PROTOCOL_VERSION;
    use tokio::net::UnixListener;

    /// A frame fixture carrying `image` and a visible remote cursor.
    fn frame(image: ImagePayload) -> ViewerFrame {
        ViewerFrame {
            seq: 7,
            ts_ms: 123,
            image,
            cursor: CursorState::at(0.25, 0.75),
            active_window_id: Some(WindowId(3)),
        }
    }

    #[test]
    fn a_decodable_frame_becomes_a_frame_event() {
        let (events, mut receiver) = unbounded_channel();
        let payload = ImagePayload::from_rgba8(1, 1, &[1, 2, 3, 255], 1.0).unwrap();

        emit_frame(&frame(payload), &events);

        match receiver.try_recv().unwrap() {
            UiEvent::Frame {
                seq,
                ts_ms,
                image,
                cursor,
                active_window_id,
            } => {
                assert_eq!(seq, 7);
                assert_eq!(ts_ms, 123);
                assert_eq!(image.width, 1);
                assert_eq!(image.height, 1);
                assert_eq!(image.rgba8, vec![1, 2, 3, 255]);
                assert_eq!(active_window_id, Some(WindowId(3)));
                // The remote pointer travels with the pixels, so the GUI can
                // draw it over the frame it just decoded.
                assert_eq!(cursor, CursorState::at(0.25, 0.75));
            }
            other => panic!("expected a frame event, got {other:?}"),
        }
    }

    #[test]
    fn a_hidden_cursor_is_still_reported() {
        let (events, mut receiver) = unbounded_channel();
        let payload = ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0).unwrap();
        let mut frame = frame(payload);
        frame.cursor = CursorState::hidden();

        emit_frame(&frame, &events);

        match receiver.try_recv().unwrap() {
            UiEvent::Frame { cursor, .. } => assert!(!cursor.visible),
            other => panic!("expected a frame event, got {other:?}"),
        }
    }

    #[test]
    fn an_undecodable_frame_becomes_a_notice() {
        let (events, mut receiver) = unbounded_channel();
        let payload = ImagePayload::from_png(2, 2, b"not a png", 1.0);

        emit_frame(&frame(payload), &events);

        assert!(matches!(receiver.try_recv().unwrap(), UiEvent::Notice(_)));
    }

    #[test]
    fn dropping_the_input_handle_ends_the_session() {
        // With no sender alive, the worker's `recv()` would resolve to `None`;
        // here we assert the handle simply stops accepting commands.
        let (sender, mut receiver) = unbounded_channel();
        let handle = InputHandle {
            sender: Rc::new(RefCell::new(Some(sender))),
        };
        handle.send(InputCommand::ActivateWindow(WindowId(1)));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            InputCommand::ActivateWindow(WindowId(1))
        ));

        handle.close();
        handle.send(InputCommand::Move { x: 0.5, y: 0.5 });
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn a_recording_status_becomes_a_recording_event() {
        let (events, mut receiver) = unbounded_channel();
        let status = RecordingStatus::idle()
            .with_recording(true)
            .with_counts(3, 500);

        let active = emit_recording(Ok(status.clone()), &events);

        assert!(active);
        match receiver.try_recv().unwrap() {
            UiEvent::Recording(Ok(received)) => assert_eq!(received, status),
            other => panic!("expected a recording event, got {other:?}"),
        }
    }

    #[test]
    fn a_recording_error_becomes_an_error_event() {
        let (events, mut receiver) = unbounded_channel();

        let active = emit_recording(Err("no encoder".to_owned()), &events);

        assert!(!active);
        match receiver.try_recv().unwrap() {
            UiEvent::Recording(Err(message)) => assert_eq!(message, "no encoder"),
            other => panic!("expected a recording error, got {other:?}"),
        }
    }

    #[test]
    fn recording_commands_reach_the_worker() {
        let (sender, mut receiver) = unbounded_channel();
        let handle = InputHandle {
            sender: Rc::new(RefCell::new(Some(sender))),
        };

        handle.send(InputCommand::StartRecording(RecordRequest::default()));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            InputCommand::StartRecording(_)
        ));

        handle.send(InputCommand::StopRecording);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            InputCommand::StopRecording
        ));
    }

    #[test]
    fn launcher_commands_reach_the_worker() {
        let (sender, mut receiver) = unbounded_channel();
        let handle = InputHandle {
            sender: Rc::new(RefCell::new(Some(sender))),
        };

        handle.send(InputCommand::ListApps);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            InputCommand::ListApps
        ));

        let app_id = AppId::from("org.mozilla.firefox");
        handle.send(InputCommand::LaunchApp(app_id.clone()));
        match receiver.try_recv().unwrap() {
            InputCommand::LaunchApp(received) => assert_eq!(received, app_id),
            // `InputCommand` deliberately has no `Debug` (its `Text` arm must
            // never reach a log), so the mismatch is reported by variant name.
            _ => panic!("expected a launch command"),
        }
    }

    #[test]
    fn a_close_command_reaches_the_worker() {
        let (sender, mut receiver) = unbounded_channel();
        let handle = InputHandle {
            sender: Rc::new(RefCell::new(Some(sender))),
        };

        handle.send(InputCommand::CloseWindow(WindowId(9)));
        match receiver.try_recv().unwrap() {
            InputCommand::CloseWindow(window_id) => assert_eq!(window_id, WindowId(9)),
            _ => panic!("expected a close command"),
        }
    }

    #[test]
    fn launcher_events_carry_their_replies() {
        let (events, mut receiver) = unbounded_channel();
        let app = AppEntry {
            id: AppId::from("org.mozilla.firefox"),
            name: "Firefox".to_owned(),
            icon: None,
            categories: Vec::new(),
        };

        let _ = events.send(UiEvent::Apps(Ok(vec![app.clone()])));
        match receiver.try_recv().unwrap() {
            UiEvent::Apps(Ok(apps)) => assert_eq!(apps, vec![app.clone()]),
            other => panic!("expected an apps event, got {other:?}"),
        }

        let _ = events.send(UiEvent::Apps(Err("not_supported".to_owned())));
        match receiver.try_recv().unwrap() {
            UiEvent::Apps(Err(message)) => assert_eq!(message, "not_supported"),
            other => panic!("expected an apps error, got {other:?}"),
        }

        let _ = events.send(UiEvent::Launch(Ok(LaunchOutcome {
            app_id: app.id.clone(),
            launch_id: adesk_core::LaunchId(3),
            action_id: None,
            window_id: None,
        })));
        match receiver.try_recv().unwrap() {
            UiEvent::Launch(Ok(outcome)) => {
                assert_eq!(outcome.app_id, app.id);
                assert_eq!(outcome.window_id, None);
            }
            other => panic!("expected a launch event, got {other:?}"),
        }

        let _ = events.send(UiEvent::WindowClosed {
            window_id: WindowId(2),
            result: Err("backend error: unknown window 2".to_owned()),
        });
        match receiver.try_recv().unwrap() {
            UiEvent::WindowClosed { window_id, result } => {
                assert_eq!(window_id, WindowId(2));
                assert!(result.is_err());
            }
            other => panic!("expected a window-closed event, got {other:?}"),
        }
    }

    /// Every seat command stays on the input path, so a queued click or
    /// keystroke is applied without a server round trip.
    #[test]
    fn seat_input_is_routed_to_the_input_path() {
        let seat = [
            InputCommand::Move { x: 0.1, y: 0.2 },
            InputCommand::Button {
                button: Button::Left,
                state: ButtonState::Pressed,
                x: 0.1,
                y: 0.2,
            },
            InputCommand::Scroll {
                dx: 0.0,
                dy: -1.0,
                x: 0.1,
                y: 0.2,
            },
            InputCommand::Key {
                keys: KeySpec::Single("Return".to_owned()),
                action: KeyAction::Tap,
            },
            InputCommand::Text("hello".to_owned()),
            InputCommand::ActivateWindow(WindowId(1)),
            InputCommand::CloseWindow(WindowId(2)),
        ];

        for command in seat {
            assert!(
                matches!(route(command), Routed::Seat(_)),
                "a seat command must be applied on the input path"
            );
        }
    }

    /// Every request/response command is handed to the requester task, so its
    /// reply is awaited off the input path.
    #[test]
    fn request_commands_are_routed_to_the_requester() {
        assert!(matches!(
            route(InputCommand::ListApps),
            Routed::Request(RequestJob::Apps)
        ));
        assert!(matches!(
            route(InputCommand::StopRecording),
            Routed::Request(RequestJob::StopRecording)
        ));
        assert!(matches!(
            route(InputCommand::StartRecording(RecordRequest::default())),
            Routed::Request(RequestJob::StartRecording(_))
        ));

        let app_id = AppId::from("org.mozilla.firefox");
        match route(InputCommand::LaunchApp(app_id.clone())) {
            Routed::Request(RequestJob::Launch(routed)) => assert_eq!(routed, app_id),
            _ => panic!("expected a launch request"),
        }
    }

    /// A VAP backend that parks every `list_apps` until the test releases it and
    /// records the input it receives, so the suite can prove a request/response
    /// round trip in flight does not delay the seat input queued behind it.
    ///
    /// It mirrors `adesk-viewer`'s `GatedBackend`, which guards the same property
    /// on the session side of the wire.
    struct GatedBackend {
        /// Signalled once a `list_apps` has been entered (a `notify_one` that
        /// arrived first is remembered, so the test cannot miss it).
        listing: tokio::sync::Notify,
        /// One permit per `list_apps` the test allows to finish; starts empty, so
        /// every listing parks.
        release: tokio::sync::Semaphore,
        /// Signalled once an input has been applied (and remembered, so the test
        /// cannot miss it either).
        applied: tokio::sync::Notify,
        /// The inputs `apply_input` received, in submission order.
        inputs: std::sync::Mutex<Vec<ViewerInput>>,
    }

    impl GatedBackend {
        /// A backend whose listings all park.
        fn new() -> GatedBackend {
            GatedBackend {
                listing: tokio::sync::Notify::new(),
                release: tokio::sync::Semaphore::new(0),
                applied: tokio::sync::Notify::new(),
                inputs: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// Resolves once a `list_apps` call has been entered.
        async fn listing(&self) {
            self.listing.notified().await;
        }

        /// Resolves once an input has been applied.
        async fn applied(&self) {
            self.applied.notified().await;
        }

        /// Lets one parked `list_apps` finish.
        fn release_listing(&self) {
            self.release.add_permits(1);
        }

        /// The inputs recorded so far, in submission order.
        fn recorded_inputs(&self) -> Vec<ViewerInput> {
            self.inputs.lock().expect("the inputs lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl ViewerBackend for GatedBackend {
        fn display(&self) -> ServerHello {
            ServerHello {
                protocol_version: PROTOCOL_VERSION,
                runtime_version: "gated".to_owned(),
                output: Size::new(8, 4),
                renderer: RendererKind::Pixman,
                cursor: CursorState::hidden(),
                control: ControlOwner::Human,
            }
        }

        async fn render_frame(&self) -> adesk_viewer::Result<ViewerFrame> {
            Ok(ViewerFrame {
                seq: 1,
                ts_ms: 0,
                image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0)
                    .expect("a valid payload"),
                cursor: CursorState::hidden(),
                active_window_id: None,
            })
        }

        async fn desktop_state(&self) -> adesk_viewer::Result<DesktopState> {
            Ok(DesktopState {
                active_window_id: None,
                windows: Vec::new(),
            })
        }

        async fn apply_input(&self, input: ViewerInput) -> adesk_viewer::Result<Option<ActionId>> {
            self.inputs.lock().expect("the inputs lock").push(input);
            self.applied.notify_one();
            Ok(None)
        }

        async fn list_apps(&self, _query: Option<String>) -> adesk_viewer::Result<Vec<AppEntry>> {
            self.listing.notify_one();
            let permit = self
                .release
                .acquire()
                .await
                .expect("the release semaphore is never closed");
            permit.forget();
            Ok(Vec::new())
        }
    }

    /// §5: a seat input queued while a request/response is in flight is applied
    /// without waiting for that reply.
    ///
    /// `list_apps` parks inside the backend, so a worker that awaited the listing's
    /// reply on the input path would leave the move behind it unread — and
    /// unapplied — until the listing finished. The move must be applied first,
    /// while the listing is still parked.
    #[tokio::test]
    async fn seat_input_is_applied_while_a_request_is_in_flight() {
        /// Generous bound for any step that depends on the peer making progress.
        const STEP: Duration = Duration::from_secs(5);

        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("viewer.sock");
        let listener = UnixListener::bind(&path).expect("bind the unix socket");

        let backend = Arc::new(GatedBackend::new());
        let serving = Arc::clone(&backend);
        let serve = tokio::spawn(async move {
            let (stream, _addr) = listener.accept().await.expect("accept the viewer");
            ViewerServer::new(serving)
                .serve(stream, PeerInfo::Other("gated".to_owned()))
                .await
        });

        let (input, mut commands) = unbounded_channel();
        let (events, mut ui) = unbounded_channel();
        let target = ViewerTarget::Unix(path.clone());
        let driver = tokio::spawn(async move {
            session(target, &mut commands, &events).await;
        });

        // The worker is up once it reports the handshake.
        loop {
            match tokio::time::timeout(STEP, ui.recv())
                .await
                .expect("the worker must connect")
            {
                Some(UiEvent::Connected { .. }) => break,
                Some(_) => continue,
                None => panic!("the worker ended before it connected"),
            }
        }

        // The launcher's registry fetch parks inside the backend.
        assert!(input.send(InputCommand::ListApps).is_ok());
        tokio::time::timeout(STEP, backend.listing())
            .await
            .expect("the listing must start");

        // A seat input queued behind the parked request is applied at once,
        // before any reply: the listing is still parked when it lands.
        assert!(input.send(InputCommand::Move { x: 0.25, y: 0.75 }).is_ok());
        tokio::time::timeout(STEP, backend.applied())
            .await
            .expect("the seat input must be applied while the listing is still parked");
        assert_eq!(
            backend.recorded_inputs(),
            vec![ViewerInput::PointerMove { x: 0.25, y: 0.75 }]
        );

        // Release the listing and end the session cleanly.
        backend.release_listing();
        drop(input);
        tokio::time::timeout(STEP, driver)
            .await
            .expect("the worker must end when the input handle drops")
            .expect("the worker task must not panic");
        tokio::time::timeout(STEP, serve)
            .await
            .expect("the server must end")
            .expect("the server task must not panic")
            .expect("the session must close cleanly");
    }
}
