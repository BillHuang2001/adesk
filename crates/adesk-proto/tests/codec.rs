//! Frame-level wire behavior pinned against `docs/protocol.md` (§1, §4, §5.4, §6).
//!
//! These tests are the Phase-2 acceptance spec for the codec bodies; they are
//! `#[ignore]`d while those bodies are `todo!()` stubs. Implementation rule:
//! remove the `#[ignore]` attributes (do not edit the assertions) once the
//! codec is implemented.

use adesk_core::{
    ActionId, AppId, Button, ErrorCode, LaunchId, Observation, Position, Rect, Region, RuntimeEvent,
    WindowId,
};
use adesk_proto::*;
use serde_json::json;

fn damage() -> Region {
    let mut region = Region::empty();
    region.push(Rect::new(630, 220, 410, 180));
    region
}

fn observation() -> Observation {
    Observation {
        window_id: Some(WindowId(17)),
        after_action: Some(ActionId(582)),
        commits: 3,
        changed_regions: vec![Rect::new(630, 220, 410, 180)],
        focus_changed: Some(false),
        title_changed: false,
        new_windows: vec![],
        destroyed_windows: vec![],
        popups_appeared: vec![],
        popups_disappeared: vec![],
        elapsed_ms: 417,
        quiet: true,
        timed_out: false,
        last_commit_seq: 8291,
        seq: 8300,
    }
}

