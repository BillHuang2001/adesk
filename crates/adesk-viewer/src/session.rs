//! Per-connection viewer session: handshake, select loop and frame pacing.
//!
//! `run_session` is the machinery behind
//! [`ViewerServer::serve`](crate::server::ViewerServer::serve). It drives **one**
//! viewer connection over any `AsyncRead + AsyncWrite` stream:
//!
//! 1. it reads and validates the mandatory `ViewerHello` handshake and replies
//!    with the backend's `ServerHello` (`docs/viewer.md` §2);
//! 2. it then runs a single `select!` loop that applies the viewer's messages in
//!    submission order (`docs/viewer.md` §5) and pushes frames **on demand** —
//!    only when the backend reports a desktop change or the pacing timer fires,
//!    never on a background loop (`docs/viewer.md` §5).
//!
//! The session owns no transport: `serve` receives an already-connected stream.
//! No pixel payload or message body is ever logged; only message types and
//! counts at `debug`/`trace`.

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use adesk_core::ErrorCode;
use adesk_viewer_proto::{
    check_version, decode_client, encode_server, AppEntry, ClientMessage, LaunchOutcome,
    RecordingStatus, ServerMessage, ViewerHello,
};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader};
use tokio::time::Instant;

use crate::backend::{RecordRequest, ViewerBackend, ViewerInput};
use crate::error::{Result, ViewerError};
use crate::server::{PeerInfo, ViewerServerConfig};
use crate::transport::{read_line, write_line, LineReader};

