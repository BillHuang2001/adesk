//! Async VAP client SDK: `ViewerClient`, `ViewerTarget` and `ConnectOptions`.
//!
//! The client is the viewer side of the Viewer Attachment Protocol
//! (`docs/viewer.md`). It connects over a Unix or TCP transport, performs the
//! §2 handshake, and then runs a background **dispatcher** that owns the read
//! half of the connection: it fans streamed `frame`s and `input_ack`s out to
//! broadcast channels, resolves in-flight `request_state`s and surfaces server
//! `error`s. The write half stays behind a mutex in the shared client state, so
//! the frame stream, the input methods and the `request_*` calls can all be used
//! concurrently without deadlock.
//!
//! Everything the client exposes is a thin wrapper over
//! [`adesk_viewer_proto`] messages; the crate speaks only that wire vocabulary
//! (`docs/viewer.md` §8). Message bodies and pixel payloads are never logged.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use adesk_core::{ActionId, AppId, Button, ButtonState, ErrorCode, OverlayKind, WindowId};
use adesk_proto::KeySpec;
use adesk_viewer_proto::{
    check_version, decode_server, encode_client, AppEntry, ClientMessage, ControlOwner,
    DesktopState, KeyAction, LaunchOutcome, RecordingStatus, ServerHello, ServerMessage,
    ViewerFrame, ViewerHello, DEFAULT_MIN_INTERVAL_MS, DEFAULT_OVERLAYS, PROTOCOL_VERSION,
};
use futures::stream::{self, Stream};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::backend::RecordRequest;
use crate::error::{Result, ViewerError};
use crate::reply::ReplyFifo;
use crate::transport::{read_line, read_line_into, write_line};

pub use crate::transport::DEFAULT_MAX_FRAME_LEN;

/// Default timeout for establishing the transport connection (`docs/viewer.md` §1).
///
/// A connect that takes longer than this fails with [`ViewerError::Io`]
/// (`ErrorKind::TimedOut`).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default timeout for the handshake exchange (`docs/viewer.md` §2).
///
/// If the server's `hello` does not arrive within this window the connection is
/// refused with [`ViewerError::Handshake`].
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Grace period [`ViewerClient::close`] allows the dispatcher to observe the
/// server's `bye` reply before the reader task is aborted.
///
/// A clean close writes `bye` and shuts its write half down; the session answers
/// with its own `bye` and finishes. Waiting this long lets the dispatcher read
/// that reply (or the resulting EOF) and end on its own, so the read half stays
/// open until the acknowledgement is delivered — which is what makes a clean
/// close a deterministic success for `ViewerServer::serve`. A close that outlives
/// this window is still a success: the reader is aborted instead.
const CLOSE_GRACE: Duration = Duration::from_millis(250);

/// Capacity of the broadcast channel that fans streamed frames out to `frames()`
/// receivers and in-flight `request_frame` calls.
///
/// A receiver that falls further behind than this skips frames (a viewer may
/// drop frames under load); the cap bounds how many decoded frames are retained.
const FRAME_CHANNEL_CAPACITY: usize = 32;

/// Capacity of the broadcast channel that fans `input_ack` messages out to
/// [`ViewerClient::input_ack`] receivers.
const INPUT_ACK_CHANNEL_CAPACITY: usize = 64;

/// Capacity of the broadcast channel that fans server `error` messages out to
/// pending `request_*` calls.
const ERROR_CHANNEL_CAPACITY: usize = 16;

/// One `input_ack` (`docs/viewer.md` §3): the acknowledged input's client `id`
/// (when it carried one) and the AGP [`ActionId`] the runtime recorded.
type InputAck = (Option<u64>, ActionId);

/// Where a [`ViewerClient`] connects (`docs/viewer.md` §1).
///
/// The Unix dialect is the local default; TCP is the remote, opt-in transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerTarget {
    /// A Unix domain socket at the given path.
    Unix(PathBuf),
    /// A TCP endpoint at the given address.
    Tcp(SocketAddr),
}

impl std::fmt::Display for ViewerTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewerTarget::Unix(path) => write!(f, "unix:{}", path.display()),
            ViewerTarget::Tcp(address) => write!(f, "tcp:{address}"),
        }
    }
}

