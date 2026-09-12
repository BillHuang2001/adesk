//! Frame, event and codec behavior of the AGP wire layer (§1, §4, §5.6, §6).
//!
//! These tests cover the response/event frame shapes, the core↔wire event
//! mapping and the NDJSON codec. Request frames are deliberately absent: they
//! embed `Method`'s serde, which is owned by `src/methods*.rs` and covered at
//! integration.

mod common;

use adesk_core::{
    ActionId, AppId, ErrorCode, LaunchId, NotificationId, Region, RuntimeEvent, WindowId,
};
use adesk_proto::*;
use common::*;
use serde_json::json;

fn surface_commit_payload() -> EventPayload {
    EventPayload::SurfaceCommit(SurfaceCommitEvent {
        window_id: WindowId(17),
        commit_seq: 8291,
        damage: damage(),
    })
}

/// One representative payload per emitted event kind (§5.6 + §5.7 push + §5.9).
fn all_payloads() -> Vec<EventPayload> {
    vec![
        EventPayload::WindowCreated(WindowCreatedEvent {
            window_id: WindowId(17),
            app_id: Some(AppId::from("org.mozilla.firefox")),
            pid: Some(4242),
            launch_id: Some(LaunchId(7)),
            title: Some("GitHub".to_owned()),
        }),
        EventPayload::WindowDestroyed(WindowDestroyedEvent {
            window_id: WindowId(17),
        }),
        EventPayload::WindowActivated(WindowActivatedEvent {
            window_id: WindowId(17),
            previous: Some(WindowId(16)),
        }),
        EventPayload::TitleChanged(TitleChangedEvent {
            window_id: WindowId(17),
            title: None,
        }),
        surface_commit_payload(),
        EventPayload::FocusChanged(FocusChangedEvent { window_id: None }),
        EventPayload::PopupAppeared(PopupAppearedEvent {
            window_id: WindowId(17),
            popup_id: 3,
        }),
        EventPayload::PopupDisappeared(PopupDisappearedEvent {
            window_id: WindowId(17),
            popup_id: 3,
        }),
        EventPayload::AppLaunched(AppLaunchedEvent {
            launch_id: LaunchId(7),
            app_id: AppId::from("org.mozilla.firefox"),
            pid: Some(4242),
        }),
        EventPayload::Notification(NotificationEvent {
            notification: notification(),
        }),
        EventPayload::NotificationClosed(NotificationClosedEvent {
            notification_id: NotificationId(5),
            reason: NotificationCloseReason::Expired,
        }),
        EventPayload::NotificationAction(NotificationActionEvent {
            notification_id: NotificationId(5),
            action_key: "open".to_owned(),
        }),
        EventPayload::Quiet(QuietEvent {
            window_id: Some(WindowId(17)),
            quiet_ms: 250,
        }),
        EventPayload::InspectFrame(InspectFrameEvent {
            subscription_id: 3,
            image: image_payload(),
        }),
    ]
}

/// Every core runtime event, one per `RuntimeEvent` variant.
fn all_runtime_events() -> Vec<RuntimeEvent> {
    vec![
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
        RuntimeEvent::Notification {
            seq: 809,
            ts_ms: 10,
            notification: notification(),
        },
        RuntimeEvent::NotificationClosed {
            seq: 810,
            ts_ms: 11,
            notification_id: NotificationId(5),
            reason: NotificationCloseReason::Dismissed,
        },
        RuntimeEvent::NotificationAction {
            seq: 811,
            ts_ms: 12,
            notification_id: NotificationId(5),
            action_key: "open".to_owned(),
        },
    ]
}

#[test]
fn response_result_matches_protocol_example() {
    let frame = Frame::Response(
        ResponseFrame::result(
            1,
            &ActionResult {
                action_id: ActionId(582),
            },
        )
        .unwrap(),
    );
    let line = NdjsonCodec.encode_str(&frame).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({"id": 1, "result": {"action_id": 582}})
    );

    let decoded = NdjsonCodec
        .decode_str(r#"{"id": 1, "result": {"action_id": 582}}"#)
        .unwrap();
    match &decoded {
        Frame::Response(response) => {
            assert_eq!(response.id, 1);
            match &response.outcome {
                ResponseOutcome::Result(payload) => assert_eq!(
                    payload.decode::<ActionResult>().unwrap(),
                    ActionResult {
                        action_id: ActionId(582)
                    }
                ),
                ResponseOutcome::Error(other) => panic!("expected a result payload, got {other:?}"),
            }
        }
        other => panic!("expected response frame, got {other:?}"),
    }
    assert_eq!(decoded, frame);
}

