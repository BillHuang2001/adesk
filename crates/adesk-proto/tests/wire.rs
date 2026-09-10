//! Golden-JSON and round-trip tests over the protocol type definitions.
//!
//! These exercise only the derived wire shapes and the static tables
//! (method names, event kinds, defaults, error-code mapping); frame-level
//! behavior is pinned in `codec.rs`.

use adesk_core::{
    ActionId, AppId, AppInfo, Button, ErrorCode, Observation, OverlayKind, Position, Rect, Region,
    Size, WindowId, WindowInfo, WindowState,
};
use adesk_proto::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

fn wire<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize")
}

fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&back, value, "round-trip mismatch for {json}");
}

fn window_info() -> WindowInfo {
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

fn app_info() -> AppInfo {
    AppInfo {
        id: AppId::from("org.mozilla.firefox"),
        name: "Firefox".to_owned(),
        icon: Some("firefox".to_owned()),
        exec: Some("/usr/bin/firefox %u".to_owned()),
        terminal: false,
        categories: vec!["Network".to_owned()],
        startup_wm_class: Some("firefox".to_owned()),
        dbus_activatable: false,
        hidden: false,
        no_display: false,
        try_exec: None,
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

fn damage() -> Region {
    let mut region = Region::empty();
    region.push(Rect::new(630, 220, 410, 180));
    region
}

#[test]
fn protocol_version_is_one_and_checked() {
    assert_eq!(PROTOCOL_VERSION, 1);
    assert!(is_compatible_version(1));
    assert!(!is_compatible_version(0));
    assert!(!is_compatible_version(2));
    assert!(check_version(1).is_ok());
    let err = check_version(2).unwrap_err();
    assert!(matches!(
        err,
        ProtoError::VersionMismatch {
            expected: 1,
            got: 2
        }
    ));
    assert_eq!(err.error_code(), ErrorCode::ProtocolVersionMismatch);
}

#[test]
fn event_kind_wire_names() {
    let expected = [
        (EventKind::WindowCreated, "window_created"),
        (EventKind::WindowDestroyed, "window_destroyed"),
        (EventKind::WindowActivated, "window_activated"),
        (EventKind::TitleChanged, "title_changed"),
        (EventKind::SurfaceCommit, "surface_commit"),
        (EventKind::SurfaceDamage, "surface_damage"),
        (EventKind::FocusChanged, "focus_changed"),
        (EventKind::PopupAppeared, "popup_appeared"),
        (EventKind::PopupDisappeared, "popup_disappeared"),
        (EventKind::Quiet, "quiet"),
        (EventKind::AppLaunched, "app_launched"),
        (EventKind::InspectFrame, "inspect_frame"),
    ];
    assert_eq!(expected.len(), EventKind::SUBSCRIBABLE.len() + 1);
    for (kind, name) in expected {
        assert_eq!(wire(&kind), json!(name));
        assert_eq!(serde_json::from_value::<EventKind>(json!(name)).unwrap(), kind);
    }
    assert_eq!(EventKind::SUBSCRIBABLE.len(), 11);
    assert!(EventKind::SUBSCRIBABLE
        .iter()
        .all(EventKind::is_subscribable));
    assert!(!EventKind::InspectFrame.is_subscribable());
}

#[test]
fn event_payload_kind_matches_variant() {
    let payloads = [
        (
            EventPayload::WindowCreated(WindowCreatedEvent {
                window_id: WindowId(17),
                app_id: None,
                pid: None,
                launch_id: None,
                title: None,
            }),
            EventKind::WindowCreated,
        ),
        (
            EventPayload::WindowDestroyed(WindowDestroyedEvent {
                window_id: WindowId(17),
            }),
            EventKind::WindowDestroyed,
        ),
        (
            EventPayload::WindowActivated(WindowActivatedEvent {
                window_id: WindowId(17),
                previous: None,
            }),
            EventKind::WindowActivated,
        ),
        (
            EventPayload::TitleChanged(TitleChangedEvent {
                window_id: WindowId(17),
                title: None,
            }),
            EventKind::TitleChanged,
        ),
        (
            EventPayload::SurfaceCommit(SurfaceCommitEvent {
                window_id: WindowId(17),
                commit_seq: 8291,
                damage: damage(),
            }),
            EventKind::SurfaceCommit,
        ),
        (
            EventPayload::FocusChanged(FocusChangedEvent { window_id: None }),
            EventKind::FocusChanged,
        ),
        (
            EventPayload::PopupAppeared(PopupAppearedEvent {
                window_id: WindowId(17),
                popup_id: 1,
            }),
            EventKind::PopupAppeared,
        ),
        (
            EventPayload::PopupDisappeared(PopupDisappearedEvent {
                window_id: WindowId(17),
                popup_id: 1,
            }),
            EventKind::PopupDisappeared,
        ),
        (
            EventPayload::AppLaunched(AppLaunchedEvent {
                launch_id: 7.into(),
                app_id: AppId::from("org.mozilla.firefox"),
                pid: None,
            }),
            EventKind::AppLaunched,
        ),
        (
            EventPayload::Quiet(QuietEvent {
                window_id: Some(WindowId(17)),
                quiet_ms: 250,
            }),
            EventKind::Quiet,
        ),
        (
            EventPayload::InspectFrame(InspectFrameEvent {
                subscription_id: 3,
                image: image_payload(),
            }),
            EventKind::InspectFrame,
        ),
    ];
    for (payload, kind) in payloads {
        assert_eq!(payload.kind(), kind);
    }
}

#[test]
fn method_name_table() {
    let cases: Vec<(Method, &str)> = vec![
        (Method::Ping(PingParams {}), "ping"),
        (Method::ListApps(ListAppsParams::default()), "list_apps"),
        (
            Method::GetApp(GetAppParams {
                app_id: AppId::from("x"),
            }),
            "get_app",
        ),
        (
            Method::LaunchApp(LaunchAppParams {
                app_id: AppId::from("x"),
                args: vec![],
            }),
            "launch_app",
        ),
        (Method::ListWindows(ListWindowsParams {}), "list_windows"),
        (
            Method::GetWindow(GetWindowParams {
                window_id: WindowId(1),
            }),
            "get_window",
        ),
        (
            Method::ActivateWindow(ActivateWindowParams {
                window_id: WindowId(1),
            }),
            "activate_window",
        ),
        (
            Method::CloseWindow(CloseWindowParams {
                window_id: WindowId(1),
            }),
            "close_window",
        ),
        (Method::GetFocus(GetFocusParams {}), "get_focus"),
        (
            Method::CaptureWindow(CaptureWindowParams {
                window_id: WindowId(1),
                region: None,
                max_dimension: None,
                format: ImageFormat::Png,
            }),
            "capture_window",
        ),
        (
            Method::CaptureRegion(CaptureRegionParams {
                window_id: WindowId(1),
                region: Rect::EMPTY,
                max_dimension: None,
                format: ImageFormat::Png,
            }),
            "capture_region",
        ),
        (
            Method::Observe(ObserveParams {
                window_id: None,
                after_action: None,
                until: Condition::Change,
                timeout_ms: 5000,
                include_image: true,
                max_dimension: None,
                region: None,
            }),
            "observe",
        ),
        (
            Method::WaitForChange(WaitForChangeParams {
                window_id: None,
                since_commit: None,
                timeout_ms: 5000,
                include_image: false,
            }),
            "wait_for_change",
        ),
        (
            Method::WaitForQuiet(WaitForQuietParams {
                window_id: None,
                quiet_ms: 250,
                timeout_ms: 5000,
                after_action: None,
                include_image: false,
            }),
            "wait_for_quiet",
        ),
        (
            Method::PointerMove(PointerMoveParams {
                window_id: WindowId(1),
                position: Position::pixels(1, 2),
            }),
            "pointer_move",
        ),
        (
            Method::Click(ClickParams {
                window_id: WindowId(1),
                position: None,
                button: Button::Left,
                count: 1,
            }),
            "click",
        ),
        (
            Method::DoubleClick(DoubleClickParams {
                window_id: WindowId(1),
                position: None,
                button: Button::Left,
            }),
            "double_click",
        ),
        (
            Method::MouseDown(MouseDownParams {
                window_id: WindowId(1),
                position: None,
                button: Button::Left,
            }),
            "mouse_down",
        ),
        (
            Method::MouseUp(MouseUpParams {
                window_id: WindowId(1),
                position: None,
                button: Button::Left,
            }),
            "mouse_up",
        ),
        (
            Method::Scroll(ScrollParams {
                window_id: WindowId(1),
                position: None,
                dx: 0.0,
                dy: 1.0,
            }),
            "scroll",
        ),
        (
            Method::Drag(DragParams {
                window_id: WindowId(1),
                from: Position::pixels(0, 0),
                to: Position::pixels(10, 10),
                button: Button::Left,
                duration_ms: 150,
            }),
            "drag",
        ),
        (
            Method::Keypress(KeypressParams {
                keys: KeySpec::from("a"),
                window_id: None,
            }),
            "keypress",
        ),
        (
            Method::KeyDown(KeyDownParams {
                key: "a".to_owned(),
                window_id: None,
            }),
            "key_down",
        ),
        (
            Method::KeyUp(KeyUpParams {
                key: "a".to_owned(),
                window_id: None,
            }),
            "key_up",
        ),
        (
            Method::TypeText(TypeTextParams {
                text: "hi".to_owned(),
                window_id: None,
            }),
            "type_text",
        ),
        (
            Method::SubscribeEvents(SubscribeEventsParams::default()),
            "subscribe_events",
        ),
        (
            Method::UnsubscribeEvents(UnsubscribeEventsParams { subscription_id: 1 }),
            "unsubscribe_events",
        ),
        (
            Method::InspectCapture(InspectCaptureParams::default()),
            "inspect_capture",
        ),
        (
            Method::InspectSubscribe(InspectSubscribeParams {
                overlays: vec![OverlayKind::Damage],
                min_interval_ms: 100,
            }),
            "inspect_subscribe",
        ),
    ];
    assert_eq!(cases.len(), 29);
    let mut names: Vec<&str> = Vec::new();
    for (method, expected) in &cases {
        assert_eq!(method.method_name(), *expected);
        names.push(method.method_name());
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 29, "method names must be unique");
}

#[test]
fn click_params_match_protocol_example() {
    let params = ClickParams {
        window_id: WindowId(17),
        position: Some(Position::normalized(0.72, 0.41)),
        button: Button::Left,
        count: 1,
    };
    assert_eq!(
        wire(&params),
        json!({
            "window_id": 17,
            "position": {"type": "normalized", "x": 0.72, "y": 0.41},
            "button": "left",
            "count": 1
        })
    );
    roundtrip(&params);
}

#[test]
fn action_result_matches_protocol_example() {
    let result = ActionResult {
        action_id: ActionId(582),
    };
    assert_eq!(wire(&result), json!({"action_id": 582}));
    roundtrip(&result);
}

#[test]
fn error_payload_matches_protocol_example() {
    let error = ErrorPayload::new(ErrorCode::UnknownWindow, "window 99 is not known")
        .with_data(json!({"window_id": 99}));
    assert_eq!(
        wire(&error),
        json!({
            "code": "unknown_window",
            "message": "window 99 is not known",
            "data": {"window_id": 99}
        })
    );
    roundtrip(&error);

    let without_data = ErrorPayload::new(ErrorCode::Timeout, "timed out");
    assert_eq!(
        wire(&without_data),
        json!({"code": "timeout", "message": "timed out"})
    );
    roundtrip(&without_data);
}

#[test]
fn image_payload_matches_protocol_example() {
    assert_eq!(
        wire(&image_payload()),
        json!({
            "width": 1280,
            "height": 800,
            "format": "rgba8",
            "stride": 5120,
            "data": "AAAA",
            "scale": 1.0
        })
    );
    roundtrip(&image_payload());

    let png: ImagePayload = serde_json::from_value(json!({
        "width": 4,
        "height": 2,
        "format": "png",
        "data": "AAAA"
    }))
    .expect("png payload without stride/scale");
    assert_eq!(png.stride, None);
    assert_eq!(png.scale, 1.0);
}

#[test]
fn ping_result_matches_protocol_example() {
    let result = PingResult {
        protocol_version: 1,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 1234,
        renderer: RendererKind::Pixman,
        output: Size::new(1280, 800),
    };
    assert_eq!(
        wire(&result),
        json!({
            "protocol_version": 1,
            "runtime_version": "0.1.0",
            "uptime_ms": 1234,
            "renderer": "pixman",
            "output": {"w": 1280, "h": 800}
        })
    );
    roundtrip(&result);
    assert!(result.is_compatible());
    let mismatched = PingResult {
        protocol_version: 2,
        ..result
    };
    assert!(!mismatched.is_compatible());
}

#[test]
fn condition_wire_shapes() {
    assert_eq!(
        wire(&Condition::Quiet { quiet_ms: 250 }),
        json!({"type": "quiet", "quiet_ms": 250})
    );
    assert_eq!(wire(&Condition::Change), json!({"type": "change"}));
    assert_eq!(wire(&Condition::Timeout), json!({"type": "timeout"}));
    roundtrip(&Condition::Quiet { quiet_ms: 10 });
    roundtrip(&Condition::Change);
    roundtrip(&Condition::Timeout);
}

#[test]
fn key_spec_wire_shapes() {
    assert_eq!(wire(&KeySpec::from("a")), json!("a"));
    assert_eq!(
        wire(&KeySpec::from(vec!["CTRL".to_owned(), "L".to_owned()])),
        json!(["CTRL", "L"])
    );
    roundtrip(&KeySpec::from("RETURN"));
    roundtrip(&KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]));
    assert_eq!(KeySpec::from("a").keys(), ["a"]);
    assert_eq!(
        KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]).keys(),
        ["CTRL", "L"]
    );
}