/// Connection and handshake options for [`ViewerClient::connect_with`].
///
/// Built from a [`ViewerTarget`] with [`ConnectOptions::new`] (the crate
/// defaults) and refined with the consuming `with_*` builders.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Where to connect.
    pub target: ViewerTarget,
    /// Maximum length of one inbound NDJSON line, in bytes
    /// ([`DEFAULT_MAX_FRAME_LEN`] by default).
    pub max_frame_len: usize,
    /// Maximum time to establish the transport connection.
    pub connect_timeout: Duration,
    /// Maximum time for the handshake exchange.
    pub handshake_timeout: Duration,
    /// Optional viewer name sent in the `hello`, for server-side logs.
    pub client_name: Option<String>,
    /// Debug overlay set the server composites into every streamed frame.
    pub overlays: Vec<OverlayKind>,
    /// Minimum spacing between streamed frames, in milliseconds (`0` = unpaced).
    pub min_interval_ms: u64,
    /// Whether to reject a server whose `protocol_version` differs from
    /// [`PROTOCOL_VERSION`].
    pub verify_version: bool,
}

impl ConnectOptions {
    /// Builds the crate-default options for `target`.
    ///
    /// Defaults: [`DEFAULT_MAX_FRAME_LEN`], [`DEFAULT_CONNECT_TIMEOUT`],
    /// [`DEFAULT_HANDSHAKE_TIMEOUT`], no client name, [`DEFAULT_OVERLAYS`],
    /// [`DEFAULT_MIN_INTERVAL_MS`], version verification on.
    pub fn new(target: ViewerTarget) -> ConnectOptions {
        ConnectOptions {
            target,
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            client_name: None,
            overlays: DEFAULT_OVERLAYS.to_vec(),
            min_interval_ms: DEFAULT_MIN_INTERVAL_MS,
            verify_version: true,
        }
    }

    /// Replaces the [`ViewerTarget`].
    pub fn with_target(mut self, target: ViewerTarget) -> ConnectOptions {
        self.target = target;
        self
    }

    /// Replaces the inbound line cap.
    pub fn with_max_frame_len(mut self, max_frame_len: usize) -> ConnectOptions {
        self.max_frame_len = max_frame_len;
        self
    }

    /// Replaces the connect timeout.
    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> ConnectOptions {
        self.connect_timeout = connect_timeout;
        self
    }

    /// Replaces the handshake timeout.
    pub fn with_handshake_timeout(mut self, handshake_timeout: Duration) -> ConnectOptions {
        self.handshake_timeout = handshake_timeout;
        self
    }

    /// Sets the viewer name sent in the `hello`.
    pub fn with_client_name(mut self, client_name: impl Into<String>) -> ConnectOptions {
        self.client_name = Some(client_name.into());
        self
    }

    /// Replaces the debug overlay set.
    pub fn with_overlays(mut self, overlays: Vec<OverlayKind>) -> ConnectOptions {
        self.overlays = overlays;
        self
    }

    /// Replaces the frame pacing interval, in milliseconds.
    pub fn with_min_interval_ms(mut self, min_interval_ms: u64) -> ConnectOptions {
        self.min_interval_ms = min_interval_ms;
        self
    }

    /// Enables or disables [`PROTOCOL_VERSION`] verification.
    pub fn with_verify_version(mut self, verify_version: bool) -> ConnectOptions {
        self.verify_version = verify_version;
        self
    }
}

/// A live connection to an ADesk runtime's VAP endpoint.
///
/// Created by [`ViewerClient::connect`] / [`ViewerClient::connect_with`]; after
/// the handshake the connection is driven by a background dispatcher task, so
/// [`frames`](ViewerClient::frames), [`input_ack`](ViewerClient::input_ack), the
/// input methods and the `request_*` calls may all be used concurrently.
///
/// Dropping the client without [`close`](ViewerClient::close) still stops the
/// dispatcher and wakes every stream.
pub struct ViewerClient {
    inner: Arc<Inner>,
    dispatcher: JoinHandle<()>,
}

impl ViewerClient {
    /// Connects to `target` with the crate-default [`ConnectOptions`].
    ///
    /// # Errors
    ///
    /// See [`ViewerClient::connect_with`].
    pub async fn connect(target: ViewerTarget) -> Result<ViewerClient> {
        ViewerClient::connect_with(ConnectOptions::new(target)).await
    }