/// Runs one viewer connection to completion (`docs/viewer.md` §2–§6).
///
/// `stream` is an already-connected bidirectional byte stream; `peer` is a log
/// label only. The function returns `Ok(())` once the viewer leaves (EOF, `bye`,
/// or a handshake/protocol violation that closes the connection) and `Err` only
/// when the transport itself fails.
///
/// # Errors
///
/// Returns [`ViewerError::Io`] on a stream failure, and
/// [`ViewerError::Handshake`] when the viewer never sends a `hello` (timeout or
/// EOF before it).
pub(crate) async fn run_session<B, S>(
    backend: &Arc<B>,
    config: &ViewerServerConfig,
    stream: S,
    peer: &PeerInfo,
) -> Result<()>
where
    B: ViewerBackend,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (read_half, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read_half);

    // --- Handshake (§2) -----------------------------------------------------
    // `perform_handshake` returns the validated hello, or `None` after it has
    // already answered the viewer with an `error` and the connection must close.
    let hello = match perform_handshake(&mut read, &mut write, config, peer).await? {
        Some(hello) => hello,
        None => return Ok(()),
    };

    // Negotiated connection settings (§2). An empty overlay list and a
    // `min_interval_ms` of `0` mean "use the server default". Because
    // `ViewerServerConfig::default().default_min_interval_ms` is `0`, a viewer
    // that asks for no pacing also gets no pacing by default (§2).
    //
    // Overlays are negotiated for the connection's lifetime but v1 does not
    // plumb them into rendering: `ViewerBackend::render_frame` takes no overlay
    // argument, so the runtime backend owns its overlay set (§2). We only record
    // the negotiated set here for the debug log.
    let overlays = if hello.overlays.is_empty() {
        config.default_overlays.clone()
    } else {
        hello.overlays.clone()
    };
    let min_interval_ms = if hello.min_interval_ms == 0 {
        config.default_min_interval_ms
    } else {
        hello.min_interval_ms
    };
    tracing::debug!(
        peer = %peer,
        client = ?hello.client,
        min_interval_ms,
        overlays = ?overlays,
        "viewer handshake accepted"
    );

    // Handshake reply (§2).
    send(&mut write, &ServerMessage::Hello(backend.display())).await?;

    // --- Select loop (§5) ---------------------------------------------------
    let change = backend.change_signal();
    let interval = Duration::from_millis(min_interval_ms);
    // Time of the last pushed frame; `None` before the first frame, so the first
    // change flushes immediately even when paced.
    let mut last_sent: Option<Instant> = None;
    // A desktop change arrived while the pacing interval had not yet elapsed.
    let mut pending = false;
    // The read-only requests received but not served yet, in submission order
    // (see [`ReadRequest`]): they are handed to [`serve_read`] one at a time, off
    // this loop, so the input messages behind them are read and applied without
    // waiting for a render or a state query.
    let mut queued: VecDeque<ReadRequest> = VecDeque::new();
    // The one read-only request currently being served; its reply is written here
    // once it is ready. Keeping it out of the read arm is what stops a slow
    // render/state query from delaying the input messages queued behind it.
    let mut deferred: Option<Deferred> = None;
    // One inbound buffer reused for the whole connection. It lives inside the
    // reader, so an arm of this `select!` that wins mid-line leaves a
    // partially-read line intact for the next iteration instead of dropping its
    // prefix (the reader's own docs spell out why a `buf.clear()`-style read
    // cannot be used here).
    let mut lines = LineReader::new(read, config.max_frame_len);

    loop {
        // Start the next queued read-only request as soon as the previous one has
        // been answered, so at most one of them is in flight and their replies keep
        // their submission order.
        if deferred.is_none() {
            if let Some(request) = queued.pop_front() {
                let serving: Deferred = Box::pin(serve_read(Arc::clone(backend), request));
                deferred = Some(serving);
            }
        }
        // The pacing arm is armed only while a change is pending; otherwise it is
        // a never-resolving future so the loop only wakes on input or a change.
        // `pending`, `last_sent` and `interval` are `Copy`, so this copies them.
        let pacing = async move {
            if pending {
                let deadline = last_sent.map_or_else(Instant::now, |last| last + interval);
                tokio::time::sleep_until(deadline).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            inbound = lines.read_line() => {
                match inbound {
                    // Clean EOF: the viewer went away (§5).
                    Ok(false) => return Ok(()),
                    Ok(true) => {}
                    // Framing corruption (over-cap line or invalid UTF-8) is a
                    // protocol error that closes the connection (§6). A partial
                    // line is never reported as corruption: the reader keeps it.
                    Err(ViewerError::Transport(_)) => {
                        send_error(&mut write, ErrorCode::InvalidRequest, "malformed message").await?;
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }

                let line = match std::str::from_utf8(lines.line()) {
                    Ok(line) => line,
                    // Unreachable: the reader already rejected invalid UTF-8;
                    // handled like any other malformed message.
                    Err(_) => {
                        send_error(&mut write, ErrorCode::InvalidRequest, "malformed message").await?;
                        return Ok(());
                    }
                };

                let message = match decode_client(line) {
                    Ok(message) => message,
                    Err(_) => {
                        send_error(&mut write, ErrorCode::InvalidRequest, "malformed message").await?;
                        return Ok(());
                    }
                };

                // A read-only request is queued for [`serve_read`], so the input
                // messages behind it are read and applied without waiting for the
                // backend (`docs/viewer.md` §5 orders *input*, mirroring
                // `docs/protocol.md` §5.5, where every other request is
                // dispatched concurrently and answered in completion order).
                // Everything else — input, window management, control, recording
                // transitions, app launches and the lifecycle messages — is
                // applied here, in submission order.
                match ReadRequest::of(&message) {
                    Some(request) => queued.push_back(request),
                    None => match handle_message(backend, &mut write, message).await? {
                        Disposition::Continue => {}
                        Disposition::Close => return Ok(()),
                    },
                }
            }
            reply = async {
                match deferred.as_mut() {
                    Some(serving) => serving.await,
                    // No read-only request in flight: this arm never resolves.
                    None => std::future::pending::<ServerMessage>().await,
                }
            } => {
                // The slot is free again; the reply is written here, so the loop
                // stays the one writer and read replies keep their submission
                // order.
                deferred = None;
                send(&mut write, &reply).await?;
            }
            () = change.changed() => {
                // A desktop change: push a frame now if pacing allows, else
                // collapse it into the pending change (§5).
                if interval.is_zero()
                    || last_sent.map_or(true, |last| last.elapsed() >= interval)
                {
                    push_frame(backend, &mut write).await?;
                    last_sent = Some(Instant::now());
                    pending = false;
                } else {
                    pending = true;
                }
            }
            () = pacing => {
                // The pacing deadline fired: flush the collapsed pending change.
                pending = false;
                push_frame(backend, &mut write).await?;
                last_sent = Some(Instant::now());
            }
        }
    }
}

/// Whether the session keeps running after handling a message.
#[derive(Debug)]
enum Disposition {
    /// Keep the connection open.
    Continue,
    /// Close the connection with a successful session result.
    Close,
}

/// A read-only viewer request the session serves off its read loop
/// (`docs/viewer.md` §4, §5).
///
/// `request_frame`, `request_state`, `request_recording` and `list_apps` change
/// no runtime state — their only effect is the reply. Serving one on its own
/// future lets the read loop keep reading and applying the *input* messages
/// behind it, so a slow render or state query cannot starve pointer or keyboard
/// input. `docs/viewer.md` §5's ordering rule is the input ordering of
/// `docs/protocol.md` §5.5, where every non-input request is dispatched
/// concurrently and answered in completion order.
///
/// These requests are held in submission order and served **one at a time**, and
/// the loop writes each reply itself, so read replies stay in submission order
/// and the session keeps its single writer.
enum ReadRequest {
    /// `request_frame`: render the current desktop.
    Frame,
    /// `request_state`: report the current desktop metadata.
    State,
    /// `request_recording`: report the recording status.
    Recording { id: Option<u64> },
    /// `list_apps`: report the launchable applications.
    Apps {
        id: Option<u64>,
        query: Option<String>,
    },
}

impl ReadRequest {
    /// The read-only request `message` carries, or `None` for every other message
    /// (those are applied inline, in submission order).
    fn of(message: &ClientMessage) -> Option<ReadRequest> {
        match message {
            ClientMessage::RequestFrame { .. } => Some(ReadRequest::Frame),
            ClientMessage::RequestState { .. } => Some(ReadRequest::State),
            ClientMessage::RequestRecording { id } => Some(ReadRequest::Recording { id: *id }),
            ClientMessage::ListApps { id, query } => Some(ReadRequest::Apps {
                id: *id,
                query: query.clone(),
            }),
            _ => None,
        }
    }
}

/// A read-only request in flight: a boxed future that resolves to the reply the
/// session writes.
type Deferred = Pin<Box<dyn Future<Output = ServerMessage> + Send>>;

/// Serves one read-only request and returns the reply to write
/// (`docs/viewer.md` §4, §5, §6).
///
/// The reply is built with the same helpers the inline path uses, so the wire
/// bytes and the backend-error mapping are identical; only *when* it is written
/// differs.
async fn serve_read<B: ViewerBackend>(backend: Arc<B>, request: ReadRequest) -> ServerMessage {
    match request {
        ReadRequest::Frame => match backend.render_frame().await {
            Ok(frame) => ServerMessage::Frame(frame),
            Err(error) => error_reply(ErrorCode::RenderFailed, error.to_string(), None),
        },
        ReadRequest::State => match backend.desktop_state().await {
            Ok(state) => ServerMessage::State(state),
            Err(error) => error_reply(ErrorCode::Internal, error.to_string(), None),
        },
        ReadRequest::Recording { id } => recording_reply(id, backend.recording_status().await),
        ReadRequest::Apps { id, query } => apps_reply(id, backend.list_apps(query).await),
    }
}

/// Reads and validates the handshake, answering a refused viewer
/// (`docs/viewer.md` §2).
///
/// Returns `Ok(Some(hello))` when the viewer may proceed. Returns `Ok(None)`
/// after answering an invalid handshake with a VAP `error`, in which case the
/// caller closes the connection. Transport failures are propagated as `Err`.
async fn perform_handshake<R, W>(
    read: &mut R,
    write: &mut W,
    config: &ViewerServerConfig,
    peer: &PeerInfo,
) -> Result<Option<ViewerHello>>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // §2: the first message must arrive within the handshake timeout.
    let line = match tokio::time::timeout(
        config.handshake_timeout,
        read_line(read, config.max_frame_len),
    )
    .await
    {
        Ok(result) => result?,
        Err(_elapsed) => {
            tracing::debug!(peer = %peer, "viewer handshake timed out");
            return Err(ViewerError::Handshake("handshake timed out".to_owned()));
        }
    };

    let line = match line {
        Some(line) => line,
        None => {
            tracing::debug!(peer = %peer, "viewer closed before hello");
            return Err(ViewerError::Handshake(
                "connection closed before hello".to_owned(),
            ));
        }
    };

    let hello = match decode_client(&line) {
        Ok(ClientMessage::Hello(hello)) => hello,
        // A malformed first line is refused (§2).
        Err(_) => {
            send_error(
                write,
                ErrorCode::InvalidRequest,
                "malformed handshake message",
            )
            .await?;
            return Ok(None);
        }
        // Anything but a `hello` (including `Unknown`) is refused (§2).
        Ok(_) => {
            send_error(write, ErrorCode::InvalidRequest, "expected hello").await?;
            return Ok(None);
        }
    };

    // A version mismatch is a hard error: the server answers `error` with
    // `protocol_version_mismatch` and closes (§2).
    if let Err(mismatch) = check_version(hello.protocol_version) {
        send_error(write, mismatch.error_code(), mismatch.to_string()).await?;
        return Ok(None);
    }

    Ok(Some(hello))
}

