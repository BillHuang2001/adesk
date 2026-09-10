//! Frame-envelope NDJSON goldens unique to this file (§1).
//!
//! The response/error frames, the core↔wire event mapping, the malformed-input
//! handling and the `Codec` trait behavior are covered by
//! `frames_events_roundtrip.rs`; the method envelope by `methods_roundtrip.rs`.
//! This file keeps only the exact encoded request lines and the encode/decode
//! round-trip over every frame kind.

mod common;

use adesk_core::{ActionId, Button, ErrorCode, Position, WindowId};
use adesk_proto::*;
use common::*;
use serde_json::json;

#[test]
fn encode_ping_request_exact_json() {
    let frame = Frame::Request(RequestFrame::new(1, Method::Ping(PingParams {})));
    let line = NdjsonCodec.encode_str(&frame).unwrap();
    assert!(!line.ends_with('\n'), "transport adds the newline");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({"id": 1, "method": "ping", "params": {}})
    );
    assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), frame);
}

#[test]
fn encode_click_request_matches_protocol_example() {
    let frame = Frame::Request(RequestFrame::new(
        1,
        Method::Click(ClickParams {
            window_id: WindowId(17),
            position: Some(Position::normalized(0.72, 0.41)),
            button: Button::Left,
            count: 1,
        }),
    ));
    let line = NdjsonCodec.encode_str(&frame).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({
            "id": 1,
            "method": "click",
            "params": {
                "window_id": 17,
                "position": {"type": "normalized", "x": 0.72, "y": 0.41},
                "button": "left",
                "count": 1
            }
        })
    );
    assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), frame);
}

#[test]
fn round_trip_all_frame_kinds() {
    let request = Frame::Request(RequestFrame::new(1, Method::GetFocus(GetFocusParams {})));
    let response_ok = Frame::Response(
        ResponseFrame::result(
            2,
            &ActionResult {
                action_id: ActionId(582),
            },
        )
        .unwrap(),
    );
    let response_err = Frame::Response(ResponseFrame::error(
        3,
        ErrorPayload::new(ErrorCode::UnknownWindow, "window 99 is not known")
            .with_data(json!({"window_id": 99})),
    ));
    let event = Frame::Event(EventFrame::new(
        EventKind::SurfaceCommit,
        8291,
        51234,
        EventPayload::SurfaceCommit(SurfaceCommitEvent {
            window_id: WindowId(17),
            commit_seq: 8291,
            damage: damage(),
        }),
    ));
    for frame in [request, response_ok, response_err, event] {
        let line = NdjsonCodec.encode_str(&frame).unwrap();
        assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), frame);
    }
}