    /// Connects to [`ConnectOptions::target`] and performs the §2 handshake.
    ///
    /// Opens a `tokio` Unix or TCP stream under [`ConnectOptions::connect_timeout`],
    /// sends a [`ViewerHello`], and waits under
    /// [`ConnectOptions::handshake_timeout`] for the server's [`ServerHello`]. On
    /// success a background dispatcher task takes over the read half.
    ///
    /// # Errors
    ///
    /// - [`ViewerError::Io`] when the transport cannot be opened (a connect
    ///   timeout is reported as `ErrorKind::TimedOut`);
    /// - [`ViewerError::Handshake`] when the handshake times out, the server
    ///   answers with an `error` message, or the first reply is not a `hello`;
    /// - [`ViewerError::Closed`] when the stream ends before the `hello`;
    /// - [`ViewerError::VersionMismatch`] when
    ///   [`ConnectOptions::verify_version`] is set and the server reports a
    ///   different [`PROTOCOL_VERSION`];
    /// - [`ViewerError::Protocol`] when the reply is not a well-formed VAP
    ///   message.
    pub async fn connect_with(options: ConnectOptions) -> Result<ViewerClient> {
        let transport = open_transport(&options).await?;
        let (read_half, mut write_half) = tokio::io::split(transport);
        let mut reader = BufReader::new(read_half);

        let hello = ClientMessage::Hello(ViewerHello {
            protocol_version: PROTOCOL_VERSION,
            client: options.client_name.clone(),
            overlays: options.overlays.clone(),
            min_interval_ms: options.min_interval_ms,
        });
        write_line(&mut write_half, &encode_client(&hello)).await?;

        let reply = timeout(
            options.handshake_timeout,
            read_line(&mut reader, options.max_frame_len),
        )
        .await
        .map_err(|_| {
            ViewerError::Handshake("the server hello did not arrive in time".to_owned())
        })??;
        let reply_line = reply.ok_or(ViewerError::Closed)?;
        let server_hello = match decode_server(&reply_line)? {
            ServerMessage::Hello(hello) => hello,
            ServerMessage::Error { message, .. } => return Err(ViewerError::Handshake(message)),
            other => {
                return Err(ViewerError::Handshake(format!(
                    "expected a server hello, received `{}`",
                    other.message_type()
                )))
            }
        };
        if options.verify_version {
            check_version(server_hello.protocol_version).map_err(|_| {
                ViewerError::VersionMismatch {
                    client: server_hello.protocol_version,
                    server: PROTOCOL_VERSION,
                }
            })?;
        }

        let inner = Arc::new(Inner {
            target: options.target,
            hello: server_hello,
            max_frame_len: options.max_frame_len,
            writer: tokio::sync::Mutex::new(write_half),
            frames: Mutex::new(Some(broadcast::channel(FRAME_CHANNEL_CAPACITY).0)),
            input_acks: Mutex::new(Some(broadcast::channel(INPUT_ACK_CHANNEL_CAPACITY).0)),
            errors: Mutex::new(Some(broadcast::channel(ERROR_CHANNEL_CAPACITY).0)),
            state_replies: ReplyFifo::new(),
            recording_replies: ReplyFifo::new(),
            apps_replies: ReplyFifo::new(),
            launch_replies: ReplyFifo::new(),
            next_recording_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let dispatcher = tokio::spawn(dispatch(Arc::clone(&inner), reader));

        Ok(ViewerClient { inner, dispatcher })
    }

    /// The server's handshake acknowledgement (`docs/viewer.md` §2).
    pub fn hello(&self) -> &ServerHello {
        &self.inner.hello
    }

    /// The target this client is connected to.
    pub fn target(&self) -> &ViewerTarget {
        &self.inner.target
    }

    /// The Unix socket path, when the target is a Unix socket (else `None`).
    pub fn socket_path(&self) -> Option<&Path> {
        match &self.inner.target {
            ViewerTarget::Unix(path) => Some(path.as_path()),
            ViewerTarget::Tcp(_) => None,
        }
    }

    /// A stream of desktop frames pushed by the server (`docs/viewer.md` §3, §5).
    ///
    /// The stream ends when the connection closes (or the server sends `bye`).
    /// It does **not** require [`request_frame`](ViewerClient::request_frame):
    /// the server pushes frames as the desktop changes, and the stream may be
    /// consumed while input methods are called concurrently.
    ///
    /// The stream owns its own channel handle, so it does not borrow `self`;
    /// the broadcast channel retains at most a small fixed number of frames and
    /// a receiver that falls further behind skips frames.
    pub fn frames(&self) -> impl Stream<Item = Result<ViewerFrame>> {
        let receiver = lock(&self.inner.frames)
            .as_ref()
            .map(broadcast::Sender::subscribe);
        stream::unfold(receiver, |receiver| async move {
            let mut receiver = receiver?;
            loop {
                match receiver.recv().await {
                    Ok(frame) => return Some((Ok(frame), Some(receiver))),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "viewer client frame stream lagged behind");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }

    /// Requests and awaits one freshly rendered frame (`docs/viewer.md` §4).
    ///
    /// Subscribes to the frame channel *before* writing `request_frame`, so the
    /// answering frame cannot be missed, then returns the next frame the server
    /// pushes.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Closed`] if the connection ends before a frame
    /// arrives, [`ViewerError::Backend`] if the server answers with an `error`,
    /// and [`ViewerError::Io`] if the request cannot be written.
    pub async fn request_frame(&self) -> Result<ViewerFrame> {
        let mut frames = self.subscribe_frames()?;
        let mut errors = self.subscribe_errors()?;
        self.send(ClientMessage::RequestFrame { id: None }).await?;
        loop {
            tokio::select! {
                received = frames.recv() => match received {
                    Ok(frame) => return Ok(frame),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Err(ViewerError::Closed),
                },
                error = errors.recv() => match error {
                    Ok((code, message)) => return Err(ViewerError::Backend { code, message }),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Err(ViewerError::Closed),
                },
            }
        }
    }

    /// Requests and awaits the current desktop state (`docs/viewer.md` §4).
    ///
    /// Registers a waiter *before* writing `request_state`; the state message
    /// carries no id, so the dispatcher resolves the oldest pending request.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Closed`] if the connection ends before the state
    /// arrives, [`ViewerError::Backend`] if the server answers with an `error`,
    /// and [`ViewerError::Io`] if the request cannot be written.
    pub async fn request_state(&self) -> Result<DesktopState> {
        self.reply_round_trip(
            &self.inner.state_replies,
            ClientMessage::RequestState { id: None },
        )
        .await
    }

    /// Starts a screen recording and returns its initial status
    /// (`docs/viewer.md` §4, §5).
    ///
    /// The runtime owns the encoder and the file: it renders the full output at
    /// [`RecordRequest::fps`] while the recording runs. The returned status
    /// reports the resolved path and the encoder actually in use, so a request
    /// with no path still learns where the recording landed. A waiter is
    /// registered *before* the request is written, so the reply cannot be missed.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Closed`] if the connection ends before the status
    /// arrives, [`ViewerError::Backend`] if the server answers with an `error`
    /// (`not_supported` for an unavailable encoder, `invalid_request` for a
    /// conflicting transition), and [`ViewerError::Io`] if the request cannot be
    /// written.
    pub async fn start_recording(&self, request: RecordRequest) -> Result<RecordingStatus> {
        let message = ClientMessage::StartRecording {
            id: Some(self.inner.next_recording_id()),
            path: request.path.map(|path| path.to_string_lossy().into_owned()),
            fps: request.fps,
            encoder: request.encoder,
        };
        self.recording_round_trip(message).await
    }

    /// Stops the active screen recording and returns the finished file's status
    /// (`docs/viewer.md` §4, §5).
    ///
    /// The reported status describes the finished recording (`recording` is
    /// `false`, `frames`/`duration_ms` are its totals).
    ///
    /// # Errors
    ///
    /// As [`ViewerClient::start_recording`].
    pub async fn stop_recording(&self) -> Result<RecordingStatus> {
        let message = ClientMessage::StopRecording {
            id: Some(self.inner.next_recording_id()),
        };
        self.recording_round_trip(message).await
    }

    /// Requests the current recording status without changing it
    /// (`docs/viewer.md` §4, §5).
    ///
    /// # Errors
    ///
    /// As [`ViewerClient::start_recording`].
    pub async fn request_recording(&self) -> Result<RecordingStatus> {
        let message = ClientMessage::RequestRecording {
            id: Some(self.inner.next_recording_id()),
        };
        self.recording_round_trip(message).await
    }

    /// Sends one recording request and awaits its `recording` reply (or a server
    /// `error`), resolving the reply through the recording FIFO.
    ///
    /// Recording is reply-only, so this mirrors
    /// [`ViewerClient::request_state`]: a waiter is registered before the request
    /// is written and the oldest pending waiter is resolved. `recording` and
    /// `state` replies have their own FIFOs, so one can never resolve the other.
    async fn recording_round_trip(&self, message: ClientMessage) -> Result<RecordingStatus> {
        self.reply_round_trip(&self.inner.recording_replies, message)
            .await
    }

    /// Moves the pointer to the normalized output position `(x, y)`
    /// (`docs/viewer.md` §4).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn pointer_move(&self, x: f64, y: f64) -> Result<()> {
        self.send(ClientMessage::PointerMove { x, y }).await
    }
    /// Presses or releases `button`, optionally moving to `pos` first
    /// (`docs/viewer.md` §4).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn pointer_button(
        &self,
        button: Button,
        state: ButtonState,
        pos: Option<(f64, f64)>,
    ) -> Result<()> {
        let (x, y) = position(pos);
        self.send(ClientMessage::PointerButton {
            button,
            state,
            x,
            y,
        })
        .await
    }

    /// Scrolls by `(dx, dy)` at an optional normalized `pos` (`docs/viewer.md` §4).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn scroll(&self, dx: f64, dy: f64, pos: Option<(f64, f64)>) -> Result<()> {
        let (x, y) = position(pos);
        self.send(ClientMessage::Scroll { dx, dy, x, y }).await
    }

    /// Presses, releases or taps `keys` (`docs/viewer.md` §4).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn key(&self, keys: KeySpec, action: KeyAction) -> Result<()> {
        self.send(ClientMessage::Key { keys, action }).await
    }

    /// Types UTF-8 `text` (`docs/viewer.md` §4).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn text(&self, text: impl Into<String>) -> Result<()> {
        self.send(ClientMessage::Text { text: text.into() }).await
    }

