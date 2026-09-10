//! Codec acceptance tests (`docs/viewer.md` §1, §2): round-trips, unknown
//! `"type"` handling, unknown-field tolerance and malformed input. No display,
//! GPU, network or socket.

use adesk_core::{
    ActionId, AppId, Button, ButtonState, ErrorCode, WindowId, WindowInfo, WindowState,
};
use adesk_proto::{ImagePayload, KeySpec};
use adesk_viewer_proto::{
    check_version, decode_client, decode_server, encode_client, encode_server, ClientMessage,
    ControlOwner, CursorState, DesktopState, KeyAction, ServerHello, ServerMessage, ViewerFrame,
    ViewerHello, ViewerProtoError,
};
use serde_json::{json, Value};

fn every_client_message() -> Vec<ClientMessage> {
    vec![
        ClientMessage::Hello(ViewerHello::new()),
        ClientMessage::Hello(ViewerHello {
            protocol_version: 1,
            client: Some("adesk-viewer".to_owned()),
            overlays: vec![adesk_core::OverlayKind::WindowIds],
            min_interval_ms: 0,
        }),
        ClientMessage::RequestFrame { id: Some(7) },
        ClientMessage::RequestFrame { id: None },
        ClientMessage::RequestState { id: Some(8) },
        ClientMessage::RequestState { id: None },
        ClientMessage::PointerMove { x: 0.42, y: 0.51 },
        ClientMessage::PointerButton {
            button: Button::Left,
            state: ButtonState::Pressed,
            x: None,
            y: None,
        },
        ClientMessage::PointerButton {
            button: Button::Side,
            state: ButtonState::Released,
            x: Some(0.1),
            y: Some(0.9),
        },
        ClientMessage::Scroll {
            dx: 0.0,
            dy: -3.0,
            x: None,
            y: None,
        },
        ClientMessage::Scroll {
            dx: 1.5,
            dy: 2.5,
            x: Some(0.5),
            y: Some(0.5),
        },
        ClientMessage::Key {
            keys: KeySpec::Single("a".to_owned()),
            action: KeyAction::Tap,
        },
        ClientMessage::Key {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            action: KeyAction::Released,
        },
        ClientMessage::Text {
            text: "hello".to_owned(),
        },
        ClientMessage::SetControl {
            owner: ControlOwner::Human,
        },
        ClientMessage::Bye {
            reason: Some("done".to_owned()),
        },
        ClientMessage::Bye { reason: None },
        ClientMessage::Unknown {
            message_type: "future_thing".to_owned(),
            value: json!({"type": "future_thing", "x": 1}),
        },
    ]
}

fn every_server_message() -> Vec<ServerMessage> {
    let window = WindowInfo {
        id: WindowId(17),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        title: Some("GitHub".to_owned()),
        geometry: adesk_core::Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: Some(4242),
        created_seq: 800,
        last_commit_seq: 8291,
        popup_count: 0,
    };
    vec![
        ServerMessage::Hello(ServerHello {
            protocol_version: 1,
            runtime_version: "0.1.0".to_owned(),
            output: adesk_core::Size::new(1280, 800),
            renderer: adesk_proto::RendererKind::Pixman,
            cursor: CursorState::hidden(),
            control: ControlOwner::Ai,
        }),
        ServerMessage::Frame(ViewerFrame {
            seq: 8291,
            ts_ms: 51234,
            image: ImagePayload::from_png(1, 1, b"x", 1.0),
            cursor: CursorState::at(0.42, 0.51),
            active_window_id: Some(WindowId(17)),
        }),
        ServerMessage::Frame(ViewerFrame {
            seq: 1,
            ts_ms: 2,
            image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 0.5).unwrap(),
            cursor: CursorState::hidden(),
            active_window_id: None,
        }),
        ServerMessage::State(DesktopState {
            active_window_id: Some(WindowId(17)),
            windows: vec![window],
        }),
        ServerMessage::State(DesktopState {
            active_window_id: None,
            windows: Vec::new(),
        }),
        ServerMessage::Control {
            owner: ControlOwner::Human,
        },
        ServerMessage::InputAck {
            id: Some(3),
            action_id: ActionId(582),
        },
        ServerMessage::InputAck {
            id: None,
            action_id: ActionId(582),
        },
        ServerMessage::Error {
            code: ErrorCode::UnknownWindow,
            message: "no such window".to_owned(),
            id: Some(3),
        },
        ServerMessage::Error {
            code: ErrorCode::RenderFailed,
            message: "boom".to_owned(),
            id: None,
        },
        ServerMessage::Bye {
            reason: "shutdown".to_owned(),
        },
        ServerMessage::Unknown {
            message_type: "future_thing".to_owned(),
            value: json!({"type": "future_thing", "y": true}),
        },
    ]
}

