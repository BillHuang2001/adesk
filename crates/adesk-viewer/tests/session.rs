//! Integration tests for the viewer **server session** (`docs/viewer.md` §2–§6).
//!
//! These drive [`ViewerServer::serve`] over an in-memory `tokio::io::duplex`
//! stream — no display, GPU, network or socket — against a fake
//! [`ViewerBackend`]. They exercise the behaviour the crate's unit tests in
//! `src/` do not reach through the public API: the handshake reply's metadata,
//! the refusal paths (version mismatch, non-hello first message, handshake
//! timeout, malformed line), the on-demand `request_frame`/`request_state`
//! round-trip, change-driven frame push, input forwarding + `input_ack`,
//! `set_control`, `bye` and forward-compatible unknown-type handling.
//!
//! Everything that could hang is wrapped in [`tokio::time::timeout`], so the
//! suite is deterministic without any fixed-sleep synchronization.

mod common;

use std::sync::Arc;
use std::time::Duration;

use adesk_core::{ActionId, Button, ButtonState, ErrorCode, Size, WindowId};
use adesk_proto::{KeySpec, RendererKind};
use adesk_viewer::{
    PeerInfo, RecordRequest, ViewerError, ViewerInput, ViewerServer, ViewerServerConfig,
};
use adesk_viewer_proto::{
    decode_server, encode_client, ClientMessage, ControlOwner, CursorState, DesktopState,
    KeyAction, RecordingEncoder, RecordingStatus, ServerHello, ServerMessage, ViewerHello,
    PROTOCOL_VERSION,
};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use common::FakeBackend;

/// The `serve` task's result type.
type Session = JoinHandle<adesk_viewer::Result<()>>;

/// The buffered client-side reader half.
type ClientRead = BufReader<ReadHalf<DuplexStream>>;

/// The client-side writer half.
type ClientWrite = WriteHalf<DuplexStream>;

/// In-memory duplex capacity, comfortably above any test frame.
const CAPACITY: usize = 1 << 16;

/// A generous upper bound for "this must happen"; the assertions are about the
/// protocol, not about timing.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The recording status the fake reports: a finished 12-frame, 400 ms recording
/// at a fixed path and encoder.
fn recording_status() -> RecordingStatus {
    RecordingStatus::idle()
        .with_path("/tmp/adesk-rec-7.mkv".to_owned())
        .with_encoder("software".to_owned())
        .with_counts(12, 400)
}

/// A fake backend configured for the session suite: an empty 1280×800 Pixman
/// desktop reported as runtime `"test"`, a fixed frame `ts_ms` of `0` and
/// `ActionId(7)` recorded for every applied input.
fn backend() -> Arc<FakeBackend> {
    Arc::new(
        FakeBackend::default()
            .with_display(ServerHello {
                protocol_version: PROTOCOL_VERSION,
                runtime_version: "test".to_owned(),
                output: Size::new(1280, 800),
                renderer: RendererKind::Pixman,
                cursor: CursorState::hidden(),
                control: ControlOwner::Ai,
            })
            .with_desktop(DesktopState {
                active_window_id: None,
                windows: Vec::new(),
            })
            .with_action(ActionId(7))
            .with_ts_ms(Some(0))
            .with_recording_status(recording_status()),
    )
}

/// Spawns a server over one half of an in-memory duplex, returning the session
/// task and the client half of the stream.
fn start(backend: Arc<FakeBackend>, config: ViewerServerConfig) -> (Session, DuplexStream) {
    let server = ViewerServer::new(backend).with_config(config);
    let (server_stream, client_stream) = tokio::io::duplex(CAPACITY);
    let handle = tokio::spawn(async move {
        server
            .serve(server_stream, PeerInfo::Other("test".to_owned()))
            .await
    });
    (handle, client_stream)
}

/// Splits a client stream into a buffered line reader and a writer.
fn split(stream: DuplexStream) -> (ClientRead, ClientWrite) {
    let (read, write) = tokio::io::split(stream);
    (BufReader::new(read), write)
}