fn image_payload() -> ImagePayload {
    ImagePayload {
        width: 1280,
        height: 800,
        format: ImageFormat::Rgba8,
        stride: Some(5120),
        data: "AAAA".to_owned(),
        scale: 1.0,
    }
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn encode_ping_request_exact_json() {
    let frame = Frame::Request(RequestFrame::new(1, Method::Ping(PingParams {})));
    let line = encode_frame(&frame).unwrap();
    assert!(!line.ends_with('\n'), "transport adds the newline");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({"id": 1, "method": "ping", "params": {}})
    );
    assert_eq!(decode_frame(&line).unwrap(), frame);
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
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
    let line = encode_frame(&frame).unwrap();
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
    assert_eq!(decode_frame(&line).unwrap(), frame);
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn round_trip_all_frame_kinds() {
    let request = Frame::Request(RequestFrame::new(
        1,
        Method::GetFocus(GetFocusParams {}),
    ));
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
        let line = encode_frame(&frame).unwrap();
        assert_eq!(decode_frame(&line).unwrap(), frame);
    }
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn response_frames_match_protocol_examples() {
    let frame = decode_frame(r#"{"id": 1, "result": {"action_id": 582}}"#).unwrap();
    match frame {
        Frame::Response(response) => {
            assert_eq!(response.id, 1);
            assert_eq!(
                response
                    .outcome
                    .result_payload()
                    .unwrap()
                    .decode::<ActionResult>()
                    .unwrap(),
                ActionResult {
                    action_id: ActionId(582)
                }
            );
        }
        other => panic!("expected response frame, got {other:?}"),
    }

    let frame = decode_frame(
        r#"{"id": 1, "error": {"code": "unknown_window", "message": "window 99 is not known", "data": {"window_id": 99}}}"#,
    )
    .unwrap();
    match frame {
        Frame::Response(response) => {
            let error = response.outcome.error_payload().unwrap();
            assert_eq!(error.code, ErrorCode::UnknownWindow);
            assert_eq!(error.message, "window 99 is not known");
            assert_eq!(error.data, Some(json!({"window_id": 99})));
        }
        other => panic!("expected response frame, got {other:?}"),
    }
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn response_with_both_or_neither_result_and_error_is_malformed() {
    let both = r#"{"id": 1, "result": {}, "error": {"code": "internal", "message": "x"}}"#;
    assert!(matches!(
        decode_frame(both).unwrap_err(),
        ProtoError::Malformed(_)
    ));
    let neither = r#"{"id": 1}"#;
    assert!(matches!(
        decode_frame(neither).unwrap_err(),
        ProtoError::Malformed(_)
    ));
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn unknown_method_reports_unknown_method() {
    let err = decode_frame(r#"{"id": 1, "method": "bogus", "params": {}}"#).unwrap_err();
    assert!(matches!(err, ProtoError::UnknownMethod(ref name) if name == "bogus"));
    assert_eq!(err.error_code(), ErrorCode::UnknownMethod);
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn missing_or_null_params_defaults_to_empty_object() {
    let expected = Frame::Request(RequestFrame::new(1, Method::Ping(PingParams {})));
    assert_eq!(
        decode_frame(r#"{"id": 1, "method": "ping"}"#).unwrap(),
        expected
    );
    assert_eq!(
        decode_frame(r#"{"id": 1, "method": "ping", "params": null}"#).unwrap(),
        expected
    );
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn decode_ignores_unknown_fields() {
    let expected = Frame::Request(RequestFrame::new(1, Method::Ping(PingParams {})));
    assert_eq!(
        decode_frame(r#"{"id": 1, "method": "ping", "params": {}, "future_field": 42}"#).unwrap(),
        expected
    );

    let click = Frame::Request(RequestFrame::new(
        2,
        Method::Click(ClickParams {
            window_id: WindowId(17),
            position: None,
            button: Button::Left,
            count: 1,
        }),
    ));
    assert_eq!(
        decode_frame(
            r#"{"id": 2, "method": "click", "params": {"window_id": 17, "future_param": true}}"#
        )
        .unwrap(),
        click
    );
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn unknown_event_kind_is_rejected() {
    let err = decode_frame(r#"{"event": "bogus", "seq": 1, "ts_ms": 1, "data": {}}"#).unwrap_err();
    assert!(matches!(err, ProtoError::UnknownEventKind(ref name) if name == "bogus"));
    assert_eq!(err.error_code(), ErrorCode::InvalidRequest);
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn event_frame_matches_protocol_example() {
    let frame = EventFrame::new(
        EventKind::SurfaceCommit,
        8291,
        51234,
        EventPayload::SurfaceCommit(SurfaceCommitEvent {
            window_id: WindowId(17),
            commit_seq: 8291,
            damage: damage(),
        }),
    );
    let line = encode_frame(&Frame::Event(frame.clone())).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({
            "event": "surface_commit",
            "seq": 8291,
            "ts_ms": 51234,
            "data": {
                "window_id": 17,
                "commit_seq": 8291,
                "damage": [{"x": 630, "y": 220, "w": 410, "h": 180}]
            }
        })
    );
    assert_eq!(decode_frame(&line).unwrap(), Frame::Event(frame));
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn event_frame_round_trips_through_runtime_event() {
    let event = RuntimeEvent::SurfaceCommit {
        seq: 8291,
        ts_ms: 51234,
        window_id: WindowId(17),
        commit_seq: 8291,
        damage: damage(),
    };
    let frame = EventFrame::from_runtime(&event);
    assert_eq!(frame.event, EventKind::SurfaceCommit);
    assert_eq!(frame.seq, 8291);
    assert_eq!(frame.ts_ms, 51234);
    assert_eq!(frame.to_runtime().unwrap(), event);

    let line = encode_frame(&Frame::Event(frame.clone())).unwrap();
    let decoded = decode_frame(&line).unwrap();
    assert_eq!(decoded, Frame::Event(frame));
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn event_payload_maps_every_runtime_event() {
    let events = vec![
        RuntimeEvent::WindowCreated {
            seq: 800,
            ts_ms: 1,
            window_id: WindowId(17),
            app_id: Some(AppId::from("org.mozilla.firefox")),
            pid: Some(4242),
            launch_id: Some(LaunchId(7)),
            title: Some("GitHub".to_owned()),
        },
        RuntimeEvent::WindowDestroyed {
            seq: 801,
            ts_ms: 2,
            window_id: WindowId(17),
        },
        RuntimeEvent::WindowActivated {
            seq: 802,
            ts_ms: 3,
            window_id: WindowId(17),
            previous: None,
        },
        RuntimeEvent::TitleChanged {
            seq: 803,
            ts_ms: 4,
            window_id: WindowId(17),
            title: None,
        },
        RuntimeEvent::SurfaceCommit {
            seq: 804,
            ts_ms: 5,
            window_id: WindowId(17),
            commit_seq: 12,
            damage: damage(),
        },
        RuntimeEvent::FocusChanged {
            seq: 805,
            ts_ms: 6,
            window_id: None,
        },
        RuntimeEvent::PopupAppeared {
            seq: 806,
            ts_ms: 7,
            window_id: WindowId(17),
            popup_id: 3,
        },
        RuntimeEvent::PopupDisappeared {
            seq: 807,
            ts_ms: 8,
            window_id: WindowId(17),
            popup_id: 3,
        },
        RuntimeEvent::AppLaunched {
            seq: 808,
            ts_ms: 9,
            launch_id: LaunchId(7),
            app_id: AppId::from("org.mozilla.firefox"),
            pid: None,
        },
    ];
    assert_eq!(events.len(), 9);
    for event in events {
        let frame = EventFrame::from_runtime(&event);
        assert_eq!(frame.event, frame.data.kind());
        assert_eq!(frame.seq, event.seq());
        assert_eq!(frame.ts_ms, event.ts_ms());
        assert_eq!(frame.to_runtime().unwrap(), event);

        // The event survives a full encode/decode round-trip.
        let line = encode_frame(&Frame::Event(frame)).unwrap();
        assert!(matches!(decode_frame(&line).unwrap(), Frame::Event(_)));
    }
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn quiet_and_inspect_frame_have_no_runtime_event() {
    let quiet = EventPayload::Quiet(QuietEvent {
        window_id: Some(WindowId(17)),
        quiet_ms: 250,
    });
    assert!(quiet.to_runtime(1, 1).is_none());

    let inspect = EventPayload::InspectFrame(InspectFrameEvent {
        subscription_id: 3,
        image: image_payload(),
    });
    assert!(inspect.to_runtime(1, 1).is_none());
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn observe_result_wire_shape_and_round_trip() {
    let result = ObserveResult {
        observation: observation(),
        image: Some(image_payload()),
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["observation"]["commits"], json!(3));
    assert_eq!(value["observation"]["image"]["width"], json!(1280));
    assert_eq!(
        value["observation"]["changed_regions"],
        json!([{"x": 630, "y": 220, "w": 410, "h": 180}])
    );

    let payload = ResultPayload::new(&result).unwrap();
    assert_eq!(payload.decode::<ObserveResult>().unwrap(), result);

    let bare = ObserveResult {
        observation: observation(),
        image: None,
    };
    assert_eq!(
        serde_json::to_value(&bare).unwrap()["observation"]["image"],
        json!(null)
    );
    assert_eq!(
        ResultPayload::new(&bare)
            .unwrap()
            .decode::<ObserveResult>()
            .unwrap(),
        bare
    );
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn result_payload_new_and_decode() {
    let payload = ResultPayload::new(&PingResult {
        protocol_version: 1,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 5,
        renderer: RendererKind::Gl,
        output: adesk_core::Size::new(1280, 800),
    })
    .unwrap();
    let decoded: PingResult = payload.decode().unwrap();
    assert_eq!(decoded.protocol_version, 1);
    assert!(payload.decode::<ListWindowsResult>().is_err());
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn image_payload_base64_helpers() {
    let rgba: Vec<u8> = (0..32).collect(); // 4x2 pixels
    let payload = ImagePayload::from_rgba8(4, 2, &rgba, 1.0).unwrap();
    assert_eq!(payload.format, ImageFormat::Rgba8);
    assert_eq!(payload.stride, Some(16));
    assert_eq!(payload.decode_data().unwrap(), rgba);

    let buffer = payload.to_rgba8_buffer().unwrap();
    assert_eq!(buffer.width, 4);
    assert_eq!(buffer.pixel(1, 0), Some([4, 5, 6, 7]));

    let png = ImagePayload::from_png(4, 2, &[0x89, b'P', b'N', b'G'], 1.0);
    assert_eq!(png.format, ImageFormat::Png);
    assert_eq!(png.stride, None);
    assert_eq!(png.decode_data().unwrap(), vec![0x89, b'P', b'N', b'G']);
    assert!(png.to_rgba8_buffer().is_err());

    // Wrong length is rejected, never silently truncated.
    assert!(ImagePayload::from_rgba8(4, 2, &rgba[..10], 1.0).is_err());
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn event_kind_matches_semantics() {
    let empty_commit = RuntimeEvent::SurfaceCommit {
        seq: 1,
        ts_ms: 1,
        window_id: WindowId(1),
        commit_seq: 1,
        damage: Region::empty(),
    };
    let damaged_commit = RuntimeEvent::SurfaceCommit {
        seq: 2,
        ts_ms: 2,
        window_id: WindowId(1),
        commit_seq: 2,
        damage: damage(),
    };
    let destroyed = RuntimeEvent::WindowDestroyed {
        seq: 3,
        ts_ms: 3,
        window_id: WindowId(1),
    };

    assert!(EventKind::SurfaceCommit.matches(&empty_commit));
    assert!(EventKind::SurfaceCommit.matches(&damaged_commit));
    assert!(!EventKind::SurfaceDamage.matches(&empty_commit));
    assert!(EventKind::SurfaceDamage.matches(&damaged_commit));
    assert!(!EventKind::SurfaceDamage.matches(&destroyed));
    assert!(EventKind::WindowDestroyed.matches(&destroyed));
    assert!(!EventKind::WindowDestroyed.matches(&empty_commit));
    assert!(!EventKind::Quiet.matches(&empty_commit));
    assert!(!EventKind::InspectFrame.matches(&empty_commit));
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn malformed_lines_are_rejected() {
    for line in ["", "   ", "not json", "{}", "[]", "{\"id\": 1}"] {
        assert!(decode_frame(line).is_err(), "line {line:?} must not decode");
    }
}

#[test]
#[ignore = "phase 2: codec bodies are todo!() stubs"]
fn ndjson_codec_works_through_the_trait() {
    let codec: &dyn Codec = &NdjsonCodec;
    assert_eq!(codec.name(), "ndjson");

    let frame = Frame::Request(RequestFrame::new(1, Method::GetFocus(GetFocusParams {})));
    let bytes = codec.encode(&frame).unwrap();
    assert!(!bytes.contains(&b'\n'), "payload carries no terminator");
    assert_eq!(codec.decode(&bytes).unwrap(), frame);
}