#[test]
fn observe_params_defaults() {
    let params: ObserveParams =
        serde_json::from_value(json!({"until": {"type": "change"}})).expect("defaults");
    assert_eq!(params.timeout_ms, 5000);
    assert!(params.include_image);
    assert_eq!(params.window_id, None);
    assert_eq!(params.after_action, None);
    assert_eq!(params.max_dimension, None);
    assert_eq!(params.region, None);
    roundtrip(&params);
}

#[test]
fn click_params_defaults() {
    let params: ClickParams = serde_json::from_value(json!({"window_id": 17})).expect("defaults");
    assert_eq!(params.button, Button::Left);
    assert_eq!(params.count, 1);
    assert_eq!(params.position, None);
    assert_eq!(
        wire(&params),
        json!({"window_id": 17, "button": "left", "count": 1})
    );
}

#[test]
fn wait_method_defaults() {
    let change: WaitForChangeParams =
        serde_json::from_value(json!({})).expect("wait_for_change defaults");
    assert_eq!(change.timeout_ms, 5000);
    assert!(!change.include_image);
    assert_eq!(change.since_commit, None);

    let quiet: WaitForQuietParams =
        serde_json::from_value(json!({})).expect("wait_for_quiet defaults");
    assert_eq!(quiet.quiet_ms, 250);
    assert_eq!(quiet.timeout_ms, 5000);
    assert!(!quiet.include_image);
    assert_eq!(quiet.after_action, None);
}

