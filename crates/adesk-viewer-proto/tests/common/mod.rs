#![allow(dead_code)] // each test target uses a subset of the fixtures

//! Shared VAP v1 message fixtures for `adesk-viewer-proto` integration tests.
//!
//! **Not a test target**: each test file declares `mod common;` and pulls the
//! message instances it needs from here, so `tests/wire.rs` (golden JSON) and
//! `tests/codec.rs` (round-trips, unknown-field tolerance) share a single
//! authoritative copy of the duplicated literals.
//!
//! What it holds:
//!
//! - [`client`] / [`server`]: serialize a message through its `Serialize` impl
//!   (the local analog of `adesk-proto`'s `wire` helper),
//! - [`sample_window`]: the `WindowInfo` fixture (window 17, `org.mozilla.firefox`
//!   / "GitHub" / 1280x800),
//! - small named constructors for the message instances both test files build,
//! - [`every_client_message`] / [`every_server_message`]: the full round-trip
//!   corpora, assembled from those constructors (plus the corpus-only variants
//!   that no other test shares).
//!
//! There is no display, GPU, network or socket: these are pure values.

use adesk_core::{
    ActionId, AppId, Button, ButtonState, ErrorCode, LaunchId, OverlayKind, Rect, Size, WindowId,
    WindowInfo, WindowState,
};
use adesk_proto::{ImagePayload, KeySpec, RendererKind};
use adesk_viewer_proto::{
    AppEntry, ClientMessage, ControlOwner, CursorState, DesktopState, KeyAction, LaunchOutcome,
    RecordingEncoder, RecordingStatus, ServerHello, ServerMessage, ViewerFrame, ViewerHello,
    DEFAULT_RECORD_FPS,
};
use serde_json::{json, Value};

/// Serializes a client message through its `Serialize` impl.
pub fn client(message: &ClientMessage) -> Value {
    serde_json::to_value(message).expect("client message serializes")
}

/// Serializes a server message through its `Serialize` impl.
pub fn server(message: &ServerMessage) -> Value {
    serde_json::to_value(message).expect("server message serializes")
}

/// The shared `WindowInfo` fixture (window 17 / `org.mozilla.firefox` / 1280x800).
pub fn sample_window() -> WindowInfo {
    WindowInfo {
        id: WindowId(17),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        title: Some("GitHub".to_owned()),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: Some(4242),
        created_seq: 800,
        last_commit_seq: 8291,
        popup_count: 0,
    }
}

/// The shared `AppEntry` fixture (`org.mozilla.firefox` / "Firefox" / Network).
pub fn sample_app() -> AppEntry {
    AppEntry {
        id: AppId::from("org.mozilla.firefox"),
        name: "Firefox".to_owned(),
        icon: Some("firefox".to_owned()),
        categories: vec!["Network".to_owned()],
    }
}

// --- client fixtures (instances shared by wire.rs and codec.rs) ------------

/// `ClientMessage::Hello(ViewerHello::new())`.
pub fn hello_message() -> ClientMessage {
    ClientMessage::Hello(ViewerHello::new())
}

/// `ClientMessage::RequestFrame { id: Some(7) }`.
pub fn request_frame_with_id() -> ClientMessage {
    ClientMessage::RequestFrame { id: Some(7) }
}

/// `ClientMessage::RequestFrame { id: None }`.
pub fn request_frame_without_id() -> ClientMessage {
    ClientMessage::RequestFrame { id: None }
}

/// `ClientMessage::RequestState { id: Some(8) }`.
pub fn request_state_with_id() -> ClientMessage {
    ClientMessage::RequestState { id: Some(8) }
}

/// `ClientMessage::RequestState { id: None }`.
pub fn request_state_without_id() -> ClientMessage {
    ClientMessage::RequestState { id: None }
}

/// `ClientMessage::PointerMove { x: 0.42, y: 0.51 }`.
pub fn pointer_move_message() -> ClientMessage {
    ClientMessage::PointerMove { x: 0.42, y: 0.51 }
}