/// Encodes, writes and flushes one client message.
async fn send<W>(writer: &mut W, message: &ClientMessage)
where
    W: AsyncWrite + Unpin,
{
    adesk_viewer::write_line(writer, &encode_client(message))
        .await
        .expect("write a message");
}

/// Writes and flushes one raw line, for malformed/unknown-type tests.
async fn send_raw<W>(writer: &mut W, line: &str)
where
    W: AsyncWrite + Unpin,
{
    adesk_viewer::write_line(writer, line)
        .await
        .expect("write a raw line");
}

/// Reads one `\n`-terminated server message; `None` on a clean EOF.
async fn recv<R>(reader: &mut R) -> Option<ServerMessage>
where
    R: AsyncBufRead + Unpin,
{
    adesk_viewer::read_line(reader, adesk_viewer::DEFAULT_MAX_FRAME_LEN)
        .await
        .expect("read a line")
        .map(|line| decode_server(&line).expect("a valid server message"))
}

/// Awaits the next server message, failing the test if none arrives in time.
async fn recv_some<R>(reader: &mut R) -> ServerMessage
where
    R: AsyncBufRead + Unpin,
{
    timeout(REPLY_TIMEOUT, recv(reader))
        .await
        .expect("a server message must arrive in time")
        .expect("the connection must stay open")
}

/// Asserts the server closed the connection (EOF instead of a message).
async fn assert_closed<R>(reader: &mut R)
where
    R: AsyncBufRead + Unpin,
{
    let next = timeout(REPLY_TIMEOUT, recv(reader))
        .await
        .expect("EOF must arrive after the server closes the connection");
    assert!(
        next.is_none(),
        "expected the connection to close, got {next:?}"
    );
}

/// Sends a hello and returns the server's first reply.
async fn handshake(
    reader: &mut ClientRead,
    writer: &mut ClientWrite,
    hello: ViewerHello,
) -> ServerMessage {
    send(writer, &ClientMessage::Hello(hello)).await;
    recv_some(reader).await
}

/// Starts a server, performs a default handshake and asserts the server hello.
async fn connected(backend: Arc<FakeBackend>) -> (Session, ClientRead, ClientWrite) {
    let (handle, stream) = start(backend, ViewerServerConfig::default());
    let (mut reader, mut writer) = split(stream);
    match handshake(&mut reader, &mut writer, ViewerHello::new()).await {
        ServerMessage::Hello(_) => {}
        other => panic!("expected the server hello, got {other:?}"),
    }
    (handle, reader, writer)
}

/// Ends a connection with `bye`, then awaits a clean session exit.
async fn bye_and_finish(handle: Session, writer: &mut ClientWrite) {
    send(writer, &ClientMessage::Bye { reason: None }).await;
    finish(handle).await;
}

/// Awaits a clean `Ok(())` session exit within the reply timeout.
async fn finish(handle: Session) {
    timeout(REPLY_TIMEOUT, handle)
        .await
        .expect("the session must finish in time")
        .expect("the server task must not panic")
        .expect("the session must return Ok");
}

