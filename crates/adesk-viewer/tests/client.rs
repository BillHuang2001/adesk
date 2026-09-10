//! Integration tests for the VAP client SDK.
//!
//! These drive a real [`ViewerClient`] against a real [`ViewerServer`] over a
//! real Unix domain socket bound in a `tempfile` directory — no network, no
//! display, no GPU. A test-local [`FakeBackend`] stands in for the runtime, so
//! the suite exercises the full client → transport → session → backend round
//! trip (`docs/viewer.md` §1–§6).
//!
//! Every test observes its assertions through protocol messages (handshake,
//! frames, `input_ack`s) rather than fixed sleeps; a `tokio::time::timeout`
//! wraps anything that could otherwise park forever on a bug.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adesk_core::{
    ActionId, AppId, Button, ButtonState, Rect, Size, WindowId, WindowInfo, WindowState,
};
use adesk_proto::{ImagePayload, KeySpec, RendererKind};
use adesk_viewer::Result as ViewerResult;
use adesk_viewer::{
    ChangeSignal, ConnectOptions, PeerInfo, ViewerBackend, ViewerClient, ViewerError, ViewerInput,
    ViewerServer, ViewerTarget,
};
use adesk_viewer_proto::{
    ControlOwner, CursorState, DesktopState, KeyAction, ServerHello, ViewerFrame, PROTOCOL_VERSION,
};
use futures::pin_mut;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

/// The `ActionId` every fake input is recorded under.
const ACTION: ActionId = ActionId(11);

/// Generous bound for any step that depends on the peer making progress.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// A one-pixel opaque black RGBA8 frame body.
fn tiny_image() -> ImagePayload {
    ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0).expect("a valid 1x1 rgba8 payload")
}

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

/// A minimal [`ViewerBackend`] that records what the session forwards to it.
struct FakeBackend {
    /// The desktop-change source the test drives directly.
    change: ChangeSignal,
    /// Monotonic frame counter.
    seq: AtomicU64,
    /// Every [`ViewerInput`] the session applied, in submission order.
    inputs: Mutex<Vec<ViewerInput>>,
    /// Every [`ControlOwner`] the session announced, in submission order.
    control: Mutex<Vec<ControlOwner>>,
}

impl Default for FakeBackend {
    fn default() -> FakeBackend {
        FakeBackend {
            change: ChangeSignal::new(),
            seq: AtomicU64::new(0),
            inputs: Mutex::new(Vec::new()),
            control: Mutex::new(Vec::new()),
        }
    }
}

impl FakeBackend {
    /// A snapshot of the inputs applied so far.
    fn inputs(&self) -> Vec<ViewerInput> {
        self.inputs.lock().expect("inputs mutex").clone()
    }

    /// A snapshot of the control-owner announcements received so far.
    fn control(&self) -> Vec<ControlOwner> {
        self.control.lock().expect("control mutex").clone()
    }
}

#[async_trait::async_trait]
impl ViewerBackend for FakeBackend {
    fn display(&self) -> ServerHello {
        ServerHello {
            protocol_version: PROTOCOL_VERSION,
            runtime_version: "0.1.0".to_owned(),
            output: Size::new(800, 600),
            renderer: RendererKind::Pixman,
            cursor: CursorState::hidden(),
            control: ControlOwner::Ai,
        }
    }

    async fn render_frame(&self) -> ViewerResult<ViewerFrame> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ViewerFrame {
            seq,
            ts_ms: seq,
            image: tiny_image(),
            cursor: CursorState::hidden(),
            active_window_id: Some(WindowId(7)),
        })
    }

    async fn desktop_state(&self) -> ViewerResult<DesktopState> {
        Ok(desktop_state())
    }

    async fn apply_input(&self, input: ViewerInput) -> ViewerResult<Option<ActionId>> {
        self.inputs.lock().expect("inputs mutex").push(input);
        Ok(Some(ACTION))
    }

    fn change_signal(&self) -> ChangeSignal {
        self.change.clone()
    }

    async fn set_control(&self, owner: ControlOwner) -> ViewerResult<()> {
        self.control.lock().expect("control mutex").push(owner);
        Ok(())
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

/// Binds a Unix socket in a fresh temp dir and spawns an accept-and-serve task.
fn start_server() -> Harness {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("viewer.sock");
    let listener = UnixListener::bind(&path).expect("bind the unix socket");
    let backend = Arc::new(FakeBackend::default());
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
        harness.backend.inputs(),
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
    assert_eq!(harness.backend.control(), vec![ControlOwner::Human]);
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

#[tokio::test]
async fn close_succeeds_and_finishes_the_server_session() {
    let harness = start_server();
    let client = connect(&harness).await;

    // `close` performs the client half of the leave handshake and always
    // succeeds (a best-effort `bye` followed by tearing the socket down).
    tokio::time::timeout(STEP_TIMEOUT, client.close())
        .await
        .expect("close must not hang")
        .expect("close must succeed");

    // The session for that viewer ends rather than hanging. `close` shuts the
    // socket down as soon as it has sent its `bye`, so the session may observe
    // the departure either as the `bye` it answered with `Ok(())` or as a
    // transport error on that best-effort reply; both mean it terminated.
    let served = tokio::time::timeout(STEP_TIMEOUT, harness.serve)
        .await
        .expect("the server session must finish after the viewer leaves")
        .expect("the serve task must not panic");
    match served {
        Ok(()) => {}
        Err(ViewerError::Io(error)) => assert!(
            matches!(
                error.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
            ),
            "unexpected session failure: {error:?}"
        ),
        Err(other) => panic!("unexpected session failure: {other:?}"),
    }
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