/// `ClientMessage::PointerButton { Left, Pressed, None, None }`.
pub fn pointer_button_pressed() -> ClientMessage {
    ClientMessage::PointerButton {
        button: Button::Left,
        state: ButtonState::Pressed,
        x: None,
        y: None,
    }
}

/// `ClientMessage::Scroll { dx: 0.0, dy: -3.0, x: None, y: None }`.
pub fn scroll_message() -> ClientMessage {
    ClientMessage::Scroll {
        dx: 0.0,
        dy: -3.0,
        x: None,
        y: None,
    }
}

/// `ClientMessage::Key { keys: Single("a"), action: Tap }`.
pub fn key_tap_message() -> ClientMessage {
    ClientMessage::Key {
        keys: KeySpec::Single("a".to_owned()),
        action: KeyAction::Tap,
    }
}

/// `ClientMessage::Text { text: "hello" }`.
pub fn text_message() -> ClientMessage {
    ClientMessage::Text {
        text: "hello".to_owned(),
    }
}

/// `ClientMessage::ActivateWindow { window_id: 17 }`.
pub fn activate_window_message() -> ClientMessage {
    ClientMessage::ActivateWindow {
        window_id: WindowId(17),
    }
}
/// `ClientMessage::SetControl { owner: Human }`.
pub fn set_control_message() -> ClientMessage {
    ClientMessage::SetControl {
        owner: ControlOwner::Human,
    }
}

/// `ClientMessage::Bye { reason: Some("done") }`.
pub fn bye_message() -> ClientMessage {
    ClientMessage::Bye {
        reason: Some("done".to_owned()),
    }
}

/// `ClientMessage::Bye { reason: None }`.
pub fn bye_without_reason() -> ClientMessage {
    ClientMessage::Bye { reason: None }
}

/// `ClientMessage::StartRecording { id: 9, path: "/tmp/rec.webm", fps: 24, Software }`.
pub fn start_recording_message() -> ClientMessage {
    ClientMessage::StartRecording {
        id: Some(9),
        path: Some("/tmp/rec.webm".to_owned()),
        fps: 24,
        encoder: RecordingEncoder::Software,
    }
}

/// `ClientMessage::StartRecording` with no optional fields (the decode defaults).
pub fn start_recording_minimal() -> ClientMessage {
    ClientMessage::StartRecording {
        id: None,
        path: None,
        fps: DEFAULT_RECORD_FPS,
        encoder: RecordingEncoder::Auto,
    }
}

/// `ClientMessage::StopRecording { id: Some(10) }`.
pub fn stop_recording_message() -> ClientMessage {
    ClientMessage::StopRecording { id: Some(10) }
}

/// `ClientMessage::RequestRecording { id: Some(11) }`.
pub fn request_recording_message() -> ClientMessage {
    ClientMessage::RequestRecording { id: Some(11) }
}

/// `ClientMessage::ListApps { id: Some(1), query: Some("fire") }`.
pub fn list_apps_message() -> ClientMessage {
    ClientMessage::ListApps {
        id: Some(1),
        query: Some("fire".to_owned()),
    }
}

/// `ClientMessage::ListApps { id: None, query: None }` (list everything).
pub fn list_apps_unfiltered() -> ClientMessage {
    ClientMessage::ListApps {
        id: None,
        query: None,
    }
}

/// `ClientMessage::LaunchApp { id: Some(2), app_id: "org.example.Editor" }`.
pub fn launch_app_message() -> ClientMessage {
    ClientMessage::LaunchApp {
        id: Some(2),
        app_id: AppId::from("org.example.Editor"),
    }
}

/// `ClientMessage::LaunchApp { id: None, app_id: "org.example.Editor" }`.
pub fn launch_app_without_id() -> ClientMessage {
    ClientMessage::LaunchApp {
        id: None,
        app_id: AppId::from("org.example.Editor"),
    }
}

/// `ClientMessage::CloseWindow { window_id: 17 }`.
pub fn close_window_message() -> ClientMessage {
    ClientMessage::CloseWindow {
        window_id: WindowId(17),
    }
}