    /// Activates `window_id` (`docs/viewer.md` §4, §5).
    ///
    /// Window switching is runtime-native, not synthesized input: the server
    /// changes compositor window state directly (the same path AGP §5.3 uses).
    /// Like the other fire-and-forget input methods this only writes the message;
    /// the server's acknowledgement — an `input_ack` carrying the recorded AGP
    /// [`ActionId`] — arrives on [`ViewerClient::input_ack`].
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn activate_window(&self, window_id: WindowId) -> Result<()> {
        self.send(ClientMessage::ActivateWindow { window_id }).await
    }

    /// Closes `window_id` (`docs/viewer.md` §4, §5).
    ///
    /// Window management is runtime-native, like
    /// [`activate_window`](ViewerClient::activate_window): the server closes the
    /// window directly instead of synthesizing input. Fire-and-forget — the
    /// server's acknowledgement (an `input_ack` carrying the recorded AGP
    /// [`ActionId`], when the runtime records one) arrives on
    /// [`ViewerClient::input_ack`].
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn close_window(&self, window_id: WindowId) -> Result<()> {
        self.send(ClientMessage::CloseWindow { window_id }).await
    }

    /// Lists the applications the runtime can launch, optionally narrowed by
    /// `query` (`docs/viewer.md` §4, §5).
    ///
    /// The reply FIFO registration is made *before* the request is written, so the
    /// `apps` reply cannot be missed; a request answered by a VAP `error`
    /// (`not_supported` from a runtime without an app registry) still leaves the
    /// FIFO aligned for the next request.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Closed`] if the connection ends before the reply
    /// arrives, [`ViewerError::Backend`] if the server answers with an `error`,
    /// and [`ViewerError::Io`] if the request cannot be written.
    pub async fn list_apps(&self, query: Option<String>) -> Result<Vec<AppEntry>> {
        self.reply_round_trip(
            &self.inner.apps_replies,
            ClientMessage::ListApps { id: None, query },
        )
        .await
    }