/// Handles one decoded client message, applying it in submission order
/// (`docs/viewer.md` §4, §5).
async fn handle_message<B, W>(
    backend: &Arc<B>,
    write: &mut W,
    message: ClientMessage,
) -> Result<Disposition>
where
    B: ViewerBackend,
    W: AsyncWrite + Unpin,
{
    match message {
        // Headless pull: render and push one frame now (§4, §5).
        ClientMessage::RequestFrame { .. } => {
            match backend.render_frame().await {
                Ok(frame) => send(write, &ServerMessage::Frame(frame)).await?,
                // A render failure answers `error` and keeps the connection open
                // (§6).
                Err(error) => send_error(write, ErrorCode::RenderFailed, error.to_string()).await?,
            }
            Ok(Disposition::Continue)
        }
        // Push the current desktop metadata (§4).
        ClientMessage::RequestState { .. } => {
            match backend.desktop_state().await {
                Ok(state) => send(write, &ServerMessage::State(state)).await?,
                Err(error) => send_error(write, ErrorCode::Internal, error.to_string()).await?,
            }
            Ok(Disposition::Continue)
        }
        ClientMessage::PointerMove { x, y } => {
            apply_input(backend, write, ViewerInput::PointerMove { x, y }).await
        }
        ClientMessage::PointerButton {
            button,
            state,
            x,
            y,
        } => {
            apply_input(
                backend,
                write,
                ViewerInput::PointerButton {
                    button,
                    state,
                    x,
                    y,
                },
            )
            .await
        }
        ClientMessage::Scroll { dx, dy, x, y } => {
            apply_input(backend, write, ViewerInput::Scroll { dx, dy, x, y }).await
        }
        ClientMessage::Key { keys, action } => {
            apply_input(backend, write, ViewerInput::Key { keys, action }).await
        }
        ClientMessage::Text { text } => {
            apply_input(backend, write, ViewerInput::Text { text }).await
        }
        // Runtime-native window switch (§5): ordered and acknowledged exactly
        // like input, but it changes compositor state instead of the seat.
        ClientMessage::ActivateWindow { window_id } => {
            apply_input(backend, write, ViewerInput::ActivateWindow { window_id }).await
        }
        // Runtime-native window management (§5), ordered and acknowledged exactly
        // like input and never synthesized input.
        ClientMessage::CloseWindow { window_id } => {
            apply_input(backend, write, ViewerInput::CloseWindow { window_id }).await
        }
        // Advisory control handshake (§5).
        ClientMessage::SetControl { owner } => {
            match backend.set_control(owner).await {
                Ok(()) => send(write, &ServerMessage::Control { owner }).await?,
                Err(error) => send_error(write, ErrorCode::Internal, error.to_string()).await?,
            }
            Ok(Disposition::Continue)
        }
        // Recording control (§4, §5). Recording is orthogonal to input: it never
        // touches the seat and never reorders input messages, and each reply
        // echoes the request id like `input_ack`/`error`.
        ClientMessage::StartRecording {
            id,
            path,
            fps,
            encoder,
        } => {
            let request = RecordRequest {
                path: path.map(PathBuf::from),
                fps,
                encoder,
            };
            let result = backend.start_recording(request).await;
            answer_recording(write, id, result).await?;
            Ok(Disposition::Continue)
        }
        ClientMessage::StopRecording { id } => {
            let result = backend.stop_recording().await;
            answer_recording(write, id, result).await?;
            Ok(Disposition::Continue)
        }
        ClientMessage::RequestRecording { id } => {
            let result = backend.recording_status().await;
            answer_recording(write, id, result).await?;
            Ok(Disposition::Continue)
        }
        // Application discovery and launch (§4, §5). Like the recording controls
        // each reply echoes the request id, so the viewer can correlate it.
        ClientMessage::ListApps { id, query } => {
            let result = backend.list_apps(query).await;
            answer_apps(write, id, result).await?;
            Ok(Disposition::Continue)
        }
        ClientMessage::LaunchApp { id, app_id } => {
            let result = backend.launch_app(app_id).await;
            answer_launch(write, id, result).await?;
            Ok(Disposition::Continue)
        }
        // The viewer is leaving: acknowledge with `bye` and close (§4).
        ClientMessage::Bye { reason } => {
            let reason = reason.unwrap_or_else(|| "viewer left".to_owned());
            // The acknowledgement is a courtesy reply, not a spec-mandated round
            // trip (§4): the viewer has *already* declared it is leaving, so a
            // peer that closes before reading our reply is a normal departure,
            // not a transport failure. Only the peer-gone error kinds on this one
            // write are downgraded; every other failure still propagates.
            if let Err(error) = send(write, &ServerMessage::Bye { reason }).await {
                if !is_peer_gone(&error) {
                    return Err(error);
                }
                tracing::debug!(
                    %error,
                    "viewer departed before its bye acknowledgement was written"
                );
            }
            Ok(Disposition::Close)
        }
        // A second handshake is a protocol error that closes (§2).
        ClientMessage::Hello(_) => {
            send_error(write, ErrorCode::InvalidRequest, "duplicate hello").await?;
            Ok(Disposition::Close)
        }
        // Forward compatibility: ignore unknown types (§1).
        ClientMessage::Unknown { message_type, .. } => {
            tracing::trace!(message_type = %message_type, "ignoring unknown viewer message");
            Ok(Disposition::Continue)
        }
    }
}