/// `ClientMessage::Unknown { "future_thing", {"type": "future_thing", "x": 1} }`.
pub fn unknown_client_message() -> ClientMessage {
    ClientMessage::Unknown {
        message_type: "future_thing".to_owned(),
        value: json!({"type": "future_thing", "x": 1}),
    }
}

// --- server fixtures (instances shared by wire.rs and codec.rs) ------------

/// `ServerMessage::Hello { Pixman, 1280x800, cursor hidden, control Ai }`.
pub fn server_hello_message() -> ServerMessage {
    ServerMessage::Hello(ServerHello {
        protocol_version: 1,
        runtime_version: "0.1.0".to_owned(),
        output: Size::new(1280, 800),
        renderer: RendererKind::Pixman,
        cursor: CursorState::hidden(),
        control: ControlOwner::Ai,
    })
}

/// `ServerMessage::Frame { seq 8291, png 1x1, cursor at (0.42, 0.51), window 17 }`.
pub fn frame_message() -> ServerMessage {
    ServerMessage::Frame(ViewerFrame {
        seq: 8291,
        ts_ms: 51234,
        image: ImagePayload::from_png(1, 1, b"x", 1.0),
        cursor: CursorState::at(0.42, 0.51),
        active_window_id: Some(WindowId(17)),
    })
}

/// `ServerMessage::State { active window 17, [sample_window()] }`.
pub fn state_with_window() -> ServerMessage {
    ServerMessage::State(DesktopState {
        active_window_id: Some(WindowId(17)),
        windows: vec![sample_window()],
    })
}

/// `ServerMessage::State { active_window_id: None, windows: [] }`.
pub fn state_empty() -> ServerMessage {
    ServerMessage::State(DesktopState {
        active_window_id: None,
        windows: Vec::new(),
    })
}

/// `ServerMessage::Control { owner: Human }`.
pub fn control_message() -> ServerMessage {
    ServerMessage::Control {
        owner: ControlOwner::Human,
    }
}

/// `ServerMessage::InputAck { id: Some(3), action_id: 582 }`.
pub fn input_ack_with_id() -> ServerMessage {
    ServerMessage::InputAck {
        id: Some(3),
        action_id: ActionId(582),
    }
}

/// `ServerMessage::InputAck { id: None, action_id: 582 }`.
pub fn input_ack_without_id() -> ServerMessage {
    ServerMessage::InputAck {
        id: None,
        action_id: ActionId(582),
    }
}

/// `ServerMessage::Error { UnknownWindow, "no such window", id: Some(3) }`.
pub fn error_message() -> ServerMessage {
    ServerMessage::Error {
        code: ErrorCode::UnknownWindow,
        message: "no such window".to_owned(),
        id: Some(3),
    }
}

/// `ServerMessage::Bye { reason: "shutdown" }`.
pub fn bye_server_message() -> ServerMessage {
    ServerMessage::Bye {
        reason: "shutdown".to_owned(),
    }
}

/// `ServerMessage::Unknown { "future_thing", {"type": "future_thing", "y": true} }`.
pub fn unknown_server_message() -> ServerMessage {
    ServerMessage::Unknown {
        message_type: "future_thing".to_owned(),
        value: json!({"type": "future_thing", "y": true}),
    }
}

/// `ServerMessage::Recording { id: 9, status: active, 120 frames / 4000 ms }`.
pub fn recording_message() -> ServerMessage {
    ServerMessage::Recording {
        id: Some(9),
        status: RecordingStatus::idle()
            .with_recording(true)
            .with_path("/tmp/rec.webm".to_owned())
            .with_encoder("h264".to_owned())
            .with_counts(120, 4000),
    }
}

/// `ServerMessage::Recording { id: None, status: idle }`.
pub fn recording_idle() -> ServerMessage {
    ServerMessage::Recording {
        id: None,
        status: RecordingStatus::idle(),
    }
}

