//! Integration tests for the VAP client SDK.
//!
//! These drive a real [`ViewerClient`] against a real [`ViewerServer`] over a
//! real Unix domain socket bound in a `tempfile` directory — no network, no
//! display, no GPU. The shared [`FakeBackend`](common::FakeBackend) stands in for
//! the runtime, so the suite exercises the full client → transport → session →
//! backend round trip (`docs/viewer.md` §1–§6).
//!
//! Every test observes its assertions through protocol messages (handshake,
//! frames, `input_ack`s) rather than fixed sleeps; a `tokio::time::timeout`
//! wraps anything that could otherwise park forever on a bug.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use adesk_core::{
    ActionId, AppId, Button, ButtonState, ErrorCode, Rect, Size, WindowId, WindowInfo, WindowState,
};
use adesk_proto::{KeySpec, RendererKind};
use adesk_viewer::Result as ViewerResult;
use adesk_viewer::{
    ConnectOptions, PeerInfo, RecordRequest, ViewerClient, ViewerError, ViewerInput, ViewerServer,
    ViewerTarget,
};
use adesk_viewer_proto::{
    encode_server, ControlOwner, CursorState, DesktopState, KeyAction, RecordingEncoder,
    RecordingStatus, ServerHello, ServerMessage, PROTOCOL_VERSION,
};
use futures::pin_mut;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use common::FakeBackend;

/// The `ActionId` every fake input is recorded under.
const ACTION: ActionId = ActionId(11);

/// Generous bound for any step that depends on the peer making progress.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// The desktop state the fake backend reports and the tests expect back.
fn desktop_state() -> DesktopState {
    DesktopState {
        active_window_id: Some(WindowId(7)),
        windows: vec![WindowInfo {
            id: WindowId(7),
            app_id: Some(AppId::from("org.example.Fake")),
            title: Some("Fake Window".to_owned()),
            geometry: Rect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            state: WindowState::Active,
            mapped: true,
            pid: Some(4242),
            created_seq: 1,
            last_commit_seq: 2,
            popup_count: 0,
        }],
    }
}

/// A stand-in server hello, shared by the fake backend and the raw-wire tests.
fn server_hello() -> ServerHello {
    ServerHello {
        protocol_version: PROTOCOL_VERSION,
        runtime_version: "0.1.0".to_owned(),
        output: Size::new(800, 600),
        renderer: RendererKind::Pixman,
        cursor: CursorState::hidden(),
        control: ControlOwner::Ai,
    }
}

/// A running server: the bound socket path, the backend the test manipulates,
/// and the task that serves the single accepted connection to completion.
struct Harness {
    /// Keeps the socket directory alive for the test's lifetime.
    _dir: TempDir,
    /// The Unix socket the client connects to.
    path: PathBuf,
    /// The backend the accepted connection is served with.
    backend: Arc<FakeBackend>,
    /// Resolves with the session's result once the connection ends.
    serve: JoinHandle<ViewerResult<()>>,
}

/// The recording status the client suite's fake reports: a finished 12-frame,
/// 400 ms recording at a fixed path and encoder.
fn recording_status() -> RecordingStatus {
    RecordingStatus::idle()
        .with_path("/tmp/adesk-rec-7.mkv".to_owned())
        .with_encoder("software".to_owned())
        .with_counts(12, 400)
}

/// Binds a Unix socket in a fresh temp dir and spawns an accept-and-serve task.
fn start_server() -> Harness {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("viewer.sock");
    let listener = UnixListener::bind(&path).expect("bind the unix socket");
    // The client suite's fake: an 800×600 desktop with one active window, an
    // advancing `ts_ms` and `ACTION` recorded for every applied input.
    let backend = Arc::new(
        FakeBackend::default()
            .with_display(server_hello())
            .with_desktop(desktop_state())
            .with_action(ACTION)
            .with_ts_ms(None)
            .with_recording_status(recording_status()),
    );
    let server_backend = Arc::clone(&backend);
    let peer_path = path.clone();
    let serve = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.map_err(ViewerError::Io)?;
        ViewerServer::new(server_backend)
            .serve(stream, PeerInfo::Unix(peer_path))
            .await
    });
    Harness {
        _dir: dir,
        path,
        backend,
        serve,
    }
}