#[test]
fn response_error_matches_protocol_example() {
    let error = ErrorPayload::new(ErrorCode::UnknownWindow, "window 99 is not known")
        .with_data(json!({"window_id": 99}));
    let frame = Frame::Response(ResponseFrame::error(1, error));
    let line = NdjsonCodec.encode_str(&frame).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        json!({
            "id": 1,
            "error": {
                "code": "unknown_window",
                "message": "window 99 is not known",
                "data": {"window_id": 99}
            }
        })
    );

    let decoded = NdjsonCodec.decode_str(
        r#"{"id": 1, "error": {"code": "unknown_window", "message": "window 99 is not known", "data": {"window_id": 99}}}"#,
    )
    .unwrap();
    match decoded {
        Frame::Response(response) => {
            assert_eq!(response.id, 1);
            let error = match &response.outcome {
                ResponseOutcome::Error(error) => error,
                ResponseOutcome::Result(other) => {
                    panic!("expected an error payload, got {other:?}")
                }
            };
            assert_eq!(error.code, ErrorCode::UnknownWindow);
            assert_eq!(error.message, "window 99 is not known");
            assert_eq!(error.data, Some(json!({"window_id": 99})));
        }
        other => panic!("expected response frame, got {other:?}"),
    }
}

#[test]
fn response_with_both_or_neither_outcome_is_malformed() {
    let both = r#"{"id": 1, "result": {}, "error": {"code": "internal", "message": "x"}}"#;
    let neither = r#"{"id": 1}"#;
    for line in [both, neither] {
        assert!(
            matches!(
                NdjsonCodec.decode_str(line).unwrap_err(),
                ProtoError::Malformed(_)
            ),
            "line {line:?} must be malformed"
        );
        // The same contract holds for the typed serde entry point.
        assert!(serde_json::from_str::<ResponseFrame>(line).is_err());
    }

    // An id alone is not enough, and an id is required either way.
    assert!(NdjsonCodec.decode_str(r#"{"result": {}}"#).is_err());
    assert!(NdjsonCodec
        .decode_str(r#"{"id": "1", "result": {}}"#)
        .is_err());
}

#[test]
fn payload_new_and_decode() {
    let payload = ResultPayload::new(&ping_result()).unwrap();
    assert_eq!(payload.as_value()["protocol_version"], json!(1));
    assert_eq!(payload.decode::<PingResult>().unwrap(), ping_result());
    assert!(matches!(
        payload.decode::<ListWindowsResult>().unwrap_err(),
        ProtoError::InvalidResult(_)
    ));

    // `ResponseFrame::result` uses the same encoding.
    let response = ResponseFrame::result(7, &ping_result()).unwrap();
    match &response.outcome {
        ResponseOutcome::Result(payload) => {
            assert_eq!(payload, &ResultPayload::new(&ping_result()).unwrap());
        }
        ResponseOutcome::Error(other) => panic!("expected a result payload, got {other:?}"),
    }
}

#[test]
fn event_frame_matches_protocol_example() {
    let frame = EventFrame::new(
        EventKind::SurfaceCommit,
        8291,
        51234,
        surface_commit_payload(),
    );
    let line = NdjsonCodec
        .encode_str(&Frame::Event(frame.clone()))
        .unwrap();
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
    assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), Frame::Event(frame));
}

#[test]
fn every_payload_kind_round_trips_through_the_wire() {
    for (index, payload) in all_payloads().into_iter().enumerate() {
        let kind = payload.kind();
        let seq = 100 + index as u64;
        let ts_ms = 200 + index as u64;
        let frame = EventFrame::new(kind, seq, ts_ms, payload.clone());
        assert_eq!(frame.event, kind);

        let line = NdjsonCodec
            .encode_str(&Frame::Event(frame.clone()))
            .unwrap();
        assert_eq!(
            NdjsonCodec.decode_str(&line).unwrap(),
            Frame::Event(frame.clone())
        );

        let data = payload.to_data().unwrap();
        assert_eq!(EventPayload::from_data(kind, data).unwrap(), payload);
        assert_eq!(frame.to_runtime(), payload.to_runtime(seq, ts_ms));
    }
}

#[test]
fn notification_event_data_matches_spec() {
    let notif = notification();

    // `notification` (§5.9): data = {"notification": Notification}.
    let payload = EventPayload::Notification(NotificationEvent {
        notification: notif.clone(),
    });
    assert_eq!(payload.kind(), EventKind::Notification);
    let data = payload.to_data().unwrap();
    assert_eq!(data, json!({"notification": notif}));
    assert_eq!(
        EventPayload::from_data(EventKind::Notification, data).unwrap(),
        payload
    );

    // `notification_closed` (§5.9): data = {"notification_id": u64, "reason": string}.
    let closed = EventPayload::NotificationClosed(NotificationClosedEvent {
        notification_id: NotificationId(5),
        reason: NotificationCloseReason::Expired,
    });
    assert_eq!(closed.kind(), EventKind::NotificationClosed);
    assert_eq!(
        closed.to_data().unwrap(),
        json!({"notification_id": 5, "reason": "expired"})
    );
    assert_eq!(
        EventPayload::from_data(EventKind::NotificationClosed, closed.to_data().unwrap()).unwrap(),
        closed
    );

    // `notification_action` (§5.9): data = {"notification_id": u64, "action_key": string}.
    let action = EventPayload::NotificationAction(NotificationActionEvent {
        notification_id: NotificationId(5),
        action_key: "open".to_owned(),
    });
    assert_eq!(action.kind(), EventKind::NotificationAction);
    assert_eq!(
        action.to_data().unwrap(),
        json!({"notification_id": 5, "action_key": "open"})
    );
    assert_eq!(
        EventPayload::from_data(EventKind::NotificationAction, action.to_data().unwrap()).unwrap(),
        action
    );

    // The three kinds are subscribable (§5.9) and match only their own event.
    let runtime_notification = RuntimeEvent::Notification {
        seq: 1,
        ts_ms: 1,
        notification: notif,
    };
    assert!(EventKind::Notification.is_subscribable());
    assert!(EventKind::NotificationClosed.is_subscribable());
    assert!(EventKind::NotificationAction.is_subscribable());
    assert!(EventKind::Notification.matches(&runtime_notification));
    assert!(!EventKind::NotificationClosed.matches(&runtime_notification));
    assert!(!EventKind::NotificationAction.matches(&runtime_notification));
}

#[test]
fn runtime_events_round_trip_through_event_frames() {
    for event in all_runtime_events() {
        let frame = EventFrame::from_runtime(&event);
        assert_eq!(frame.event, frame.data.kind());
        assert_eq!(frame.seq, event.seq());
        assert_eq!(frame.ts_ms, event.ts_ms());
        assert_eq!(frame.to_runtime().unwrap(), event);

        let line = NdjsonCodec
            .encode_str(&Frame::Event(frame.clone()))
            .unwrap();
        assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), Frame::Event(frame));
    }
}