#[test]
fn subscribe_events_defaults_to_all_filterable_kinds() {
    let params: SubscribeEventsParams = serde_json::from_value(json!({})).expect("defaults");
    assert_eq!(params.kinds, EventKind::SUBSCRIBABLE.to_vec());
    assert_eq!(params.window_id, None);
    roundtrip(&SubscribeEventsParams::default());
}

#[test]
fn inspect_defaults() {
    let capture: InspectCaptureParams = serde_json::from_value(json!({})).expect("defaults");
    assert_eq!(
        capture.overlays,
        vec![
            OverlayKind::WindowIds,
            OverlayKind::Focus,
            OverlayKind::Damage
        ]
    );

    let subscribe: InspectSubscribeParams =
        serde_json::from_value(json!({"overlays": ["damage"]})).expect("min_interval default");
    assert_eq!(subscribe.min_interval_ms, 100);

    // overlays are required for inspect_subscribe (§5.7)
    assert!(serde_json::from_value::<InspectSubscribeParams>(json!({})).is_err());
}

#[test]
fn launch_app_args_default_to_empty() {
    let params: LaunchAppParams =
        serde_json::from_value(json!({"app_id": "org.mozilla.firefox"})).expect("defaults");
    assert!(params.args.is_empty());
    assert_eq!(wire(&params), json!({"app_id": "org.mozilla.firefox", "args": []}));
}