/// `ServerMessage::Apps { id: Some(1), apps: [sample_app()] }`.
pub fn apps_message() -> ServerMessage {
    ServerMessage::Apps {
        id: Some(1),
        apps: vec![sample_app()],
    }
}

/// `ServerMessage::Apps { id: None, apps: [] }`.
pub fn apps_empty() -> ServerMessage {
    ServerMessage::Apps {
        id: None,
        apps: Vec::new(),
    }
}

/// `ServerMessage::LaunchResult { id: Some(2), launch 3 for "org.example.Editor",
/// action 582, window 17 }`.
pub fn launch_result_message() -> ServerMessage {
    ServerMessage::LaunchResult {
        id: Some(2),
        result: LaunchOutcome {
            app_id: AppId::from("org.example.Editor"),
            launch_id: LaunchId(3),
            action_id: Some(ActionId(582)),
            window_id: Some(WindowId(17)),
        },
    }
}

/// `ServerMessage::LaunchResult { id: None, launch 3, no action/window yet }`.
pub fn launch_result_pending() -> ServerMessage {
    ServerMessage::LaunchResult {
        id: None,
        result: LaunchOutcome {
            app_id: AppId::from("org.example.Editor"),
            launch_id: LaunchId(3),
            action_id: None,
            window_id: None,
        },
    }
}

// --- corpora (round-trip / acceptance fixtures) ---------------------------

/// Every `ClientMessage` variant the codec must round-trip.
pub fn every_client_message() -> Vec<ClientMessage> {
    vec![
        hello_message(),
        ClientMessage::Hello(ViewerHello {
            protocol_version: 1,
            client: Some("adesk-viewer".to_owned()),
            overlays: vec![OverlayKind::WindowIds],
            min_interval_ms: 0,
        }),
        request_frame_with_id(),
        request_frame_without_id(),
        request_state_with_id(),
        request_state_without_id(),
        pointer_move_message(),
        pointer_button_pressed(),
        ClientMessage::PointerButton {
            button: Button::Side,
            state: ButtonState::Released,
            x: Some(0.1),
            y: Some(0.9),
        },
        scroll_message(),
        ClientMessage::Scroll {
            dx: 1.5,
            dy: 2.5,
            x: Some(0.5),
            y: Some(0.5),
        },
        key_tap_message(),
        ClientMessage::Key {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            action: KeyAction::Released,
        },
        text_message(),
        set_control_message(),
        bye_message(),
        bye_without_reason(),
        start_recording_message(),
        start_recording_minimal(),
        ClientMessage::StartRecording {
            id: None,
            path: Some("/srv/cap.mkv".to_owned()),
            fps: 60,
            encoder: RecordingEncoder::Gpu,
        },
        stop_recording_message(),
        ClientMessage::StopRecording { id: None },
        request_recording_message(),
        ClientMessage::RequestRecording { id: None },
        list_apps_message(),
        list_apps_unfiltered(),
        launch_app_message(),
        launch_app_without_id(),
        close_window_message(),
        unknown_client_message(),
    ]
}

/// Every `ServerMessage` variant the codec must round-trip.
pub fn every_server_message() -> Vec<ServerMessage> {
    vec![
        server_hello_message(),
        frame_message(),
        ServerMessage::Frame(ViewerFrame {
            seq: 1,
            ts_ms: 2,
            image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 0.5).unwrap(),
            cursor: CursorState::hidden(),
            active_window_id: None,
        }),
        state_with_window(),
        state_empty(),
        control_message(),
        input_ack_with_id(),
        input_ack_without_id(),
        error_message(),
        ServerMessage::Error {
            code: ErrorCode::RenderFailed,
            message: "boom".to_owned(),
            id: None,
        },
        bye_server_message(),
        recording_message(),
        recording_idle(),
        ServerMessage::Recording {
            id: Some(4),
            status: RecordingStatus::idle().with_error("encoder unavailable".to_owned()),
        },
        apps_message(),
        apps_empty(),
        launch_result_message(),
        launch_result_pending(),
        unknown_server_message(),
    ]
}