    /// Launches the application `app_id` and returns the runtime's launch outcome
    /// (`docs/viewer.md` §4, §5).
    ///
    /// The reply (`launch_result`) carries the launch id and the window the
    /// runtime correlated with it, when it already has one. As
    /// [`list_apps`](ViewerClient::list_apps), the registration is made before the
    /// request is written.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Closed`] if the connection ends before the reply
    /// arrives, [`ViewerError::Backend`] if the server answers with an `error`
    /// (e.g. `unknown_app` for an id the runtime does not know), and
    /// [`ViewerError::Io`] if the request cannot be written.
    pub async fn launch_app(&self, app_id: AppId) -> Result<LaunchOutcome> {
        self.reply_round_trip(
            &self.inner.launch_replies,
            ClientMessage::LaunchApp { id: None, app_id },
        )
        .await
    }

    /// Announces that `owner` owns input (`docs/viewer.md` §4, §5).
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] if the message cannot be written.
    pub async fn set_control(&self, owner: ControlOwner) -> Result<()> {
        self.send(ClientMessage::SetControl { owner }).await
    }

    /// The `input_ack` messages the server sends (`docs/viewer.md` §3).
    ///
    /// Each item is the acknowledged input's client `id` (when it carried one)
    /// and the AGP [`ActionId`] the runtime recorded. The stream ends when the
    /// connection closes, and owns its own channel handle (it does not borrow
    /// `self`).
    pub fn input_ack(&self) -> impl Stream<Item = (Option<u64>, ActionId)> {
        let receiver = lock(&self.inner.input_acks)
            .as_ref()
            .map(broadcast::Sender::subscribe);
        stream::unfold(receiver, |receiver| async move {
            let mut receiver = receiver?;
            loop {
                match receiver.recv().await {
                    Ok(ack) => return Some((ack, Some(receiver))),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "viewer client input-ack stream lagged behind");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }

    /// Closes the connection: sends a best-effort `bye`, shuts the write half
    /// down, waits briefly for the server's reply, then stops the dispatcher.
    ///
    /// The close is best-effort and always succeeds: a failed `bye` write and a
    /// dispatcher that does not finish within its grace period are both logged,
    /// not returned. After writing `bye` the client shuts its write half down and
    /// gives the dispatcher a bounded grace period to read the server's `bye`
    /// reply (or the resulting EOF) and end on its own; only if that window
    /// elapses is the reader task aborted. Ending the reader immediately would
    /// drop the read half before the server's courtesy acknowledgement could be
    /// written, surfacing a spurious peer-gone error from an otherwise clean
    /// close.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Transport`] only if the dispatcher task itself
    /// failed.
    pub async fn close(mut self) -> Result<()> {
        if let Err(error) = write_message(&self.inner, &ClientMessage::Bye { reason: None }).await {
            tracing::debug!(%error, "could not send the viewer bye message");
        }
        {
            let mut writer = self.inner.writer.lock().await;
            let _ = writer.shutdown().await;
        }
        // Wake every stream and pending request even if the peer never closes its
        // own read side. This does not touch the dispatcher's reader, so it can
        // still observe the server's `bye` reply below.
        self.inner.shutdown();
        let stopped = timeout(CLOSE_GRACE, &mut self.dispatcher).await;
        match stopped {
            // The dispatcher ended on its own (the server's `bye` or EOF).
            Ok(Ok(())) => {}
            // Cancelled elsewhere: there is nothing left to stop.
            Ok(Err(error)) if error.is_cancelled() => {}
            // The dispatcher task itself failed: surface it.
            Ok(Err(error)) => {
                return Err(ViewerError::Transport(format!(
                    "viewer connection task failed: {error}"
                )));
            }
            // The grace period elapsed: stop the reader task outright. A
            // best-effort close is still a success.
            Err(_elapsed) => {
                tracing::debug!("viewer dispatcher did not stop within the close grace period");
                self.dispatcher.abort();
            }
        }
        Ok(())
    }

    /// Writes one client message as a single NDJSON line.
    async fn send(&self, message: ClientMessage) -> Result<()> {
        write_message(&self.inner, &message).await
    }

    /// Sends one reply-only request and awaits its reply (or a server `error`),
    /// resolving it through `replies`.
    ///
    /// The reply-FIFO registration is made *before* the request is written and is
    /// dropped again if the write fails or the request is answered by a VAP
    /// `error`, so a registration can never outlive its request and swallow the
    /// next reply.
    async fn reply_round_trip<T>(
        &self,
        replies: &ReplyFifo<T>,
        message: ClientMessage,
    ) -> Result<T> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(ViewerError::Closed);
        }
        let mut errors = self.subscribe_errors()?;
        let pending = replies.register();
        if let Err(error) = self.send(message).await {
            // The request was never written: drop the registration so it cannot
            // swallow the next reply.
            replies.remove(pending.token);
            return Err(error);
        }
        replies.await_reply(pending, &mut errors).await
    }

    /// Subscribes to the frame channel, or fails once the connection is gone.
    fn subscribe_frames(&self) -> Result<broadcast::Receiver<ViewerFrame>> {
        lock(&self.inner.frames)
            .as_ref()
            .map(broadcast::Sender::subscribe)
            .ok_or(ViewerError::Closed)
    }

    /// Subscribes to the error channel, or fails once the connection is gone.
    ///
    /// Each item is the AGP `ErrorCode` the server reported and its message.
    fn subscribe_errors(&self) -> Result<broadcast::Receiver<(ErrorCode, String)>> {
        lock(&self.inner.errors)
            .as_ref()
            .map(broadcast::Sender::subscribe)
            .ok_or(ViewerError::Closed)
    }
}