#[test]
fn window_info_matches_protocol_example() {
    assert_eq!(
        wire(&window_info()),
        json!({
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
        })
    );
    roundtrip(&window_info());
}

#[test]
fn app_info_includes_additive_core_fields() {
    let value = wire(&app_info());
    assert_eq!(value["id"], json!("org.mozilla.firefox"));
    assert_eq!(value["name"], json!("Firefox"));
    assert_eq!(value["terminal"], json!(false));
    assert_eq!(value["categories"], json!(["Network"]));
    // `no_display` / `try_exec` are additive over the §4 example (§7).
    assert_eq!(value["no_display"], json!(false));
    assert_eq!(value["try_exec"], json!(null));
    roundtrip(&app_info());
}

#[test]
fn surface_commit_data_matches_protocol_example() {
    let data = SurfaceCommitEvent {
        window_id: WindowId(17),
        commit_seq: 8291,
        damage: damage(),
    };
    assert_eq!(
        wire(&data),
        json!({
            "window_id": 17,
            "commit_seq": 8291,
            "damage": [{"x": 630, "y": 220, "w": 410, "h": 180}]
        })
    );
    roundtrip(&data);
}

#[test]
fn proto_error_maps_to_agp_codes() {
    assert_eq!(
        ProtoError::UnknownMethod("bogus".to_owned()).error_code(),
        ErrorCode::UnknownMethod
    );
    assert_eq!(
        ProtoError::Malformed("bad".to_owned()).error_code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        ProtoError::VersionMismatch {
            expected: 1,
            got: 2
        }
        .error_code(),
        ErrorCode::ProtocolVersionMismatch
    );
    assert_eq!(
        ProtoError::InvalidParams {
            method: "click".to_owned(),
            message: "missing window_id".to_owned()
        }
        .error_code(),
        ErrorCode::InvalidRequest
    );
    let core_error: adesk_core::Error = ProtoError::Malformed("bad".to_owned()).into();
    assert_eq!(core_error.code, ErrorCode::InvalidRequest);
}