/// §2: the handshake reply carries the backend's `display()` metadata.
#[tokio::test]
async fn handshake_replies_with_backend_display_metadata() {
    let backend = backend();
    let (handle, stream) = start(backend, ViewerServerConfig::default());
    let (mut reader, mut writer) = split(stream);

    match handshake(&mut reader, &mut writer, ViewerHello::new()).await {
        ServerMessage::Hello(hello) => {
            assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
            assert_eq!(hello.runtime_version, "test");
            assert_eq!(hello.output, Size::new(1280, 800));
            assert_eq!(hello.renderer, RendererKind::Pixman);
            assert_eq!(hello.cursor, CursorState::hidden());
            assert_eq!(hello.control, ControlOwner::Ai);
        }
        other => panic!("expected the server hello, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §2: a version mismatch is a hard error that closes the connection, while
/// `serve` still reports success.
#[tokio::test]
async fn version_mismatch_is_refused_and_closes() {
    let backend = backend();
    let (handle, stream) = start(backend, ViewerServerConfig::default());
    let (mut reader, mut writer) = split(stream);

    let mut hello = ViewerHello::new();
    hello.protocol_version = PROTOCOL_VERSION + 1;
    send(&mut writer, &ClientMessage::Hello(hello)).await;

    match recv_some(&mut reader).await {
        ServerMessage::Error { code, .. } => {
            assert_eq!(code, ErrorCode::ProtocolVersionMismatch);
        }
        other => panic!("expected a version mismatch error, got {other:?}"),
    }
    assert_closed(&mut reader).await;
    finish(handle).await;
}

/// §2: a first message that is not a hello is refused and the connection closes.
#[tokio::test]
async fn a_non_hello_first_message_is_refused_and_closes() {
    let backend = backend();
    let (handle, stream) = start(backend, ViewerServerConfig::default());
    let (mut reader, mut writer) = split(stream);

    send(&mut writer, &ClientMessage::RequestFrame { id: None }).await;

    match recv_some(&mut reader).await {
        ServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
        other => panic!("expected an invalid_request error, got {other:?}"),
    }
    assert_closed(&mut reader).await;
    finish(handle).await;
}

/// §2: no hello within the handshake timeout fails `serve` with a handshake
/// error (the client stream is kept alive so this is a timeout, not an EOF).
#[tokio::test]
async fn a_silent_viewer_hits_the_handshake_timeout() {
    let backend = backend();
    let config = ViewerServerConfig::default().with_handshake_timeout(Duration::from_millis(50));
    let (handle, _client) = start(backend, config);

    let result = timeout(REPLY_TIMEOUT, handle)
        .await
        .expect("the handshake timeout must fire")
        .expect("the server task must not panic");
    assert!(
        matches!(result, Err(ViewerError::Handshake(_))),
        "expected a handshake error, got {result:?}"
    );
}

/// §4/§5: `request_frame` answers a `frame` and `request_state` answers a
/// `state`.
#[tokio::test]
async fn request_frame_and_request_state_round_trip() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend).await;

    send(&mut writer, &ClientMessage::RequestFrame { id: None }).await;
    match recv_some(&mut reader).await {
        ServerMessage::Frame(frame) => assert!(frame.seq >= 1, "a rendered frame has a seq"),
        other => panic!("expected a frame, got {other:?}"),
    }

    send(&mut writer, &ClientMessage::RequestState { id: None }).await;
    match recv_some(&mut reader).await {
        ServerMessage::State(state) => assert!(state.windows.is_empty()),
        other => panic!("expected a state, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §5/§6: a failing `desktop_state` answers `error` with the `internal` code and
/// the connection stays open for the next request.
#[tokio::test]
async fn request_state_failure_answers_error_and_keeps_the_connection_open() {
    let backend = backend();
    backend.set_fail_state(true);
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    send(&mut writer, &ClientMessage::RequestState { id: None }).await;
    match recv_some(&mut reader).await {
        ServerMessage::Error { code, id, .. } => {
            assert_eq!(code, ErrorCode::Internal);
            assert_eq!(id, None);
        }
        other => panic!("expected an internal error, got {other:?}"),
    }

    // The connection is still usable afterwards (§6): the next state request
    // succeeds once the backend recovers.
    backend.set_fail_state(false);
    send(&mut writer, &ClientMessage::RequestState { id: None }).await;
    match recv_some(&mut reader).await {
        ServerMessage::State(state) => assert!(state.windows.is_empty()),
        other => panic!("expected a state, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §5: a desktop change pushes a frame without a `request_frame`.
#[tokio::test]
async fn a_desktop_change_pushes_a_frame_on_demand() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    backend.change.notify();
    match recv_some(&mut reader).await {
        ServerMessage::Frame(_) => {}
        other => panic!("expected a pushed frame, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §4/§5: a pointer move is applied through the backend and acknowledged with
/// the recorded action id.
#[tokio::test]
async fn pointer_move_is_forwarded_and_acknowledged() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    send(&mut writer, &ClientMessage::PointerMove { x: 0.5, y: 0.25 }).await;

    match recv_some(&mut reader).await {
        ServerMessage::InputAck { id, action_id } => {
            assert_eq!(id, None);
            assert_eq!(action_id, ActionId(7));
        }
        other => panic!("expected an input_ack, got {other:?}"),
    }
    assert_eq!(
        backend.recorded_inputs(),
        vec![ViewerInput::PointerMove { x: 0.5, y: 0.25 }]
    );

    bye_and_finish(handle, &mut writer).await;
}

/// §4/§5: every other input variant is forwarded verbatim and in submission
/// order.
#[tokio::test]
async fn every_input_variant_is_forwarded_in_order() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    let messages = vec![
        ClientMessage::PointerButton {
            button: Button::Right,
            state: ButtonState::Pressed,
            x: Some(0.1),
            y: None,
        },
        ClientMessage::Scroll {
            dx: 0.0,
            dy: -3.0,
            x: None,
            y: None,
        },
        ClientMessage::Key {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            action: KeyAction::Tap,
        },
        ClientMessage::Text {
            text: "hello".to_owned(),
        },
        ClientMessage::ActivateWindow {
            window_id: WindowId(17),
        },
    ];

    for message in &messages {
        send(&mut writer, message).await;
        match recv_some(&mut reader).await {
            ServerMessage::InputAck { id, action_id } => {
                assert_eq!(id, None);
                assert_eq!(action_id, ActionId(7));
            }
            other => panic!("expected an input_ack, got {other:?}"),
        }
    }

    assert_eq!(
        backend.recorded_inputs(),
        vec![
            ViewerInput::PointerButton {
                button: Button::Right,
                state: ButtonState::Pressed,
                x: Some(0.1),
                y: None,
            },
            ViewerInput::Scroll {
                dx: 0.0,
                dy: -3.0,
                x: None,
                y: None,
            },
            ViewerInput::Key {
                keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
                action: KeyAction::Tap,
            },
            ViewerInput::Text {
                text: "hello".to_owned(),
            },
            ViewerInput::ActivateWindow {
                window_id: WindowId(17),
            },
        ]
    );

    bye_and_finish(handle, &mut writer).await;
}

/// §5: `set_control` is echoed as `control` and announced to the backend.
#[tokio::test]
async fn set_control_is_echoed_and_recorded() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    send(
        &mut writer,
        &ClientMessage::SetControl {
            owner: ControlOwner::Human,
        },
    )
    .await;

    match recv_some(&mut reader).await {
        ServerMessage::Control { owner } => assert_eq!(owner, ControlOwner::Human),
        other => panic!("expected a control message, got {other:?}"),
    }
    assert_eq!(backend.recorded_controls(), vec![ControlOwner::Human]);

    bye_and_finish(handle, &mut writer).await;
}

/// §4: `bye` is acknowledged and then closes the connection.
#[tokio::test]
async fn bye_is_echoed_and_closes_the_connection() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend).await;

    send(
        &mut writer,
        &ClientMessage::Bye {
            reason: Some("done".to_owned()),
        },
    )
    .await;

    match recv_some(&mut reader).await {
        ServerMessage::Bye { reason } => assert_eq!(reason, "done"),
        other => panic!("expected a bye, got {other:?}"),
    }
    assert_closed(&mut reader).await;
    finish(handle).await;
}

/// §6: a malformed line is answered with `error` and then closes the
/// connection.
#[tokio::test]
async fn a_malformed_line_is_refused_and_closes() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend).await;

    send_raw(&mut writer, "not json").await;

    match recv_some(&mut reader).await {
        ServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
        other => panic!("expected an invalid_request error, got {other:?}"),
    }
    assert_closed(&mut reader).await;
    finish(handle).await;
}

/// §1/§7: an unrecognised message type is ignored; the connection stays usable.
#[tokio::test]
async fn an_unknown_message_type_is_ignored() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend).await;

    send_raw(&mut writer, r#"{"type":"future_thing"}"#).await;
    send(&mut writer, &ClientMessage::RequestFrame { id: None }).await;

    match recv_some(&mut reader).await {
        ServerMessage::Frame(_) => {}
        other => panic!("expected a frame after the ignored message, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §4/§5: `start_recording` is forwarded to the backend as a `RecordRequest` and
/// answered with a `recording` status that echoes the client id.
#[tokio::test]
async fn start_recording_is_forwarded_and_answered() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend.clone()).await;

    send(
        &mut writer,
        &ClientMessage::StartRecording {
            id: Some(7),
            path: Some("/tmp/out.mkv".to_owned()),
            fps: 15,
            encoder: RecordingEncoder::Software,
        },
    )
    .await;

    match recv_some(&mut reader).await {
        ServerMessage::Recording { id, status } => {
            assert_eq!(id, Some(7), "the reply echoes the request id");
            assert!(
                status.recording,
                "a started recording reports recording=true"
            );
            assert_eq!(status.path.as_deref(), Some("/tmp/adesk-rec-7.mkv"));
        }
        other => panic!("expected a recording status, got {other:?}"),
    }
    assert_eq!(
        backend.recorded_recording(),
        vec![RecordRequest::new()
            .with_path("/tmp/out.mkv")
            .with_fps(15)
            .with_encoder(RecordingEncoder::Software)]
    );

    bye_and_finish(handle, &mut writer).await;
}

/// §4/§5: `stop_recording` and `request_recording` are answered with the
/// backend's status, echoing each request id.
#[tokio::test]
async fn stop_and_request_recording_round_trip() {
    let backend = backend();
    let (handle, mut reader, mut writer) = connected(backend).await;

    send(&mut writer, &ClientMessage::StopRecording { id: Some(8) }).await;
    match recv_some(&mut reader).await {
        ServerMessage::Recording { id, status } => {
            assert_eq!(id, Some(8));
            assert!(!status.recording, "a stopped recording is not active");
            assert_eq!(status.frames, 12);
            assert_eq!(status.duration_ms, 400);
        }
        other => panic!("expected a recording status, got {other:?}"),
    }

    send(
        &mut writer,
        &ClientMessage::RequestRecording { id: Some(9) },
    )
    .await;
    match recv_some(&mut reader).await {
        ServerMessage::Recording { id, status } => {
            assert_eq!(id, Some(9));
            assert!(!status.recording);
            assert_eq!(status.encoder.as_deref(), Some("software"));
        }
        other => panic!("expected a recording status, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}

/// §5/§6: an unavailable recorder answers `error` with code `not_supported`,
/// echoing the request id, and the connection stays open.
#[tokio::test]
async fn start_recording_failure_answers_error_with_the_id() {
    let backend = backend();
    backend.set_fail_recording(true);
    let (handle, mut reader, mut writer) = connected(backend).await;

    send(
        &mut writer,
        &ClientMessage::StartRecording {
            id: Some(5),
            path: None,
            fps: 30,
            encoder: RecordingEncoder::Auto,
        },
    )
    .await;

    match recv_some(&mut reader).await {
        ServerMessage::Error { code, id, .. } => {
            assert_eq!(code, ErrorCode::NotSupported);
            assert_eq!(id, Some(5));
        }
        other => panic!("expected a not_supported error, got {other:?}"),
    }

    // The connection is still usable afterwards (§6).
    send(&mut writer, &ClientMessage::RequestRecording { id: None }).await;
    match recv_some(&mut reader).await {
        ServerMessage::Error { code, .. } => assert_eq!(code, ErrorCode::NotSupported),
        other => panic!("expected a not_supported error, got {other:?}"),
    }

    bye_and_finish(handle, &mut writer).await;
}
