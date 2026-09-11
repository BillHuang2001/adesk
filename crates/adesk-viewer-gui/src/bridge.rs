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

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use adesk_core::{Button, ButtonState, WindowId};
use adesk_proto::KeySpec;
use adesk_viewer::{ViewerClient, ViewerTarget};
use adesk_viewer_proto::{ControlOwner, DesktopState, KeyAction, ServerHello, ViewerFrame};

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
        /// The window the frame targets, when one is active.
        active_window_id: Option<WindowId>,
    },
    /// The connection ended; carries a human-readable reason.
    Disconnected(String),
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

        let spawned = std::thread::Builder::new()
            .name("adesk-viewer-gui-bridge".to_owned())
            .spawn(move || worker(target, input_rx, worker_events));

        if let Err(error) = spawned {
            // The thread never started, so report a terminal disconnect and drop
            // the (unused) input receiver with the closure.
            let _ = event_tx.send(UiEvent::Disconnected(format!(
                "could not start the viewer bridge thread: {error}"
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
                "could not start the viewer async runtime: {error}"
            )));
            return;
        }
    };

    let mut input = input;
    runtime.block_on(session(target, &mut input, &events));
}

/// Connects, then runs the frame/input/refresh loop until the connection or the
/// GTK side goes away. Best-effort throughout: no failure is fatal.
async fn session(
    target: ViewerTarget,
    input: &mut UnboundedReceiver<InputCommand>,
    events: &UnboundedSender<UiEvent>,
) {
    let client = match ViewerClient::connect(target.clone()).await {
        Ok(client) => client,
        Err(error) => {
            let _ = events.send(UiEvent::Disconnected(error.to_string()));
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

    // An initial state + frame so the task bar and view fill in immediately.
    refresh_state(&client, events).await;
    let mut last_active = match client.request_frame().await {
        Ok(frame) => {
            emit_frame(&frame, events);
            frame.active_window_id
        }
        Err(error) => {
            tracing::debug!(%error, "initial viewer frame request failed");
            None
        }
    };

    let mut frames = std::pin::pin!(client.frames());

    // Refresh the task bar on a modest timer as well as on an active-window
    // change: state is requested inline, so at most one `request_state` is ever
    // in flight (inherent coalescing) and create/destroy/title changes converge.
    let mut ticker = tokio::time::interval(STATE_REFRESH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the interval's immediately-ready first tick (the initial state
    // request above already ran).
    ticker.tick().await;

    loop {
        tokio::select! {
            command = input.recv() => match command {
                Some(command) => apply_input(&client, command, events).await,
                None => break,
            },
            frame = frames.next() => match frame {
                Some(Ok(frame)) => {
                    emit_frame(&frame, events);
                    if frame.active_window_id != last_active {
                        last_active = frame.active_window_id;
                        refresh_state(&client, events).await;
                    }
                }
                Some(Err(error)) => {
                    tracing::debug!(%error, "viewer frame stream error");
                    let _ = events.send(UiEvent::Notice(format!("frame stream error: {error}")));
                }
                None => {
                    let _ = events.send(UiEvent::Disconnected(
                        "the viewer connection closed".to_owned(),
                    ));
                    break;
                }
            },
            _ = ticker.tick() => refresh_state(&client, events).await,
        }
    }

    if let Err(error) = client.close().await {
        tracing::debug!(%error, "viewer close failed");
    }
}

/// Decodes `frame` and forwards it, turning a decode failure into a notice.
fn emit_frame(frame: &ViewerFrame, events: &UnboundedSender<UiEvent>) {
    match decode(&frame.image) {
        Ok(image) => {
            let _ = events.send(UiEvent::Frame {
                seq: frame.seq,
                ts_ms: frame.ts_ms,
                image,
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

/// Applies one input command, logging and noticing (never failing) on error.
async fn apply_input(
    client: &ViewerClient,
    command: InputCommand,
    events: &UnboundedSender<UiEvent>,
) {
    let result = match command {
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

    use adesk_proto::ImagePayload;
    use adesk_viewer_proto::CursorState;

    /// A frame fixture carrying `image`.
    fn frame(image: ImagePayload) -> ViewerFrame {
        ViewerFrame {
            seq: 7,
            ts_ms: 123,
            image,
            cursor: CursorState::hidden(),
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
                active_window_id,
            } => {
                assert_eq!(seq, 7);
                assert_eq!(ts_ms, 123);
                assert_eq!(image.width, 1);
                assert_eq!(image.height, 1);
                assert_eq!(image.rgba8, vec![1, 2, 3, 255]);
                assert_eq!(active_window_id, Some(WindowId(3)));
            }
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
}