/// Applies one viewer action through the backend and answers with `input_ack`
/// when the runtime recorded an action (`docs/viewer.md` §4, §5).
///
/// VAP input messages carry no client `id`, so the ack's `id` is always `None`.
/// A backend failure keeps its AGP [`ErrorCode`] on the wire (§6) — an unknown
/// window id is answered with `unknown_window`, not a collapsed code.
async fn apply_input<B, W>(
    backend: &Arc<B>,
    write: &mut W,
    input: ViewerInput,
) -> Result<Disposition>
where
    B: ViewerBackend,
    W: AsyncWrite + Unpin,
{
    match backend.apply_input(input).await {
        Ok(Some(action_id)) => {
            send(
                write,
                &ServerMessage::InputAck {
                    id: None,
                    action_id,
                },
            )
            .await?;
        }
        // Applied but no recorded action: nothing to acknowledge.
        Ok(None) => {}
        // An undeliverable action answers `error` and keeps the connection open
        // (§6). The backend classifies the failure, so its AGP code travels to the
        // viewer unchanged.
        Err(error) => {
            send_error(
                write,
                error.code_or(ErrorCode::InvalidRequest),
                error.to_string(),
            )
            .await?;
        }
    }
    Ok(Disposition::Continue)
}

/// Builds the VAP `error` message for a failed backend call, echoing the
/// request's client `id` (`docs/viewer.md` §6).
///
/// This is the one place a backend failure becomes a wire error, so the AGP code
/// the backend classified travels to the viewer unchanged — `unknown_window` for
/// an unknown window, `not_supported` for an unavailable encoder or a runtime
/// without app control, `render_failed` for a failed render.
fn error_reply(code: ErrorCode, message: impl Into<String>, id: Option<u64>) -> ServerMessage {
    ServerMessage::Error {
        code,
        message: message.into(),
        id,
    }
}

