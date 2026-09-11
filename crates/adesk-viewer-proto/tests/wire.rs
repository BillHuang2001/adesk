//! Golden-JSON and version-helper tests for the VAP v1 type + message layer
//! (`docs/viewer.md` §2–§4). No display, GPU, network or socket.

mod common;

use adesk_core::{Button, ButtonState, ErrorCode, OverlayKind, Size, WindowId};
use adesk_proto::{ImageFormat, ImagePayload, KeySpec, RendererKind};
use adesk_viewer_proto::{
    check_version, is_compatible_version, ClientMessage, ControlOwner, CursorState, KeyAction,
    ServerHello, ServerMessage, ViewerFrame, ViewerHello, ViewerProtoError,
    DEFAULT_MIN_INTERVAL_MS, DEFAULT_OVERLAYS, PROTOCOL_VERSION,
};
use common::{
    activate_window_message, bye_message, bye_server_message, bye_without_reason, client,
    control_message, error_message, frame_message, hello_message, input_ack_with_id,
    input_ack_without_id, key_tap_message, pointer_button_pressed, pointer_move_message,
    request_frame_with_id, request_frame_without_id, request_state_with_id, request_state_without_id,
    scroll_message, server, server_hello_message, set_control_message, state_empty,
    state_with_window, text_message, unknown_client_message, unknown_server_message,
};
use serde_json::{json, Value};

// --- constants and version helpers ---------------------------------------

#[test]
fn protocol_version_is_one_and_checked() {
    assert_eq!(PROTOCOL_VERSION, 1);
    assert!(is_compatible_version(1));
    assert!(!is_compatible_version(0));
    assert!(!is_compatible_version(2));
    assert!(check_version(1).is_ok());
    let error = check_version(2).expect_err("v2 is a mismatch");
    assert!(matches!(
        error,
        ViewerProtoError::VersionMismatch {
            client: 2,
            server: 1,
        }
    ));
    assert_eq!(error.error_code(), ErrorCode::ProtocolVersionMismatch);
}

#[test]
fn defaults_match_the_spec() {
    assert_eq!(DEFAULT_MIN_INTERVAL_MS, 100);
    assert_eq!(
        DEFAULT_OVERLAYS,
        &[
            OverlayKind::WindowIds,
            OverlayKind::Focus,
            OverlayKind::Damage
        ]
    );
    assert_eq!(
        serde_json::to_value(DEFAULT_OVERLAYS).unwrap(),
        json!(["window_ids", "focus", "damage"])
    );
}

#[test]
fn viewer_hello_new_fills_the_defaults() {
    let hello = ViewerHello::new();
    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
    assert_eq!(hello.client, None);
    assert_eq!(hello.overlays, DEFAULT_OVERLAYS.to_vec());
    assert_eq!(hello.min_interval_ms, DEFAULT_MIN_INTERVAL_MS);
    assert_eq!(ViewerHello::default(), hello);
}

// --- client messages ------------------------------------------------------

#[test]
fn client_hello_golden() {
    let hello = ViewerHello {
        protocol_version: PROTOCOL_VERSION,
        client: Some("adesk-viewer".to_owned()),
        overlays: vec![
            OverlayKind::WindowIds,
            OverlayKind::Focus,
            OverlayKind::Damage,
        ],
        min_interval_ms: 100,
    };
    assert_eq!(
        client(&ClientMessage::Hello(hello)),
        json!({
            "type": "hello",
            "protocol_version": 1,
            "client": "adesk-viewer",
            "overlays": ["window_ids", "focus", "damage"],
            "min_interval_ms": 100
        })
    );
}

#[test]
fn client_request_frame_and_state_golden() {
    assert_eq!(
        client(&request_frame_with_id()),
        json!({"type": "request_frame", "id": 7})
    );
    assert_eq!(
        client(&request_frame_without_id()),
        json!({"type": "request_frame"})
    );
    assert_eq!(
        client(&request_state_with_id()),
        json!({"type": "request_state", "id": 8})
    );
    assert_eq!(
        client(&request_state_without_id()),
        json!({"type": "request_state"})
    );
}

#[test]
fn client_pointer_move_golden() {
    assert_eq!(
        client(&pointer_move_message()),
        json!({"type": "pointer_move", "x": 0.42, "y": 0.51})
    );
}

