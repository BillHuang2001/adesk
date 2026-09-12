//! Codec acceptance tests (`docs/viewer.md` §1, §2): round-trips, unknown
//! `"type"` handling, unknown-field tolerance and malformed input. No display,
//! GPU, network or socket.

mod common;

use adesk_core::{ErrorCode, WindowId};
use adesk_proto::KeySpec;
use adesk_viewer_proto::{
    check_version, decode_client, decode_server, encode_client, encode_server, ClientMessage,
    KeyAction, RecordingEncoder, RecordingStatus, ServerMessage, ViewerHello, ViewerProtoError,
    DEFAULT_RECORD_FPS,
};
use common::{every_client_message, every_server_message};
use serde_json::{json, Value};

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

    let decoded =
        decode_client(r#"{"type": "activate_window", "window_id": 17, "future": true}"#).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::ActivateWindow {
            window_id: WindowId(17),
        }
    );

    let decoded = decode_client(
        r#"{"type": "start_recording", "id": 5, "path": "/x", "fps": 12,
        "encoder": "gpu", "future": true}"#,
    )
    .unwrap();
    assert_eq!(
        decoded,
        ClientMessage::StartRecording {
            id: Some(5),
            path: Some("/x".to_owned()),
            fps: 12,
            encoder: RecordingEncoder::Gpu,
        }
    );

    let decoded = decode_client(r#"{"type": "stop_recording", "id": 6, "extra": true}"#).unwrap();
    assert_eq!(decoded, ClientMessage::StopRecording { id: Some(6) });

    let decoded = decode_server(
        r#"{"type": "recording", "recording": false, "fps": 30, "frames": 0,
        "duration_ms": 0, "future": "ignored"}"#,
    )
    .unwrap();
    assert_eq!(
        decoded,
        ServerMessage::Recording {
            id: None,
            status: RecordingStatus::idle(),
        }
    );
}

// --- recording messages ---------------------------------------------------

#[test]
fn start_recording_defaults_the_absent_optionals() {
    // No fields at all: `fps`/`encoder` default, `id`/`path` stay absent.
    let decoded = decode_client(r#"{"type": "start_recording"}"#).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::StartRecording {
            id: None,
            path: None,
            fps: DEFAULT_RECORD_FPS,
            encoder: RecordingEncoder::Auto,
        }
    );

    // Only `path` present: the counters still default.
    let decoded = decode_client(r#"{"type": "start_recording", "path": "/tmp/rec.webm"}"#).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::StartRecording {
            id: None,
            path: Some("/tmp/rec.webm".to_owned()),
            fps: DEFAULT_RECORD_FPS,
            encoder: RecordingEncoder::Auto,
        }
    );

    // Explicit values are honoured.
    let decoded = decode_client(
        r#"{"type": "start_recording", "id": 9, "path": "/tmp/rec.webm",
        "fps": 24, "encoder": "software"}"#,
    )
    .unwrap();
    assert_eq!(
        decoded,
        ClientMessage::StartRecording {
            id: Some(9),
            path: Some("/tmp/rec.webm".to_owned()),
            fps: 24,
            encoder: RecordingEncoder::Software,
        }
    );
}

#[test]
fn recording_round_trips_its_optional_fields() {
    let with_error = encode_server(&ServerMessage::Recording {
        id: Some(4),
        status: RecordingStatus::idle().with_error("encoder unavailable".to_owned()),
    });
    assert_eq!(
        decode_server(&with_error).unwrap(),
        ServerMessage::Recording {
            id: Some(4),
            status: RecordingStatus::idle().with_error("encoder unavailable".to_owned()),
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
        "{\"type\":\"text\"",                                 // unterminated
        "[1, 2]",                                             // not an object
        "\"text\"",                                           // not an object
        "42",                                                 // not an object
        "{}",                                                 // missing type
        r#"{"x": 1}"#,                                        // missing type
        r#"{"type": 5}"#,                                     // non-string type
        r#"{"type": null}"#,                                  // non-string type
        r#"{"type": "key"}"#,                                 // known tag, missing keys
        r#"{"type": "pointer_button"}"#,                      // known tag, missing fields
        r#"{"type": "activate_window"}"#,                     // known tag, missing window_id
        r#"{"type": "bye", "reason": 5}"#,                    // wrong field type
        r#"{"type": "start_recording", "encoder": "bogus"}"#, // unknown encoder
        r#"{"type": "start_recording", "fps": "fast"}"#,      // wrong field type
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
        r#"{"type": "recording"}"#, // known tag, missing the required counters
        r#"{"type": "recording", "recording": true, "fps": 30, "frames": 0}"#, // missing duration_ms
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