#[test]
fn quiet_and_inspect_frame_have_no_runtime_event() {
    let quiet = EventPayload::Quiet(QuietEvent {
        window_id: Some(WindowId(17)),
        quiet_ms: 250,
    });
    assert!(quiet.to_runtime(1, 1).is_none());
    assert!(EventFrame::new(EventKind::Quiet, 1, 1, quiet)
        .to_runtime()
        .is_none());

    let inspect = EventPayload::InspectFrame(InspectFrameEvent {
        subscription_id: 3,
        image: image_payload(),
    });
    assert!(inspect.to_runtime(1, 1).is_none());
    assert!(EventFrame::new(EventKind::InspectFrame, 1, 1, inspect)
        .to_runtime()
        .is_none());
}

#[test]
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

    // Every core event matches exactly its own kind (plus `surface_damage`
    // for commits with damage).
    for event in all_runtime_events() {
        let payload = EventPayload::from_runtime(&event);
        let mut expected = vec![payload.kind()];
        if let RuntimeEvent::SurfaceCommit { damage, .. } = &event {
            if !damage.is_empty() {
                expected.push(EventKind::SurfaceDamage);
            }
        }
        let matched: Vec<EventKind> = EventKind::SUBSCRIBABLE
            .iter()
            .copied()
            .filter(|kind| kind.matches(&event))
            .collect();
        assert_eq!(matched, expected, "event {event:?}");
    }
}