impl Drop for ViewerClient {
    fn drop(&mut self) {
        // A client dropped without `close` must still leave no reader task and no
        // live broadcast channel behind.
        self.dispatcher.abort();
        self.inner.shutdown();
    }
}

/// A type-erased, bidirectional VAP transport.
///
/// The two [`ViewerTarget`] dialects produce different concrete stream types;
/// boxing them behind this private trait gives the client a single code path for
/// the handshake and the dispatcher.
trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> Transport for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

/// Shared client state: the write half plus every fan-out channel and waiter the
/// dispatcher serves.
struct Inner {
    /// The target this client is connected to.
    target: ViewerTarget,
    /// The server's handshake acknowledgement.
    hello: ServerHello,
    /// Inbound line cap for the dispatcher.
    max_frame_len: usize,
    /// The connection's write half, serialising concurrent writers.
    writer: tokio::sync::Mutex<WriteHalf<Box<dyn Transport>>>,
    /// Drop-sender fan-out of streamed frames; `None` once shut down.
    frames: Mutex<Option<broadcast::Sender<ViewerFrame>>>,
    /// Drop-sender fan-out of `input_ack` messages; `None` once shut down.
    input_acks: Mutex<Option<broadcast::Sender<InputAck>>>,
    /// Drop-sender fan-out of server error messages; `None` once shut down.
    errors: Mutex<Option<broadcast::Sender<(ErrorCode, String)>>>,
    /// Pending `request_state` replies, resolved oldest-first.
    state_replies: ReplyFifo<DesktopState>,
    /// Pending recording replies, resolved oldest-first. Recording is reply-only
    /// (there is no unsolicited `recording` push in v1), so it gets its own FIFO
    /// — a `recording` message can never be mistaken for a `state` reply.
    recording_replies: ReplyFifo<RecordingStatus>,
    /// Pending `list_apps` replies, resolved oldest-first.
    apps_replies: ReplyFifo<Vec<AppEntry>>,
    /// Pending `launch_app` replies, resolved oldest-first.
    launch_replies: ReplyFifo<LaunchOutcome>,
    /// Monotonic client id stamped on each recording request, echoed by the reply.
    next_recording_id: AtomicU64,
    /// Set once the connection is gone, for a cheap `request_state` early-out.
    closed: AtomicBool,
}