/// Connects a client to `harness`, failing the test on a hang or an error.
async fn connect(harness: &Harness) -> ViewerClient {
    let target = ViewerTarget::Unix(harness.path.clone());
    tokio::time::timeout(STEP_TIMEOUT, ViewerClient::connect(target))
        .await
        .expect("connect must not hang")
        .expect("connect must succeed")
}

#[tokio::test]
async fn connect_performs_the_handshake_over_unix() {
    let harness = start_server();
    let client = connect(&harness).await;

    let hello = client.hello();
    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
    assert_eq!(hello.output, Size::new(800, 600));
    assert_eq!(hello.renderer, RendererKind::Pixman);
    assert_eq!(hello.runtime_version, "0.1.0");
    assert_eq!(hello.control, ControlOwner::Ai);

    assert_eq!(client.socket_path(), Some(harness.path.as_path()));
    assert_eq!(client.target(), &ViewerTarget::Unix(harness.path.clone()));
}

#[tokio::test]
async fn request_frame_returns_a_rendered_frame() {
    let harness = start_server();
    let client = connect(&harness).await;

    let frame = tokio::time::timeout(STEP_TIMEOUT, client.request_frame())
        .await
        .expect("request_frame must not hang")
        .expect("request_frame must succeed");

    assert!(frame.seq >= 1, "seq must be plausible, got {}", frame.seq);
    assert_eq!(frame.image.width, 1);
    assert_eq!(frame.image.height, 1);
    assert_eq!(frame.active_window_id, Some(WindowId(7)));

    // A second request renders again and advances the sequence.
    let second = tokio::time::timeout(STEP_TIMEOUT, client.request_frame())
        .await
        .expect("request_frame must not hang")
        .expect("request_frame must succeed");
    assert_eq!(second.seq, frame.seq + 1);
}

#[tokio::test]
async fn request_state_returns_the_backend_desktop_state() {
    let harness = start_server();
    let client = connect(&harness).await;

    let state = tokio::time::timeout(STEP_TIMEOUT, client.request_state())
        .await
        .expect("request_state must not hang")
        .expect("request_state must succeed");

    assert_eq!(state, desktop_state());
}

#[tokio::test]
async fn input_methods_reach_the_backend() {
    let harness = start_server();
    let client = connect(&harness).await;

    // Subscribe *before* sending any input so no ack can be missed.
    let acks = client.input_ack();
    pin_mut!(acks);

    // `set_control` is ordered ahead of the inputs: because the session applies
    // messages in submission order, the input acks below prove it was handled.
    client
        .set_control(ControlOwner::Human)
        .await
        .expect("set_control");

    client.pointer_move(0.5, 0.5).await.expect("pointer_move");
    client
        .pointer_button(Button::Left, ButtonState::Pressed, None)
        .await
        .expect("pointer_button");
    client
        .scroll(1.5, -2.5, Some((0.1, 0.2)))
        .await
        .expect("scroll");
    client
        .key(KeySpec::from("a"), KeyAction::Tap)
        .await
        .expect("key");
    client.text("hi").await.expect("text");

    // One `input_ack` per applied input, all carrying the recorded action id.
    for _ in 0..5 {
        let ack = tokio::time::timeout(STEP_TIMEOUT, acks.next())
            .await
            .expect("input_ack must not hang")
            .expect("the ack stream must stay open");
        assert_eq!(ack, (None, ACTION));
    }

    assert_eq!(
        harness.backend.recorded_inputs(),
        vec![
            ViewerInput::PointerMove { x: 0.5, y: 0.5 },
            ViewerInput::PointerButton {
                button: Button::Left,
                state: ButtonState::Pressed,
                x: None,
                y: None,
            },
            ViewerInput::Scroll {
                dx: 1.5,
                dy: -2.5,
                x: Some(0.1),
                y: Some(0.2),
            },
            ViewerInput::Key {
                keys: KeySpec::from("a"),
                action: KeyAction::Tap,
            },
            ViewerInput::Text {
                text: "hi".to_owned(),
            },
        ]
    );
    assert_eq!(
        harness.backend.recorded_controls(),
        vec![ControlOwner::Human]
    );
}

#[tokio::test]
async fn activate_window_reaches_the_backend_and_is_acknowledged() {
    let harness = start_server();
    let client = connect(&harness).await;

    // Subscribe *before* sending so the ack cannot be missed.
    let acks = client.input_ack();
    pin_mut!(acks);

    client
        .activate_window(WindowId(9))
        .await
        .expect("activate_window");

    let ack = tokio::time::timeout(STEP_TIMEOUT, acks.next())
        .await
        .expect("input_ack must not hang")
        .expect("the ack stream must stay open");
    assert_eq!(ack, (None, ACTION));

    assert_eq!(
        harness.backend.recorded_inputs(),
        vec![ViewerInput::ActivateWindow {
            window_id: WindowId(9)
        }]
    );
}