#[test]
fn from_data_rejects_surface_damage_and_mismatched_data() {
    // `surface_damage` is a filter alias, never an emitted kind.
    assert!(matches!(
        EventPayload::from_data(EventKind::SurfaceDamage, json!({})).unwrap_err(),
        ProtoError::Malformed(_)
    ));
    assert!(matches!(
        NdjsonCodec
            .decode_str(r#"{"event": "surface_damage", "seq": 1, "ts_ms": 1, "data": {}}"#)
            .unwrap_err(),
        ProtoError::Malformed(_)
    ));

    // Shape mismatches are `invalid_event_data`, naming the kind.
    let err = EventPayload::from_data(EventKind::WindowDestroyed, json!({"window_id": "17"}))
        .unwrap_err();
    assert!(
        matches!(err, ProtoError::InvalidEventData { ref kind, .. } if kind == "window_destroyed"),
        "unexpected error: {err:?}"
    );
    assert!(EventPayload::from_data(EventKind::SurfaceCommit, json!({"window_id": 17})).is_err());
    assert!(EventPayload::from_data(EventKind::Quiet, json!(null)).is_err());

    // Unknown kinds and wrong field types are rejected before `data` matters.
    assert!(matches!(
        NdjsonCodec.decode_str(r#"{"event": "bogus", "seq": 1, "ts_ms": 1, "data": {}}"#).unwrap_err(),
        ProtoError::UnknownEventKind(ref name) if name == "bogus"
    ));
    assert!(matches!(
        NdjsonCodec
            .decode_str(r#"{"event": 5, "seq": 1, "ts_ms": 1, "data": {}}"#)
            .unwrap_err(),
        ProtoError::Malformed(_)
    ));
    assert!(matches!(
        NdjsonCodec
            .decode_str(r#"{"event": "quiet", "data": {}}"#)
            .unwrap_err(),
        ProtoError::Malformed(_)
    ));
    assert!(matches!(
        NdjsonCodec
            .decode_str(r#"{"event": "quiet", "seq": 1, "ts_ms": 2, "data": {}}"#)
            .unwrap_err(),
        ProtoError::InvalidEventData { .. }
    ));
}

#[test]
fn event_frame_accepts_known_data_and_ignores_unknown_fields() {
    let frame = NdjsonCodec.decode_str(
        r#"{"event": "window_destroyed", "seq": 1, "ts_ms": 2, "data": {"window_id": 17, "future_field": 42}}"#,
    )
    .unwrap();
    assert_eq!(
        frame,
        Frame::Event(EventFrame::new(
            EventKind::WindowDestroyed,
            1,
            2,
            EventPayload::WindowDestroyed(WindowDestroyedEvent {
                window_id: WindowId(17)
            })
        ))
    );

    let quiet = NdjsonCodec.decode_str(
        r#"{"event": "quiet", "seq": 3, "ts_ms": 4, "data": {"window_id": null, "quiet_ms": 250}}"#,
    )
    .unwrap();
    assert_eq!(
        quiet,
        Frame::Event(EventFrame::new(
            EventKind::Quiet,
            3,
            4,
            EventPayload::Quiet(QuietEvent {
                window_id: None,
                quiet_ms: 250
            })
        ))
    );
}

#[test]
fn frame_serde_discriminates_by_keys() {
    let response: Frame =
        serde_json::from_str(r#"{"id": 1, "result": {"action_id": 582}}"#).unwrap();
    assert!(matches!(response, Frame::Response(_)));

    let event: Frame = serde_json::from_str(
        r#"{"event": "surface_commit", "seq": 8291, "ts_ms": 51234, "data": {"window_id": 17, "commit_seq": 8291, "damage": []}}"#,
    )
    .unwrap();
    assert!(matches!(event, Frame::Event(_)));

    for line in ["null", "42", "\"ping\"", "[]", "{}"] {
        assert!(
            NdjsonCodec.decode_str(line).is_err(),
            "line {line:?} must not decode"
        );
        assert!(serde_json::from_str::<Frame>(line).is_err());
    }
}

#[test]
fn malformed_lines_are_rejected() {
    for line in [
        "",
        "   ",
        "not json",
        "{}",
        "[]",
        "{\"id\": 1}",
        "{\"id\": 1, \"result\": {}, \"error\": {\"code\": \"internal\", \"message\": \"x\"}}",
        "{\"event\": \"bogus\", \"seq\": 1, \"ts_ms\": 1, \"data\": {}}",
    ] {
        assert!(
            NdjsonCodec.decode_str(line).is_err(),
            "line {line:?} must not decode"
        );
    }
}

#[test]
fn ndjson_codec_encodes_without_a_terminator_and_round_trips() {
    let frame = Frame::Event(EventFrame::new(
        EventKind::SurfaceCommit,
        8291,
        51234,
        surface_commit_payload(),
    ));
    let line = NdjsonCodec.encode_str(&frame).unwrap();
    assert!(!line.contains('\n'), "payload carries no terminator");
    assert!(!line.ends_with('\n'), "transport adds the newline");
    assert_eq!(NdjsonCodec.decode_str(&line).unwrap(), frame);
    let codec: &dyn Codec = &NdjsonCodec;
    assert_eq!(codec.name(), "ndjson");
    let bytes = codec.encode(&frame).unwrap();
    assert!(!bytes.contains(&b'\n'), "payload carries no terminator");
    assert_eq!(codec.decode(&bytes).unwrap(), frame);

    // Invalid UTF-8 is a codec-level failure, not a panic.
    assert!(codec.decode(&[0xff, 0xfe]).is_err());
}