#[test]
fn response_outcome_accessors() {
    let ok = ResponseOutcome::Result(ResultPayload(json!({"action_id": 582})));
    assert!(!ok.is_error());
    assert!(ok.error_payload().is_none());
    assert_eq!(
        ok.result_payload().expect("result").as_value(),
        &json!({"action_id": 582})
    );

    let err = ResponseOutcome::Error(ErrorPayload::new(ErrorCode::Timeout, "late"));
    assert!(err.is_error());
    assert!(err.result_payload().is_none());
    assert_eq!(err.error_payload().expect("error").code, ErrorCode::Timeout);
}

#[test]
fn frame_constructors_and_conversions() {
    let request = RequestFrame::new(1, Method::Ping(PingParams {}));
    assert_eq!(request.id, 1);
    assert!(matches!(Frame::from(request), Frame::Request(_)));

    let response = ResponseFrame::error(2, ErrorPayload::new(ErrorCode::Internal, "boom"));
    assert!(matches!(Frame::from(response), Frame::Response(_)));

    let event = EventFrame::new(
        EventKind::Quiet,
        3,
        4,
        EventPayload::Quiet(QuietEvent {
            window_id: None,
            quiet_ms: 250,
        }),
    );
    assert_eq!(event.seq, 3);
    assert!(matches!(Frame::from(event), Frame::Event(_)));
}