/// The `recording` reply for a recording status result, or the `error` that
/// replaces it (`docs/viewer.md` §4, §6).
///
/// The status is sent as `recording`; the failure keeps its AGP code
/// (`not_supported` for an unavailable encoder, `invalid_request` for a
/// conflicting transition), so the viewer can distinguish them.
fn recording_reply(id: Option<u64>, result: Result<RecordingStatus>) -> ServerMessage {
    match result {
        Ok(status) => ServerMessage::Recording { id, status },
        Err(error) => error_reply(error.code_or(ErrorCode::Internal), error.to_string(), id),
    }
}

/// The `apps` reply for a registry listing result, or the `error` that replaces
/// it (`docs/viewer.md` §4, §6).
///
/// The entries are sent as `apps`; the failure keeps its AGP code
/// (`not_supported` for a runtime without an app registry), so the viewer can
/// distinguish an empty registry from an unsupported one.
fn apps_reply(id: Option<u64>, result: Result<Vec<AppEntry>>) -> ServerMessage {
    match result {
        Ok(apps) => ServerMessage::Apps { id, apps },
        Err(error) => error_reply(error.code_or(ErrorCode::Internal), error.to_string(), id),
    }
}

/// Answers a recording control message with the backend's status, exporting a
/// backend failure as a VAP `error` that echoes the request id
/// (`docs/viewer.md` §4, §6).
async fn answer_recording<W>(
    write: &mut W,
    id: Option<u64>,
    result: Result<RecordingStatus>,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    send(write, &recording_reply(id, result)).await
}