#[test]
fn client_pointer_button_golden() {
    assert_eq!(
        client(&pointer_button_pressed()),
        json!({"type": "pointer_button", "button": "left", "state": "pressed"})
    );
    assert_eq!(
        client(&ClientMessage::PointerButton {
            button: Button::Right,
            state: ButtonState::Released,
            x: Some(0.1),
            y: Some(0.2),
        }),
        json!({
            "type": "pointer_button",
            "button": "right",
            "state": "released",
            "x": 0.1,
            "y": 0.2
        })
    );
}

#[test]
fn client_scroll_golden() {
    assert_eq!(
        client(&scroll_message()),
        json!({"type": "scroll", "dx": 0.0, "dy": -3.0})
    );
}

#[test]
fn client_key_uses_the_state_wire_name() {
    assert_eq!(
        client(&key_tap_message()),
        json!({"type": "key", "keys": "a", "state": "tap"})
    );
    assert_eq!(
        client(&ClientMessage::Key {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            action: KeyAction::Pressed,
        }),
        json!({"type": "key", "keys": ["CTRL", "L"], "state": "pressed"})
    );
}

#[test]
fn client_text_set_control_and_bye_golden() {
    assert_eq!(
        client(&text_message()),
        json!({"type": "text", "text": "hello"})
    );
    assert_eq!(
        client(&set_control_message()),
        json!({"type": "set_control", "owner": "human"})
    );
    assert_eq!(
        client(&bye_message()),
        json!({"type": "bye", "reason": "done"})
    );
    assert_eq!(client(&bye_without_reason()), json!({"type": "bye"}));
}

#[test]
fn client_activate_window_golden() {
    // A flat object with exactly the `type` tag and the one `window_id` field.
    assert_eq!(
        client(&activate_window_message()),
        json!({"type": "activate_window", "window_id": 17})
    );
    let value = client(&ClientMessage::ActivateWindow {
        window_id: WindowId(4242),
    });
    assert_eq!(value, json!({"type": "activate_window", "window_id": 4242}));
    assert!(value.get("id").is_none(), "activate_window carries no `id`");
}

#[test]
fn client_unknown_is_emitted_verbatim() {
    let value = json!({"type": "future_thing", "x": 1});
    assert_eq!(client(&unknown_client_message()), value);
}

#[test]
fn client_message_type_tags() {
    let cases = [
        (hello_message(), "hello"),
        (request_frame_without_id(), "request_frame"),
        (request_state_without_id(), "request_state"),
        (
            ClientMessage::PointerMove { x: 0.0, y: 0.0 },
            "pointer_move",
        ),
        (pointer_button_pressed(), "pointer_button"),
        (
            ClientMessage::Scroll {
                dx: 0.0,
                dy: 0.0,
                x: None,
                y: None,
            },
            "scroll",
        ),
        (key_tap_message(), "key"),
        (
            ClientMessage::Text {
                text: String::new(),
            },
            "text",
        ),
        (activate_window_message(), "activate_window"),
        (
            ClientMessage::SetControl {
                owner: ControlOwner::Ai,
            },
            "set_control",
        ),
        (bye_without_reason(), "bye"),
        (
            ClientMessage::Unknown {
                message_type: "mystery".to_owned(),
                value: json!({"type": "mystery"}),
            },
            "mystery",
        ),
    ];
    for (message, tag) in cases {
        assert_eq!(message.message_type(), tag);
        assert_eq!(
            client(&message).get("type").and_then(Value::as_str),
            Some(tag),
            "{tag} serializes its tag"
        );
    }
}

// --- server messages ------------------------------------------------------

#[test]
fn server_hello_golden() {
    assert_eq!(
        server(&server_hello_message()),
        json!({
            "type": "hello",
            "protocol_version": 1,
            "runtime_version": "0.1.0",
            "output": {"w": 1280, "h": 800},
            "renderer": "pixman",
            "cursor": {"x": 0.0, "y": 0.0, "visible": false},
            "control": "ai"
        })
    );
}

#[test]
fn server_frame_golden() {
    assert_eq!(
        server(&frame_message()),
        json!({
            "type": "frame",
            "seq": 8291,
            "ts_ms": 51234,
            "image": {
                "width": 1,
                "height": 1,
                "format": "png",
                "stride": null,
                "data": "eA==",
                "scale": 1.0
            },
            "cursor": {"x": 0.42, "y": 0.51, "visible": true},
            "active_window_id": 17
        })
    );
    // No active window is an explicit `null`.
    let idle = ViewerFrame {
        seq: 1,
        ts_ms: 2,
        image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0).unwrap(),
        cursor: CursorState::hidden(),
        active_window_id: None,
    };
    let value = server(&ServerMessage::Frame(idle));
    assert_eq!(value["active_window_id"], Value::Null);
    assert_eq!(value["image"]["format"], json!("rgba8"));
    assert_eq!(value["image"]["stride"], json!(4));
}