#[test]
fn params_and_results_round_trip() {
    roundtrip(&PingParams {});
    roundtrip(&PingResult {
        protocol_version: 1,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 12,
        renderer: RendererKind::Gl,
        output: Size::new(1280, 800),
    });

    roundtrip(&ListAppsParams {
        query: Some("fire".to_owned()),
        include_hidden: true,
    });
    roundtrip(&ListAppsResult {
        apps: vec![app_info()],
    });
    roundtrip(&GetAppParams {
        app_id: AppId::from("org.mozilla.firefox"),
    });
    roundtrip(&GetAppResult { app: app_info() });
    roundtrip(&LaunchAppParams {
        app_id: AppId::from("org.mozilla.firefox"),
        args: vec!["--new-window".to_owned()],
    });
    roundtrip(&LaunchAppResult {
        launch_id: 7.into(),
        app_id: AppId::from("org.mozilla.firefox"),
        pid: Some(4242),
    });

    roundtrip(&ListWindowsParams {});
    roundtrip(&ListWindowsResult {
        windows: vec![window_info()],
        active_window_id: Some(WindowId(17)),
    });
    roundtrip(&GetWindowParams {
        window_id: WindowId(17),
    });
    roundtrip(&GetWindowResult {
        window: window_info(),
    });
    roundtrip(&ActivateWindowParams {
        window_id: WindowId(17),
    });
    roundtrip(&CloseWindowParams {
        window_id: WindowId(17),
    });
    roundtrip(&GetFocusParams {});
    roundtrip(&GetFocusResult {
        window_id: Some(WindowId(17)),
        surface_focus: true,
    });

    roundtrip(&CaptureWindowParams {
        window_id: WindowId(17),
        region: Some(Rect::new(10, 20, 30, 40)),
        max_dimension: Some(800),
        format: ImageFormat::Rgba8,
    });
    roundtrip(&CaptureRegionParams {
        window_id: WindowId(17),
        region: Rect::new(10, 20, 30, 40),
        max_dimension: None,
        format: ImageFormat::Png,
    });
    roundtrip(&CaptureResult {
        image: image_payload(),
        window: window_info(),
        commit_seq: 8291,
        changed_regions: vec![Rect::new(630, 220, 410, 180)],
    });
    roundtrip(&ObserveParams {
        window_id: Some(WindowId(17)),
        after_action: Some(ActionId(582)),
        until: Condition::Quiet { quiet_ms: 250 },
        timeout_ms: 5000,
        include_image: true,
        max_dimension: None,
        region: None,
    });
    roundtrip(&WaitForChangeParams {
        window_id: None,
        since_commit: Some(800),
        timeout_ms: 5000,
        include_image: false,
    });
    roundtrip(&WaitForQuietParams {
        window_id: Some(WindowId(17)),
        quiet_ms: 250,
        timeout_ms: 5000,
        after_action: None,
        include_image: false,
    });

    roundtrip(&PointerMoveParams {
        window_id: WindowId(17),
        position: Position::normalized(0.5, 0.5),
    });
    roundtrip(&ClickParams {
        window_id: WindowId(17),
        position: Some(Position::pixels(1, 2)),
        button: Button::Right,
        count: 2,
    });
    roundtrip(&DoubleClickParams {
        window_id: WindowId(17),
        position: None,
        button: Button::Left,
    });
    roundtrip(&MouseDownParams {
        window_id: WindowId(17),
        position: None,
        button: Button::Middle,
    });
    roundtrip(&MouseUpParams {
        window_id: WindowId(17),
        position: None,
        button: Button::Side,
    });
    roundtrip(&ScrollParams {
        window_id: WindowId(17),
        position: None,
        dx: 0.0,
        dy: -3.0,
    });
    roundtrip(&DragParams {
        window_id: WindowId(17),
        from: Position::pixels(1, 2),
        to: Position::normalized(0.9, 0.1),
        button: Button::Left,
        duration_ms: 150,
    });
    roundtrip(&KeypressParams {
        keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
        window_id: None,
    });
    roundtrip(&KeyDownParams {
        key: "a".to_owned(),
        window_id: Some(WindowId(17)),
    });
    roundtrip(&KeyUpParams {
        key: "RETURN".to_owned(),
        window_id: None,
    });
    roundtrip(&TypeTextParams {
        text: "hello".to_owned(),
        window_id: None,
    });
    roundtrip(&TypeTextResult {
        action_id: ActionId(1),
        skipped: vec!["\u{1f600}".to_owned()],
    });

    roundtrip(&SubscribeEventsParams::default());
    roundtrip(&SubscribeEventsResult { subscription_id: 3 });
    roundtrip(&UnsubscribeEventsParams { subscription_id: 3 });
    roundtrip(&UnsubscribeEventsResult {});

    roundtrip(&InspectCaptureParams::default());
    roundtrip(&InspectCaptureResult {
        image: image_payload(),
    });
    roundtrip(&InspectSubscribeParams {
        overlays: vec![OverlayKind::Damage],
        min_interval_ms: 100,
    });
    roundtrip(&InspectSubscribeResult { subscription_id: 4 });

    roundtrip(&ActionResult {
        action_id: ActionId(582),
    });
    roundtrip(&observation());
}