/// Answers a `list_apps` request with the backend's registry entries, exporting
/// a backend failure as a VAP `error` that echoes the request id
/// (`docs/viewer.md` §4, §6).
async fn answer_apps<W>(write: &mut W, id: Option<u64>, result: Result<Vec<AppEntry>>) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    send(write, &apps_reply(id, result)).await
}

/// Answers a `launch_app` request with the backend's launch outcome, exporting a
/// backend failure as a VAP `error` that echoes the request id
/// (`docs/viewer.md` §4, §6).
///
/// The outcome is sent as `launch_result`; the failure keeps its AGP code, so an
/// unknown application id stays distinguishable from a runtime that cannot launch
/// applications at all.
async fn answer_launch<W>(
    write: &mut W,
    id: Option<u64>,
    result: Result<LaunchOutcome>,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match result {
        Ok(result) => send(write, &ServerMessage::LaunchResult { id, result }).await,
        Err(error) => {
            send_error_id(
                write,
                error.code_or(ErrorCode::Internal),
                error.to_string(),
                id,
            )
            .await
        }
    }
}

/// Renders one frame and pushes it, reporting a backend failure as `error`
/// without closing the connection (`docs/viewer.md` §5, §6).
async fn push_frame<B, W>(backend: &Arc<B>, write: &mut W) -> Result<()>
where
    B: ViewerBackend,
    W: AsyncWrite + Unpin,
{
    match backend.render_frame().await {
        Ok(frame) => send(write, &ServerMessage::Frame(frame)).await,
        Err(error) => send_error(write, ErrorCode::RenderFailed, error.to_string()).await,
    }
}

/// Encodes and writes one server message (`docs/viewer.md` §1).
async fn send<W: AsyncWrite + Unpin>(write: &mut W, message: &ServerMessage) -> Result<()> {
    let line = encode_server(message);
    write_line(write, &line).await
}

/// Writes a VAP `error` message with no client id (`docs/viewer.md` §6).
async fn send_error<W: AsyncWrite + Unpin>(
    write: &mut W,
    code: ErrorCode,
    message: impl Into<String>,
) -> Result<()> {
    send_error_id(write, code, message, None).await
}

/// Writes a VAP `error` message echoing the request's client `id`
/// (`docs/viewer.md` §6).
async fn send_error_id<W: AsyncWrite + Unpin>(
    write: &mut W,
    code: ErrorCode,
    message: impl Into<String>,
    id: Option<u64>,
) -> Result<()> {
    send(write, &error_reply(code, message, id)).await
}