#[tokio::test]
async fn frames_stream_pushes_a_frame_on_a_desktop_change() {
    let harness = start_server();
    let client = connect(&harness).await;

    // Subscribe to the pushed-frame stream, then signal a change. The change
    // flushes immediately because no frame was sent before it.
    let frames = client.frames();
    pin_mut!(frames);
    harness.backend.change.notify();

    let frame = tokio::time::timeout(STEP_TIMEOUT, frames.next())
        .await
        .expect("a pushed frame must not hang")
        .expect("the frame stream must stay open")
        .expect("the pushed frame must be Ok");

    assert!(frame.seq >= 1, "seq must be plausible, got {}", frame.seq);
    assert_eq!(frame.image.width, 1);
}

/// §4/§5: the recording SDK round-trips start/request/stop against the runtime,
/// and the request reaches the backend verbatim.
#[tokio::test]
async fn recording_methods_round_trip() {
    let harness = start_server();
    let client = connect(&harness).await;

    let request = RecordRequest::new()
        .with_path("out.mkv")
        .with_fps(15)
        .with_encoder(RecordingEncoder::Gpu);
    let started = tokio::time::timeout(STEP_TIMEOUT, client.start_recording(request.clone()))
        .await
        .expect("start_recording must not hang")
        .expect("start_recording must succeed");
    assert!(started.recording);
    assert_eq!(started.path.as_deref(), Some("/tmp/adesk-rec-7.mkv"));

    let current = tokio::time::timeout(STEP_TIMEOUT, client.request_recording())
        .await
        .expect("request_recording must not hang")
        .expect("request_recording must succeed");
    assert!(!current.recording);
    assert_eq!(current.encoder.as_deref(), Some("software"));

    let stopped = tokio::time::timeout(STEP_TIMEOUT, client.stop_recording())
        .await
        .expect("stop_recording must not hang")
        .expect("stop_recording must succeed");
    assert!(!stopped.recording);
    assert_eq!(stopped.frames, 12);
    assert_eq!(stopped.duration_ms, 400);

    assert_eq!(harness.backend.recorded_recording(), vec![request]);
}

/// §5/§6: a recording request the backend refuses surfaces as
/// [`ViewerError::Backend`] carrying the AGP `not_supported` code.
#[tokio::test]
async fn start_recording_error_maps_to_a_backend_error() {
    let harness = start_server();
    harness.backend.set_fail_recording(true);
    let client = connect(&harness).await;

    let error = tokio::time::timeout(STEP_TIMEOUT, client.start_recording(RecordRequest::new()))
        .await
        .expect("start_recording must not hang")
        .expect_err("an unavailable recorder must fail");
    match error {
        ViewerError::Backend { code, .. } => assert_eq!(code, ErrorCode::NotSupported),
        other => panic!("expected a backend error, got {other:?}"),
    }
}

/// A clean `ViewerClient::close` must deterministically make `serve()` return
/// `Ok(())`: the server writes a courtesy `bye` acknowledgement in reply, and the
/// client has to keep its read half open long enough to receive it. Looping makes
/// a flaky regression (a spurious `BrokenPipe` out of the session) visible.
#[tokio::test]
async fn close_succeeds_and_finishes_the_server_session() {
    for iteration in 0..10 {
        let harness = start_server();
        let client = connect(&harness).await;

        // A frame round trip proves the connection is live before closing.
        tokio::time::timeout(STEP_TIMEOUT, client.request_frame())
            .await
            .expect("request_frame must not hang")
            .expect("request_frame must succeed");

        // `close` performs the client half of the leave handshake and always
        // succeeds (a best-effort `bye` then a bounded wait for the reply).
        tokio::time::timeout(STEP_TIMEOUT, client.close())
            .await
            .expect("close must not hang")
            .expect("close must succeed");

        // The session for that viewer ends cleanly — never with a peer-gone
        // transport error — because the client waits for the `bye` reply.
        let served = tokio::time::timeout(STEP_TIMEOUT, harness.serve)
            .await
            .expect("the server session must finish after the viewer leaves")
            .expect("the serve task must not panic");
        assert!(
            served.is_ok(),
            "iteration {iteration}: a clean viewer close must be Ok, got {served:?}"
        );
    }
}