impl Inner {
    /// Marks the connection closed and drops every channel sender and pending
    /// waiter, so no stream or request can await forever after the peer is gone.
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.frames).take();
        lock(&self.input_acks).take();
        lock(&self.errors).take();
        self.state_replies.clear();
        self.recording_replies.clear();
        self.apps_replies.clear();
        self.launch_replies.clear();
    }

    /// The next recording request id (starting at `1`).
    fn next_recording_id(&self) -> u64 {
        self.next_recording_id.fetch_add(1, Ordering::SeqCst)
    }
}

/// Opens the transport for `options.target`, honouring `connect_timeout`.
async fn open_transport(options: &ConnectOptions) -> Result<Box<dyn Transport>> {
    match &options.target {
        ViewerTarget::Unix(path) => {
            let stream = timeout(options.connect_timeout, UnixStream::connect(path))
                .await
                .map_err(|_| connect_timeout(&options.target))??;
            Ok(Box::new(stream))
        }
        ViewerTarget::Tcp(address) => {
            let stream = timeout(options.connect_timeout, TcpStream::connect(address))
                .await
                .map_err(|_| connect_timeout(&options.target))??;
            Ok(Box::new(stream))
        }
    }
}

/// The error a connect timeout yields: an [`ViewerError::Io`] of kind
/// `TimedOut`.
fn connect_timeout(target: &ViewerTarget) -> ViewerError {
    ViewerError::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("connecting to {target} timed out"),
    ))
}

/// Writes one client message as an NDJSON line under the shared write lock.
async fn write_message(inner: &Inner, message: &ClientMessage) -> Result<()> {
    let line = encode_client(message);
    let mut writer = inner.writer.lock().await;
    write_line(&mut *writer, &line).await
}

/// The background reader: decodes server messages until the connection ends and
/// fans them out, then [`Inner::shutdown`]s.
async fn dispatch(inner: Arc<Inner>, mut reader: BufReader<ReadHalf<Box<dyn Transport>>>) {
    // One inbound line buffer reused across the connection's lifetime.
    let mut line_buf: Vec<u8> = Vec::new();
    loop {
        match read_line_into(&mut reader, &mut line_buf, inner.max_frame_len).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!("viewer connection reached end of stream");
                break;
            }
            Err(error) => {
                tracing::debug!(%error, "viewer connection read failed");
                break;
            }
        }
        let line = match std::str::from_utf8(&line_buf) {
            Ok(line) => line,
            // Unreachable: `read_line_into` already rejected invalid UTF-8; a
            // malformed line is dropped and the connection stays open.
            Err(error) => {
                tracing::debug!(%error, "ignoring a malformed viewer message");
                continue;
            }
        };
        let message = match decode_server(line) {
            Ok(message) => message,
            Err(error) => {
                // A malformed line is dropped; the connection stays open, like
                // the forward-compatible `Unknown` handling (§1).
                tracing::debug!(%error, "ignoring a malformed viewer message");
                continue;
            }
        };
        match message {
            ServerMessage::Frame(frame) => {
                if let Some(sender) = lock(&inner.frames).as_ref() {
                    let _ = sender.send(frame);
                }
            }
            ServerMessage::InputAck { id, action_id } => {
                if let Some(sender) = lock(&inner.input_acks).as_ref() {
                    let _ = sender.send((id, action_id));
                }
            }
            ServerMessage::State(state) => {
                if !inner.state_replies.resolve(state) {
                    tracing::trace!("ignoring an unsolicited desktop state message");
                }
            }
            ServerMessage::Recording { id, status } => {
                if !inner.recording_replies.resolve(status) {
                    tracing::trace!(?id, "ignoring an unsolicited recording status message");
                }
            }
            ServerMessage::Apps { id, apps } => {
                if !inner.apps_replies.resolve(apps) {
                    tracing::trace!(?id, "ignoring an unsolicited apps reply");
                }
            }
            ServerMessage::LaunchResult { id, result } => {
                if !inner.launch_replies.resolve(result) {
                    tracing::trace!(?id, "ignoring an unsolicited launch result");
                }
            }
            ServerMessage::Error { code, message, .. } => {
                if let Some(sender) = lock(&inner.errors).as_ref() {
                    let _ = sender.send((code, message));
                }
            }
            ServerMessage::Control { owner } => {
                tracing::trace!(?owner, "viewer control owner updated");
            }
            ServerMessage::Bye { .. } => {
                tracing::debug!("viewer connection closed by the server");
                break;
            }
            ServerMessage::Hello(_) | ServerMessage::Unknown { .. } => {
                tracing::trace!("ignoring a viewer message with no client-side effect");
            }
        }
    }
    inner.shutdown();
}