// --- round-trips ----------------------------------------------------------

#[test]
fn client_messages_round_trip() {
    for message in every_client_message() {
        let line = encode_client(&message);
        let decoded =
            decode_client(&line).unwrap_or_else(|error| panic!("decoding {line} failed: {error}"));
        assert_eq!(decoded, message, "round-trip of {line}");
    }
}

#[test]
fn server_messages_round_trip() {
    for message in every_server_message() {
        let line = encode_server(&message);
        let decoded =
            decode_server(&line).unwrap_or_else(|error| panic!("decoding {line} failed: {error}"));
        assert_eq!(decoded, message, "round-trip of {line}");
    }
}

#[test]
fn encoded_lines_are_single_line_json_objects() {
    for message in every_client_message() {
        let line = encode_client(&message);
        assert!(!line.contains('\n'), "no embedded newline: {line}");
        assert!(!line.ends_with('\n'), "no trailing newline: {line}");
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "object: {line}"
        );
        assert!(serde_json::from_str::<Value>(&line).is_ok());
    }
    for message in every_server_message() {
        let line = encode_server(&message);
        assert!(!line.contains('\n'), "no embedded newline: {line}");
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "object: {line}"
        );
    }
}

#[test]
fn serde_round_trips_through_the_message_impls() {
    for message in every_client_message() {
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            message
        );
    }
    for message in every_server_message() {
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            message
        );
    }
}

// --- unknown message types ------------------------------------------------

#[test]
fn unknown_client_type_becomes_the_unknown_variant() {
    let value = json!({"type": "future_thing", "payload": [1, 2]});
    let decoded = decode_client(&value.to_string()).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Unknown {
            message_type: "future_thing".to_owned(),
            value: value.clone(),
        }
    );
    assert_eq!(decoded.message_type(), "future_thing");
}

#[test]
fn unknown_server_type_becomes_the_unknown_variant() {
    let value = json!({"type": "future_thing", "ok": true});
    let decoded = decode_server(&value.to_string()).unwrap();
    assert_eq!(
        decoded,
        ServerMessage::Unknown {
            message_type: "future_thing".to_owned(),
            value: value.clone(),
        }
    );
    assert_eq!(decoded.message_type(), "future_thing");
}

#[test]
fn a_server_only_type_is_unknown_to_the_client_decoder() {
    let decoded = decode_client(r#"{"type": "frame", "seq": 1}"#).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Unknown {
            message_type: "frame".to_owned(),
            value: json!({"type": "frame", "seq": 1}),
        }
    );
}