/// Whether `error` reports that the peer is already gone — a broken pipe or a
/// reset connection.
///
/// Used only to downgrade a failed courtesy write after the peer has announced
/// it is leaving (`docs/viewer.md` §4); it must never be used to hide a failure
/// on a write whose delivery the protocol actually depends on.
fn is_peer_gone(error: &ViewerError) -> bool {
    matches!(
        error,
        ViewerError::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_viewer_proto::{encode_client, ControlOwner};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    use crate::test_support::FakeBackend;

    /// The buffered line reader the tests use on the client half.
    type ClientLines =
        tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;

    fn config() -> ViewerServerConfig {
        ViewerServerConfig::default()
    }

    /// Sends the hello and returns a line reader positioned after the server's
    /// handshake reply.
    async fn start(
        backend: Arc<FakeBackend>,
        hello: ViewerHello,
    ) -> (
        tokio::task::JoinHandle<Result<()>>,
        ClientLines,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        start_with_config(backend, hello, config()).await
    }

    /// Like [`start`] but with an explicit server configuration.
    async fn start_with_config(
        backend: Arc<FakeBackend>,
        hello: ViewerHello,
        config: ViewerServerConfig,
    ) -> (
        tokio::task::JoinHandle<Result<()>>,
        ClientLines,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (server_stream, client_stream) = tokio::io::duplex(1 << 16);
        let peer = PeerInfo::Other("test".to_owned());
        let handle =
            tokio::spawn(async move { run_session(&backend, &config, server_stream, &peer).await });

        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let mut lines = tokio::io::BufReader::new(client_read).lines();

        let line = encode_client(&ClientMessage::Hello(hello));
        client_write.write_all(line.as_bytes()).await.unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.flush().await.unwrap();

        let reply = lines.next_line().await.unwrap().unwrap();
        assert!(
            reply.contains("\"hello\""),
            "expected the server hello: {reply}"
        );

        (handle, lines, client_write)
    }

    /// Sends a `bye` so the session ends without relying on stream teardown.
    async fn send_bye(write: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>) {
        let line = encode_client(&ClientMessage::Bye { reason: None });
        write.write_all(line.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();
    }

    #[tokio::test]
    async fn handshake_replies_with_the_server_hello() {
        let backend = FakeBackend::shared();
        let (handle, _lines, mut write) = start(backend, ViewerHello::new()).await;

        send_bye(&mut write).await;

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_frame_is_answered_with_a_frame() {
        let backend = FakeBackend::shared();
        let (handle, mut lines, mut write) = start(backend, ViewerHello::new()).await;

        let request = encode_client(&ClientMessage::RequestFrame { id: None });
        write.write_all(request.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();

        let frame = lines.next_line().await.unwrap().unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn change_pushes_a_frame_and_input_is_forwarded() {
        let backend = FakeBackend::shared();
        let (handle, mut lines, mut write) = start(backend.clone(), ViewerHello::new()).await;

        // A desktop change pushes a frame (unpaced: one frame per change).
        backend.notify_change();
        let frame = lines.next_line().await.unwrap().unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        // Input is applied through the backend and acknowledged with the action.
        let move_line = encode_client(&ClientMessage::PointerMove { x: 0.5, y: 0.25 });
        write.write_all(move_line.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();

        let ack = lines.next_line().await.unwrap().unwrap();
        assert!(ack.contains("\"input_ack\""), "{ack}");
        assert_eq!(
            backend.last_action(),
            Some(ViewerInput::PointerMove { x: 0.5, y: 0.25 })
        );

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn malformed_handshake_is_refused_and_closes() {
        let backend = FakeBackend::shared();
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let config = config();
        let peer = PeerInfo::Other("test".to_owned());
        let handle =
            tokio::spawn(async move { run_session(&backend, &config, server_stream, &peer).await });

        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let mut lines = tokio::io::BufReader::new(client_read).lines();

        client_write.write_all(b"not json\n").await.unwrap();
        client_write.flush().await.unwrap();

        let error = lines.next_line().await.unwrap().unwrap();
        assert!(error.contains("\"error\""), "{error}");
        assert!(error.contains("malformed handshake message"), "{error}");

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn version_mismatch_is_a_hard_error() {
        let backend = FakeBackend::shared();
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let config = config();
        let peer = PeerInfo::Other("test".to_owned());
        let handle =
            tokio::spawn(async move { run_session(&backend, &config, server_stream, &peer).await });

        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let mut lines = tokio::io::BufReader::new(client_read).lines();

        let mut hello = ViewerHello::new();
        hello.protocol_version = adesk_viewer_proto::PROTOCOL_VERSION + 1;
        let line = encode_client(&ClientMessage::Hello(hello));
        client_write.write_all(line.as_bytes()).await.unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.flush().await.unwrap();

        let error = lines.next_line().await.unwrap().unwrap();
        assert!(
            error.contains("protocol_version_mismatch"),
            "expected a version mismatch error: {error}"
        );

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unknown_message_type_is_ignored() {
        let backend = FakeBackend::shared();
        let (handle, mut lines, mut write) = start(backend, ViewerHello::new()).await;

        // An unrecognised `"type"` decodes to `Unknown` and is ignored (§1).
        write
            .write_all(br#"{"type":"something_new","x":1}"#)
            .await
            .unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();

        // The connection is still usable: a frame request is answered.
        let request = encode_client(&ClientMessage::RequestFrame { id: None });
        write.write_all(request.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();

        let frame = lines.next_line().await.unwrap().unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn eof_before_hello_is_a_handshake_error() {
        let backend = FakeBackend::shared();
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let config = config();
        let peer = PeerInfo::Other("test".to_owned());
        let handle =
            tokio::spawn(async move { run_session(&backend, &config, server_stream, &peer).await });

        drop(client_stream);

        let error = handle.await.unwrap().unwrap_err();
        assert!(
            matches!(error, ViewerError::Handshake(_)),
            "expected a handshake error, got {error:?}"
        );
    }

    #[tokio::test]
    async fn request_state_and_set_control_round_trip() {
        let backend = FakeBackend::shared();
        let (handle, mut lines, mut write) = start(backend, ViewerHello::new()).await;

        let state = encode_client(&ClientMessage::RequestState { id: None });
        write.write_all(state.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();
        let reply = lines.next_line().await.unwrap().unwrap();
        assert!(reply.contains("\"state\""), "{reply}");

        let control = encode_client(&ClientMessage::SetControl {
            owner: ControlOwner::Human,
        });
        write.write_all(control.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();
        let reply = lines.next_line().await.unwrap().unwrap();
        assert!(reply.contains("\"control\""), "{reply}");
        assert!(reply.contains("human"), "{reply}");

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn render_failure_answers_error_and_keeps_the_connection_open() {
        let backend = FakeBackend::shared();
        backend.set_fail_render(true);
        let (handle, mut lines, mut write) = start(backend.clone(), ViewerHello::new()).await;

        let request = encode_client(&ClientMessage::RequestFrame { id: None });
        write.write_all(request.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();
        let error = lines.next_line().await.unwrap().unwrap();
        assert!(error.contains("render_failed"), "{error}");

        // The connection is still usable afterwards (§6).
        backend.set_fail_render(false);
        write.write_all(request.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();
        let frame = lines.next_line().await.unwrap().unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn paced_changes_collapse_into_a_single_frame_per_interval() {
        let backend = FakeBackend::shared();
        // The viewer asks for no pacing (`min_interval_ms == 0`), so the server
        // default (`200ms`) applies (§2).
        let config = ViewerServerConfig::default().with_default_min_interval_ms(200);
        let (handle, mut lines, mut write) =
            start_with_config(backend.clone(), ViewerHello::new(), config).await;

        // The first change flushes immediately (no previous frame).
        backend.notify_change();
        let frame = lines.next_line().await.unwrap().unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        // A second change within the interval is deferred, not queued: no frame
        // arrives right away.
        backend.notify_change();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lines.next_line())
                .await
                .is_err(),
            "a paced change must not flush before the interval elapses"
        );

        // The pacing deadline then flushes the collapsed change exactly once.
        let frame = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("the pacing timer flushes a frame")
            .unwrap()
            .unwrap();
        assert!(frame.contains("\"frame\""), "{frame}");

        send_bye(&mut write).await;
        handle.await.unwrap().unwrap();
    }

    /// An [`AsyncWrite`] whose every write fails with a fixed error kind, to
    /// simulate a peer that is already gone.
    struct FailingWriter {
        kind: std::io::ErrorKind,
    }

    impl AsyncWrite for FailingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::from(self.kind)))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// §4: a failed courtesy acknowledgement after an already-received `bye` is
    /// not an error — the viewer declared it is leaving, so the session closes
    /// cleanly instead of surfacing a peer-gone transport failure.
    #[tokio::test]
    async fn bye_acknowledgement_to_a_gone_peer_is_a_clean_exit() {
        let backend = FakeBackend::shared();
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
        ] {
            let mut write = FailingWriter { kind };
            let disposition =
                handle_message(&backend, &mut write, ClientMessage::Bye { reason: None })
                    .await
                    .expect("a peer-gone bye acknowledgement must not fail the session");
            assert!(matches!(disposition, Disposition::Close));
        }
    }

    /// §4: any other write failure while acknowledging a `bye` still propagates,
    /// so a genuine transport fault is never silently hidden.
    #[tokio::test]
    async fn bye_acknowledgement_failure_that_is_not_peer_gone_propagates() {
        let backend = FakeBackend::shared();
        let mut write = FailingWriter {
            kind: std::io::ErrorKind::WouldBlock,
        };
        let error = handle_message(&backend, &mut write, ClientMessage::Bye { reason: None })
            .await
            .expect_err("a non-peer-gone write failure must propagate");
        assert!(matches!(error, ViewerError::Io(_)), "{error:?}");
    }

    /// `is_peer_gone` matches exactly the two peer-gone kinds and nothing else.
    #[test]
    fn peer_gone_matches_only_broken_pipe_and_connection_reset() {
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
        ] {
            assert!(is_peer_gone(&ViewerError::Io(std::io::Error::from(kind))));
        }
        for kind in [
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            assert!(!is_peer_gone(&ViewerError::Io(std::io::Error::from(kind))));
        }
        assert!(!is_peer_gone(&ViewerError::Closed));
        assert!(!is_peer_gone(&ViewerError::Transport("x".to_owned())));
    }
}