/// Splits an optional normalized position into its optional components.
fn position(pos: Option<(f64, f64)>) -> (Option<f64>, Option<f64>) {
    match pos {
        Some((x, y)) => (Some(x), Some(y)),
        None => (None, None),
    }
}

/// Locks a standard mutex, recovering the guard on poisoning.
///
/// The client never panics while holding these locks, so poisoning is
/// unreachable in practice; recovering keeps every path panic-free.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_options_new_uses_the_crate_defaults() {
        let options = ConnectOptions::new(ViewerTarget::Unix(PathBuf::from("/run/adesk.sock")));
        assert_eq!(options.max_frame_len, DEFAULT_MAX_FRAME_LEN);
        assert_eq!(options.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(options.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
        assert!(options.client_name.is_none());
        assert_eq!(options.overlays, DEFAULT_OVERLAYS.to_vec());
        assert_eq!(options.min_interval_ms, DEFAULT_MIN_INTERVAL_MS);
        assert!(options.verify_version);
    }

    #[test]
    fn builders_override_every_field() {
        let options = ConnectOptions::new(ViewerTarget::Tcp("127.0.0.1:9".parse().unwrap()))
            .with_target(ViewerTarget::Unix(PathBuf::from("/run/other.sock")))
            .with_max_frame_len(1024)
            .with_connect_timeout(Duration::from_millis(11))
            .with_handshake_timeout(Duration::from_millis(22))
            .with_client_name("adesk-viewer")
            .with_overlays(vec![OverlayKind::Cursor])
            .with_min_interval_ms(0)
            .with_verify_version(false);
        assert_eq!(
            options.target,
            ViewerTarget::Unix(PathBuf::from("/run/other.sock"))
        );
        assert_eq!(options.max_frame_len, 1024);
        assert_eq!(options.connect_timeout, Duration::from_millis(11));
        assert_eq!(options.handshake_timeout, Duration::from_millis(22));
        assert_eq!(options.client_name.as_deref(), Some("adesk-viewer"));
        assert_eq!(options.overlays, vec![OverlayKind::Cursor]);
        assert_eq!(options.min_interval_ms, 0);
        assert!(!options.verify_version);
    }

    #[test]
    fn target_display_names_both_dialects() {
        assert_eq!(
            ViewerTarget::Unix(PathBuf::from("/run/adesk-viewer.sock")).to_string(),
            "unix:/run/adesk-viewer.sock"
        );
        let address: SocketAddr = "127.0.0.1:7000".parse().unwrap();
        assert_eq!(ViewerTarget::Tcp(address).to_string(), "tcp:127.0.0.1:7000");
    }

    #[tokio::test]
    async fn connecting_to_a_missing_socket_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.sock");
        let result = ViewerClient::connect(ViewerTarget::Unix(path)).await;
        let error = match result {
            Ok(_) => panic!("connecting to a missing socket must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, ViewerError::Io(_)), "{error:?}");
    }

    #[tokio::test]
    async fn connect_timeout_maps_to_a_timed_out_io_error() {
        let target = ViewerTarget::Unix(PathBuf::from("/run/adesk-viewer.sock"));
        let error = connect_timeout(&target);
        match error {
            ViewerError::Io(error) => assert_eq!(error.kind(), std::io::ErrorKind::TimedOut),
            other => panic!("expected an Io error, got {other:?}"),
        }
    }
}