/// The same clean-close guarantee must hold on a multi-thread runtime, where the
/// client and the server are polled on different worker threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_succeeds_on_a_multi_thread_runtime() {
    for iteration in 0..10 {
        let harness = start_server();
        let client = connect(&harness).await;

        tokio::time::timeout(STEP_TIMEOUT, client.close())
            .await
            .expect("close must not hang")
            .expect("close must succeed");

        let served = tokio::time::timeout(STEP_TIMEOUT, harness.serve)
            .await
            .expect("the server session must finish after the viewer leaves")
            .expect("the serve task must not panic");
        assert!(
            served.is_ok(),
            "iteration {iteration}: a clean viewer close must be Ok, got {served:?}"
        );
    }
}

/// A server-initiated close is observed by the client: when the server sends
/// `bye` and drops the connection, the client's frame stream ends instead of
/// hanging, and the client can still be closed cleanly.
#[tokio::test]
async fn client_notices_a_server_initiated_bye() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("viewer.sock");
    let listener = UnixListener::bind(&path).expect("bind the unix socket");
    let server = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.expect("accept a viewer");
        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut lines = tokio::io::BufReader::new(read_half).lines();
        // Read the client handshake, then answer with a hello and immediately a
        // server-initiated `bye`.
        let handshake = lines.next_line().await.expect("read the handshake");
        assert!(handshake.is_some(), "the client must send a hello");
        let hello = encode_server(&ServerMessage::Hello(server_hello()));
        adesk_viewer::write_line(&mut write_half, &hello)
            .await
            .unwrap();
        let bye = encode_server(&ServerMessage::Bye {
            reason: "server shutting down".to_owned(),
        });
        adesk_viewer::write_line(&mut write_half, &bye)
            .await
            .unwrap();
        // Dropping the stream closes the connection from the server side.
    });

    let target = ViewerTarget::Unix(path);
    let client = tokio::time::timeout(STEP_TIMEOUT, ViewerClient::connect(target))
        .await
        .expect("connect must not hang")
        .expect("connect must succeed");

    // The server's `bye` ends the client's frame stream (it never hangs).
    let frames = client.frames();
    pin_mut!(frames);
    let next = tokio::time::timeout(STEP_TIMEOUT, frames.next())
        .await
        .expect("the client must notice the server-ended connection");
    assert!(next.is_none(), "the frame stream must end, got {next:?}");

    // A client whose peer already left still closes cleanly.
    tokio::time::timeout(STEP_TIMEOUT, client.close())
        .await
        .expect("close must not hang")
        .expect("close must succeed");

    tokio::time::timeout(STEP_TIMEOUT, server)
        .await
        .expect("the server task must finish")
        .expect("the server task must not panic");
}

/// A viewer that leaves by closing its socket without a `bye` is a clean EOF:
/// the session ends with `Ok(())` (`docs/viewer.md` §5).
#[tokio::test]
async fn session_returns_ok_when_the_viewer_leaves_cleanly() {
    let harness = start_server();
    let client = connect(&harness).await;

    // A frame round trip proves the session is live before the viewer leaves.
    tokio::time::timeout(STEP_TIMEOUT, client.request_frame())
        .await
        .expect("request_frame must not hang")
        .expect("request_frame must succeed");

    // Dropping the client closes the socket (no `bye`).
    drop(client);

    let served = tokio::time::timeout(STEP_TIMEOUT, harness.serve)
        .await
        .expect("the server session must finish when the viewer's socket closes")
        .expect("the serve task must not panic");
    assert!(
        served.is_ok(),
        "a clean viewer departure must be Ok: {served:?}"
    );
}
#[tokio::test]
async fn connect_with_version_verification_disabled_still_completes() {
    let harness = start_server();

    let options = ConnectOptions::new(ViewerTarget::Unix(harness.path.clone()))
        .with_client_name("adesk-viewer-tests")
        .with_verify_version(false);
    let client = tokio::time::timeout(STEP_TIMEOUT, ViewerClient::connect_with(options))
        .await
        .expect("connect_with must not hang")
        .expect("connect_with must succeed");

    assert_eq!(client.hello().output, Size::new(800, 600));
    assert_eq!(client.target(), &ViewerTarget::Unix(harness.path.clone()));
    assert_eq!(client.socket_path(), Some(harness.path.as_path()));
}