#[test]
fn a_client_only_type_is_unknown_to_the_server_decoder() {
    let decoded = decode_server(r#"{"type": "pointer_move", "x": 0.0, "y": 0.0}"#).unwrap();
    assert_eq!(
        decoded,
        ServerMessage::Unknown {
            message_type: "pointer_move".to_owned(),
            value: json!({"type": "pointer_move", "x": 0.0, "y": 0.0}),
        }
    );
}

#[test]
fn an_unknown_variant_echoes_its_original_object() {
    let decoded = decode_client(r#"{"type": "future_thing", "x": 1}"#).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&encode_client(&decoded)).unwrap(),
        json!({"type": "future_thing", "x": 1})
    );
}

// --- unknown fields are ignored ------------------------------------------

#[test]
fn unknown_fields_in_a_known_message_are_ignored() {
    let decoded =
        decode_client(r#"{"type": "pointer_move", "x": 0.5, "y": 0.25, "future": true}"#).unwrap();
    assert_eq!(decoded, ClientMessage::PointerMove { x: 0.5, y: 0.25 });

    let decoded =
        decode_client(r#"{"type": "key", "keys": "a", "state": "tap", "extra": {"nested": 1}}"#)
            .unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Key {
            keys: KeySpec::Single("a".to_owned()),
            action: KeyAction::Tap,
        }
    );

    let decoded =
        decode_client(r#"{"type": "bye", "reason": "done", "unknown_extra": 7}"#).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Bye {
            reason: Some("done".to_owned()),
        }
    );
}

#[test]
fn unknown_fields_in_nested_payloads_are_ignored() {
    // Unknown fields are ignored at the message level and inside the nested
    // `hello`/`window` payloads (forward compatibility, §1).
    let line = r#"{"type": "hello", "protocol_version": 1, "client": null,
        "overlays": [], "min_interval_ms": 0, "future": 1}"#;
    let decoded = decode_client(line).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Hello(ViewerHello {
            protocol_version: 1,
            client: None,
            overlays: Vec::new(),
            min_interval_ms: 0,
        })
    );

    let line = r#"{"type": "state", "active_window_id": null, "windows": [
        {"id": 1, "app_id": null, "title": null, "geometry": {"x": 0, "y": 0, "w": 1, "h": 1},
         "state": "inactive", "mapped": false, "pid": null, "created_seq": 0,
         "last_commit_seq": 0, "popup_count": 0, "future_field": "ignored"}]}"#;
    let decoded = decode_server(line).unwrap();
    let ServerMessage::State(state) = decoded else {
        panic!("expected a state message");
    };
    assert_eq!(state.windows.len(), 1);
    assert_eq!(state.windows[0].id, WindowId(1));
}

// --- malformed input ------------------------------------------------------

#[test]
fn malformed_lines_are_rejected() {
    let cases = [
        "",
        "   ",
        "not json",
        "{\"type\":\"text\"",              // unterminated
        "[1, 2]",                          // not an object
        "\"text\"",                        // not an object
        "42",                              // not an object
        "{}",                              // missing type
        r#"{"x": 1}"#,                     // missing type
        r#"{"type": 5}"#,                  // non-string type
        r#"{"type": null}"#,               // non-string type
        r#"{"type": "key"}"#,              // known tag, missing keys
        r#"{"type": "pointer_button"}"#,   // known tag, missing fields
        r#"{"type": "bye", "reason": 5}"#, // wrong field type
    ];
    for line in cases {
        assert!(
            matches!(decode_client(line), Err(ViewerProtoError::Malformed)),
            "client must reject {line:?}"
        );
    }

    let server_cases = [
        "",
        "[1, 2]",
        "{}",
        r#"{"type": 5}"#,
        r#"{"type": "frame"}"#,     // known tag, missing fields
        r#"{"type": "input_ack"}"#, // known tag, missing fields
        r#"{"type": "error"}"#,     // known tag, missing fields
        r#"{"type": "control", "owner": "nobody"}"#, // unknown enum value
    ];
    for line in server_cases {
        assert!(
            matches!(decode_server(line), Err(ViewerProtoError::Malformed)),
            "server must reject {line:?}"
        );
    }
}

#[test]
fn malformed_input_maps_to_invalid_request() {
    let error = decode_client("nope").unwrap_err();
    assert_eq!(error.error_code(), ErrorCode::InvalidRequest);
    assert_eq!(
        ViewerProtoError::Unknown {
            message_type: "x".to_owned(),
        }
        .error_code(),
        ErrorCode::InvalidRequest
    );
}

// --- version mismatch -----------------------------------------------------

#[test]
fn version_mismatch_maps_to_protocol_version_mismatch() {
    let error = check_version(2).unwrap_err();
    assert_eq!(error.error_code(), ErrorCode::ProtocolVersionMismatch);
    assert_eq!(
        error.to_string(),
        "protocol version mismatch: this build speaks v1, peer sent v2"
    );
    // A well-formed hello from a future version still decodes; version checking
    // is the connection layer's job (§2).
    let decoded = decode_client(
        r#"{"type": "hello", "protocol_version": 2,
        "client": null, "overlays": [], "min_interval_ms": 0}"#,
    )
    .unwrap();
    assert_eq!(decoded.message_type(), "hello");
}