#[test]
fn server_state_golden() {
    assert_eq!(
        server(&state_with_window()),
        json!({
            "type": "state",
            "active_window_id": 17,
            "windows": [{
                "id": 17,
                "app_id": "org.mozilla.firefox",
                "title": "GitHub",
                "geometry": {"x": 0, "y": 0, "w": 1280, "h": 800},
                "state": "active",
                "mapped": true,
                "pid": 4242,
                "created_seq": 800,
                "last_commit_seq": 8291,
                "popup_count": 0
            }]
        })
    );
}

#[test]
fn server_control_input_ack_error_bye_golden() {
    assert_eq!(
        server(&control_message()),
        json!({"type": "control", "owner": "human"})
    );
    assert_eq!(
        server(&input_ack_with_id()),
        json!({"type": "input_ack", "id": 3, "action_id": 582})
    );
    assert_eq!(
        server(&input_ack_without_id()),
        json!({"type": "input_ack", "action_id": 582})
    );
    assert_eq!(
        server(&error_message()),
        json!({
            "type": "error",
            "code": "unknown_window",
            "message": "no such window",
            "id": 3
        })
    );
    assert_eq!(
        server(&bye_server_message()),
        json!({"type": "bye", "reason": "shutdown"})
    );
}

#[test]
fn server_unknown_is_emitted_verbatim() {
    let value = json!({"type": "future_thing", "y": true});
    assert_eq!(server(&unknown_server_message()), value);
}

#[test]
fn server_message_type_tags() {
    let state = state_empty();
    let frame = ServerMessage::Frame(ViewerFrame {
        seq: 1,
        ts_ms: 2,
        image: ImagePayload::from_png(1, 1, &[], 1.0),
        cursor: CursorState::hidden(),
        active_window_id: None,
    });
    let cases = [
        (
            ServerMessage::Hello(ServerHello {
                protocol_version: 1,
                runtime_version: "0.1.0".to_owned(),
                output: Size::new(1, 1),
                renderer: RendererKind::Gl,
                cursor: CursorState::hidden(),
                control: ControlOwner::Ai,
            }),
            "hello",
        ),
        (frame, "frame"),
        (state, "state"),
        (
            ServerMessage::Control {
                owner: ControlOwner::Ai,
            },
            "control",
        ),
        (
            ServerMessage::InputAck {
                id: None,
                action_id: adesk_core::ActionId(1),
            },
            "input_ack",
        ),
        (
            ServerMessage::Error {
                code: ErrorCode::Internal,
                message: "x".to_owned(),
                id: None,
            },
            "error",
        ),
        (
            ServerMessage::Bye {
                reason: "x".to_owned(),
            },
            "bye",
        ),
        (
            ServerMessage::Unknown {
                message_type: "mystery".to_owned(),
                value: json!({"type": "mystery"}),
            },
            "mystery",
        ),
    ];
    for (message, tag) in cases {
        assert_eq!(message.message_type(), tag);
        assert_eq!(
            server(&message).get("type").and_then(Value::as_str),
            Some(tag),
            "{tag} serializes its tag"
        );
    }
}

// --- vocabulary wire names ------------------------------------------------

#[test]
fn control_owner_and_key_action_wire_names() {
    assert_eq!(serde_json::to_value(ControlOwner::Ai).unwrap(), json!("ai"));
    assert_eq!(
        serde_json::to_value(ControlOwner::Human).unwrap(),
        json!("human")
    );
    assert_eq!(
        serde_json::to_value(KeyAction::Pressed).unwrap(),
        json!("pressed")
    );
    assert_eq!(
        serde_json::to_value(KeyAction::Released).unwrap(),
        json!("released")
    );
    assert_eq!(serde_json::to_value(KeyAction::Tap).unwrap(), json!("tap"));
}

#[test]
fn cursor_state_constructors() {
    assert_eq!(
        CursorState::hidden(),
        CursorState {
            x: 0.0,
            y: 0.0,
            visible: false
        }
    );
    assert_eq!(
        CursorState::at(0.5, 0.25),
        CursorState {
            x: 0.5,
            y: 0.25,
            visible: true
        }
    );
}

#[test]
fn renderer_kind_reuses_the_agp_vocabulary() {
    assert_eq!(
        serde_json::to_value(RendererKind::Pixman).unwrap(),
        json!("pixman")
    );
    assert_eq!(
        serde_json::to_value(ImageFormat::Png).unwrap(),
        json!("png")
    );
}
