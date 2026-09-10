//! Method-layer wire behavior (§5.1–§5.7): `Method::from_parts`/`params_value`,
//! the `{"method", "params"}` map serde, the spec defaults and the §4
//! `ObserveResult` observation/image split.
//!
//! Frame/codec/event behavior is pinned in `wire.rs` and `codec.rs`.

mod common;

use adesk_core::{
    ActionId, AppId, Button, ErrorCode, LaunchId, OverlayKind, Position, Rect, WindowId,
};
use adesk_proto::*;
use common::*;
use serde_json::{json, Value};
/// One instance of every one of the 29 methods (§5.1–§5.7).
fn methods() -> Vec<Method> {
    vec![
        Method::Ping(PingParams {}),
        Method::ListApps(ListAppsParams {
            query: Some("fire".to_owned()),
            include_hidden: true,
        }),
        Method::GetApp(GetAppParams {
            app_id: AppId::from("org.mozilla.firefox"),
        }),
        Method::LaunchApp(LaunchAppParams {
            app_id: AppId::from("org.mozilla.firefox"),
            args: vec!["--new-window".to_owned()],
        }),
        Method::ListWindows(ListWindowsParams {}),
        Method::GetWindow(GetWindowParams {
            window_id: WindowId(17),
        }),
        Method::ActivateWindow(ActivateWindowParams {
            window_id: WindowId(17),
        }),
        Method::CloseWindow(CloseWindowParams {
            window_id: WindowId(17),
        }),
        Method::GetFocus(GetFocusParams {}),
        Method::CaptureWindow(CaptureWindowParams {
            window_id: WindowId(17),
            region: Some(Rect::new(1, 2, 3, 4)),
            max_dimension: Some(640),
            format: ImageFormat::Rgba8,
        }),
        Method::CaptureRegion(CaptureRegionParams {
            window_id: WindowId(17),
            region: Rect::new(1, 2, 3, 4),
            max_dimension: None,
            format: ImageFormat::Png,
        }),
        Method::Observe(ObserveParams {
            window_id: Some(WindowId(17)),
            after_action: Some(ActionId(582)),
            until: Condition::Quiet { quiet_ms: 250 },
            timeout_ms: 5000,
            include_image: true,
            max_dimension: Some(800),
            region: Some(Rect::new(0, 0, 10, 10)),
        }),
        Method::WaitForChange(WaitForChangeParams {
            window_id: Some(WindowId(17)),
            since_commit: Some(8291),
            timeout_ms: 1000,
            include_image: true,
        }),
        Method::WaitForQuiet(WaitForQuietParams {
            window_id: None,
            quiet_ms: 100,
            timeout_ms: 1000,
            after_action: Some(ActionId(1)),
            include_image: true,
        }),
        Method::PointerMove(PointerMoveParams {
            window_id: WindowId(17),
            position: Position::pixels(100, 50),
        }),
        Method::Click(ClickParams {
            window_id: WindowId(17),
            position: Some(Position::normalized(0.72, 0.41)),
            button: Button::Right,
            count: 2,
        }),
        Method::DoubleClick(DoubleClickParams {
            window_id: WindowId(17),
            position: None,
            button: Button::Left,
        }),
        Method::MouseDown(MouseDownParams {
            window_id: WindowId(17),
            position: Some(Position::pixels(1, 2)),
            button: Button::Middle,
        }),
        Method::MouseUp(MouseUpParams {
            window_id: WindowId(17),
            position: None,
            button: Button::Side,
        }),
        Method::Scroll(ScrollParams {
            window_id: WindowId(17),
            position: None,
            dx: -1.5,
            dy: 3.0,
        }),
        Method::Drag(DragParams {
            window_id: WindowId(17),
            from: Position::pixels(1, 2),
            to: Position::normalized(0.9, 0.1),
            button: Button::Left,
            duration_ms: 150,
        }),
        Method::Keypress(KeypressParams {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            window_id: Some(WindowId(17)),
        }),
        Method::KeyDown(KeyDownParams {
            key: "a".to_owned(),
            window_id: None,
        }),
        Method::KeyUp(KeyUpParams {
            key: "RETURN".to_owned(),
            window_id: Some(WindowId(17)),
        }),
        Method::TypeText(TypeTextParams {
            text: "hello".to_owned(),
            window_id: None,
        }),
        Method::SubscribeEvents(SubscribeEventsParams {
            kinds: vec![EventKind::SurfaceCommit],
            window_id: Some(WindowId(17)),
        }),
        Method::UnsubscribeEvents(UnsubscribeEventsParams { subscription_id: 3 }),
        Method::InspectCapture(InspectCaptureParams {
            overlays: vec![OverlayKind::Damage],
            region: None,
            max_dimension: Some(800),
        }),
        Method::InspectSubscribe(InspectSubscribeParams {
            overlays: vec![OverlayKind::WindowIds],
            min_interval_ms: 100,
        }),
    ]
}

#[test]
fn every_method_round_trips_through_from_parts_and_serde() {
    let methods = methods();
    assert_eq!(methods.len(), 29);
    for method in &methods {
        let name = method.method_name();

        let params = method.params_value().expect("params_value");
        assert!(params.is_object(), "{name}: params must be a JSON object");
        assert_eq!(
            &Method::from_parts(name, params).expect("from_parts"),
            method,
            "{name}: from_parts(params_value()) must round-trip"
        );

        let value = wire(method);
        let object = value.as_object().expect("method serializes as a map");
        assert_eq!(object.len(), 2, "{name}: exactly method + params");
        assert_eq!(value["method"], json!(name));
        assert_eq!(value["params"], method.params_value().unwrap());
        assert_eq!(
            &serde_json::from_value::<Method>(value).expect("deserialize"),
            method,
            "{name}: serde round-trip"
        );
    }
}

#[test]
fn missing_and_null_params_default_to_empty_object() {
    let ping = Method::Ping(PingParams {});
    assert_eq!(Method::from_parts("ping", Value::Null).unwrap(), ping);
    assert_eq!(Method::from_parts("ping", json!({})).unwrap(), ping);
    assert_eq!(
        Method::from_parts("get_focus", Value::Null).unwrap(),
        Method::GetFocus(GetFocusParams {})
    );
    // Both are equivalent through the map serde too.
    assert_eq!(
        serde_json::from_value::<Method>(json!({"method": "ping", "params": null})).unwrap(),
        ping
    );
    assert_eq!(
        serde_json::from_value::<Method>(json!({"method": "ping"})).unwrap(),
        ping
    );
}

#[test]
fn unknown_fields_in_params_and_method_are_ignored() {
    let click = Method::Click(ClickParams {
        window_id: WindowId(17),
        position: None,
        button: Button::Left,
        count: 1,
    });
    assert_eq!(
        Method::from_parts(
            "click",
            json!({"window_id": 17, "future_param": true, "another": [1, 2]})
        )
        .unwrap(),
        click
    );
    assert_eq!(
        serde_json::from_value::<Method>(
            json!({"method": "click", "params": {"window_id": 17, "future_param": true}, "future": 42})
        )
        .unwrap(),
        click
    );
}

#[test]
fn unknown_method_is_rejected() {
    let err = Method::from_parts("bogus", json!({})).unwrap_err();
    assert!(matches!(err, ProtoError::UnknownMethod(ref name) if name == "bogus"));
    assert_eq!(err.error_code(), ErrorCode::UnknownMethod);
    assert!(serde_json::from_value::<Method>(json!({"method": "bogus", "params": {}})).is_err());
    // `method` itself is required.
    assert!(serde_json::from_value::<Method>(json!({"params": {}})).is_err());
}

#[test]
fn invalid_params_are_rejected_with_invalid_params() {
    let cases = [
        ("ping", json!(42)),
        ("ping", json!("ping")),
        ("ping", json!([1])),
        ("click", json!({"window_id": "17"})),
        ("click", json!({"button": "left"})),
        (
            "click",
            json!({"window_id": 17, "position": {"type": "pixels"}}),
        ),
        ("observe", json!({})),
        ("observe", json!({"until": {"type": "bogus"}})),
        ("scroll", json!({"window_id": 17})),
        (
            "drag",
            json!({"window_id": 17, "from": {"type": "pixels", "x": 1, "y": 2}}),
        ),
        ("keypress", json!({})),
        ("keypress", json!({"keys": 5})),
        ("get_window", json!({"window_id": null})),
        ("unsubscribe_events", json!({})),
        ("inspect_subscribe", json!({})),
        ("capture_region", json!({"window_id": 17})),
    ];
    for (name, params) in cases {
        let err = Method::from_parts(name, params.clone()).unwrap_err();
        match &err {
            ProtoError::InvalidParams { method, .. } => assert_eq!(
                method, name,
                "{name} {params}: InvalidParams names the method"
            ),
            other => panic!("{name} {params}: expected InvalidParams, got {other:?}"),
        }
        assert_eq!(err.error_code(), ErrorCode::InvalidRequest);
    }
}

#[test]
fn click_params_match_protocol_example() {
    let method = Method::from_parts(
        "click",
        json!({"window_id": 17, "position": {"type": "normalized", "x": 0.72, "y": 0.41}}),
    )
    .unwrap();
    assert_eq!(
        wire(&method),
        json!({
            "method": "click",
            "params": {
                "window_id": 17,
                "position": {"type": "normalized", "x": 0.72, "y": 0.41},
                "button": "left",
                "count": 1
            }
        })
    );

    // Pixels positions use the same tagged union (§2).
    let pixels = Method::from_parts(
        "click",
        json!({"window_id": 17, "position": {"type": "pixels", "x": 100, "y": 50}}),
    )
    .unwrap();
    assert_eq!(
        wire(&pixels)["params"]["position"],
        json!({"type": "pixels", "x": 100, "y": 50})
    );

    // `position` is optional and omitted when absent.
    let bare = Method::from_parts("click", json!({"window_id": 17})).unwrap();
    assert_eq!(
        wire(&bare)["params"],
        json!({"window_id": 17, "button": "left", "count": 1})
    );
}

#[test]
fn observe_defaults_and_condition_shapes() {
    let method = Method::from_parts("observe", json!({"until": {"type": "change"}})).unwrap();
    assert_eq!(
        wire(&method)["params"],
        json!({"until": {"type": "change"}, "timeout_ms": 5000, "include_image": true})
    );

    let quiet = Method::from_parts(
        "observe",
        json!({"until": {"type": "quiet", "quiet_ms": 250}}),
    )
    .unwrap();
    assert_eq!(
        wire(&quiet)["params"]["until"],
        json!({"type": "quiet", "quiet_ms": 250})
    );

    let timeout = Method::from_parts("observe", json!({"until": {"type": "timeout"}})).unwrap();
    assert_eq!(
        wire(&timeout)["params"]["until"],
        json!({"type": "timeout"})
    );

    // Optional filters are omitted; provided ones survive verbatim.
    let scoped = Method::from_parts(
        "observe",
        json!({
            "window_id": 17,
            "after_action": 582,
            "until": {"type": "timeout"},
            "max_dimension": 800,
            "region": {"x": 1, "y": 2, "w": 3, "h": 4}
        }),
    )
    .unwrap();
    assert_eq!(
        wire(&scoped)["params"],
        json!({
            "window_id": 17,
            "after_action": 582,
            "until": {"type": "timeout"},
            "timeout_ms": 5000,
            "include_image": true,
            "max_dimension": 800,
            "region": {"x": 1, "y": 2, "w": 3, "h": 4}
        })
    );
}

#[test]
fn wait_method_defaults() {
    let change = Method::from_parts("wait_for_change", json!({})).unwrap();
    assert_eq!(
        wire(&change)["params"],
        json!({"timeout_ms": 5000, "include_image": false})
    );
    let quiet = Method::from_parts("wait_for_quiet", json!({})).unwrap();
    assert_eq!(
        wire(&quiet)["params"],
        json!({"quiet_ms": 250, "timeout_ms": 5000, "include_image": false})
    );
    // `include_image` is only defaulted to false, an explicit true survives.
    let with_image = Method::from_parts(
        "wait_for_change",
        json!({"window_id": 17, "since_commit": 8291, "include_image": true}),
    )
    .unwrap();
    assert_eq!(
        wire(&with_image)["params"],
        json!({
            "window_id": 17,
            "since_commit": 8291,
            "timeout_ms": 5000,
            "include_image": true
        })
    );
}

#[test]
fn subscribe_events_defaults_to_all_eleven_filterable_kinds() {
    let method = Method::from_parts("subscribe_events", json!({})).unwrap();
    assert_eq!(
        wire(&method)["params"],
        json!({
            "kinds": [
                "window_created",
                "window_destroyed",
                "window_activated",
                "title_changed",
                "surface_commit",
                "surface_damage",
                "focus_changed",
                "popup_appeared",
                "popup_disappeared",
                "quiet",
                "app_launched"
            ]
        })
    );
    let scoped = Method::from_parts(
        "subscribe_events",
        json!({"kinds": ["surface_commit", "quiet"], "window_id": 17}),
    )
    .unwrap();
    assert_eq!(
        wire(&scoped)["params"],
        json!({"kinds": ["surface_commit", "quiet"], "window_id": 17})
    );
    // `inspect_frame` is not filterable (§5.7) but the wire name still decodes.
    let unknown_kind = Method::from_parts("subscribe_events", json!({"kinds": ["bogus"]}));
    assert!(unknown_kind.is_err());
}

#[test]
fn input_param_defaults() {
    // scroll: `dy` is required, `dx` defaults to 0.0.
    assert!(Method::from_parts("scroll", json!({"window_id": 17})).is_err());
    let scroll = Method::from_parts("scroll", json!({"window_id": 17, "dy": 3.0})).unwrap();
    assert_eq!(
        wire(&scroll)["params"],
        json!({"window_id": 17, "dx": 0.0, "dy": 3.0})
    );

    // drag: `button` and `duration_ms` default (§5.5).
    let drag = Method::from_parts(
        "drag",
        json!({
            "window_id": 17,
            "from": {"type": "pixels", "x": 1, "y": 2},
            "to": {"type": "normalized", "x": 0.9, "y": 0.1}
        }),
    )
    .unwrap();
    assert_eq!(
        wire(&drag)["params"],
        json!({
            "window_id": 17,
            "from": {"type": "pixels", "x": 1, "y": 2},
            "to": {"type": "normalized", "x": 0.9, "y": 0.1},
            "button": "left",
            "duration_ms": 150
        })
    );

    // Button wire names round-trip (§2).
    for button in ["left", "right", "middle", "side", "extra"] {
        let method =
            Method::from_parts("double_click", json!({"window_id": 1, "button": button})).unwrap();
        assert_eq!(wire(&method)["params"]["button"], json!(button));
    }

    // pointer_move takes a required tagged position.
    let pointer = Method::from_parts(
        "pointer_move",
        json!({"window_id": 17, "position": {"type": "pixels", "x": 100, "y": 50}}),
    )
    .unwrap();
    assert_eq!(
        wire(&pointer)["params"],
        json!({"window_id": 17, "position": {"type": "pixels", "x": 100, "y": 50}})
    );
}

#[test]
fn keypress_accepts_chord_and_single_key() {
    let chord =
        Method::from_parts("keypress", json!({"keys": ["CTRL", "L"], "window_id": 17})).unwrap();
    assert_eq!(
        wire(&chord),
        json!({"method": "keypress", "params": {"keys": ["CTRL", "L"], "window_id": 17}})
    );
    match &chord {
        Method::Keypress(params) => {
            assert_eq!(params.keys, KeySpec::Chord(vec!["CTRL".into(), "L".into()]));
            assert_eq!(params.keys.keys(), ["CTRL", "L"]);
        }
        other => panic!("expected keypress, got {other:?}"),
    }

    let single = Method::from_parts("keypress", json!({"keys": "a"})).unwrap();
    assert_eq!(
        wire(&single),
        json!({"method": "keypress", "params": {"keys": "a"}})
    );
    match &single {
        Method::Keypress(params) => {
            assert_eq!(params.keys, KeySpec::Single("a".to_owned()));
            assert_eq!(params.keys.keys(), ["a"]);
        }
        other => panic!("expected keypress, got {other:?}"),
    }

    let down = Method::from_parts("key_down", json!({"key": "RETURN"})).unwrap();
    assert_eq!(
        wire(&down),
        json!({"method": "key_down", "params": {"key": "RETURN"}})
    );
    let up = Method::from_parts("key_up", json!({"key": "RETURN", "window_id": 17})).unwrap();
    assert_eq!(
        wire(&up)["params"],
        json!({"key": "RETURN", "window_id": 17})
    );
}

#[test]
fn type_text_params_and_result() {
    let method = Method::from_parts("type_text", json!({"text": "héllo"})).unwrap();
    assert_eq!(
        wire(&method),
        json!({"method": "type_text", "params": {"text": "héllo"}})
    );
    let scoped = Method::from_parts("type_text", json!({"text": "hi", "window_id": 17})).unwrap();
    assert_eq!(
        wire(&scoped)["params"],
        json!({"text": "hi", "window_id": 17})
    );

    let result = TypeTextResult {
        action_id: ActionId(582),
        skipped: vec!["\u{1f600}".to_owned()],
    };
    assert_eq!(
        wire(&result),
        json!({"action_id": 582, "skipped": ["\u{1f600}"]})
    );
    assert_eq!(
        serde_json::from_value::<TypeTextResult>(wire(&result)).unwrap(),
        result
    );
}

#[test]
fn inspect_defaults_and_capture_shapes() {
    let capture = Method::from_parts("inspect_capture", json!({})).unwrap();
    assert_eq!(
        wire(&capture)["params"],
        json!({"overlays": ["window_ids", "focus", "damage"]})
    );
    let cropped = Method::from_parts(
        "inspect_capture",
        json!({
            "overlays": ["damage", "cursor"],
            "region": {"x": 1, "y": 2, "w": 3, "h": 4},
            "max_dimension": 640
        }),
    )
    .unwrap();
    assert_eq!(
        wire(&cropped)["params"],
        json!({
            "overlays": ["damage", "cursor"],
            "region": {"x": 1, "y": 2, "w": 3, "h": 4},
            "max_dimension": 640
        })
    );

    // `overlays` is required for inspect_subscribe (§5.7).
    assert!(Method::from_parts("inspect_subscribe", json!({})).is_err());
    let subscribe =
        Method::from_parts("inspect_subscribe", json!({"overlays": ["damage"]})).unwrap();
    assert_eq!(
        wire(&subscribe)["params"],
        json!({"overlays": ["damage"], "min_interval_ms": 100})
    );

    let result = InspectCaptureResult {
        image: image_payload(),
    };
    assert_eq!(wire(&result)["image"]["width"], json!(1280));
}

#[test]
fn capture_params_and_results() {
    let window = Method::from_parts("capture_window", json!({"window_id": 17})).unwrap();
    assert_eq!(
        wire(&window)["params"],
        json!({"window_id": 17, "format": "png"})
    );
    let rgba = Method::from_parts(
        "capture_window",
        json!({
            "window_id": 17,
            "region": {"x": 1, "y": 2, "w": 3, "h": 4},
            "max_dimension": 800,
            "format": "rgba8"
        }),
    )
    .unwrap();
    assert_eq!(
        wire(&rgba)["params"],
        json!({
            "window_id": 17,
            "region": {"x": 1, "y": 2, "w": 3, "h": 4},
            "max_dimension": 800,
            "format": "rgba8"
        })
    );

    // `region` is required for capture_region (§5.4).
    assert!(Method::from_parts("capture_region", json!({"window_id": 17})).is_err());
    let region = Method::from_parts(
        "capture_region",
        json!({"window_id": 17, "region": {"x": 1, "y": 2, "w": 3, "h": 4}}),
    )
    .unwrap();
    assert_eq!(
        wire(&region)["params"],
        json!({"window_id": 17, "region": {"x": 1, "y": 2, "w": 3, "h": 4}, "format": "png"})
    );
}

#[test]
fn app_and_window_params() {
    assert_eq!(
        wire(&Method::from_parts("list_apps", json!({})).unwrap())["params"],
        json!({"include_hidden": false})
    );
    assert_eq!(
        wire(
            &Method::from_parts(
                "list_apps",
                json!({"query": "fire", "include_hidden": true})
            )
            .unwrap()
        )["params"],
        json!({"query": "fire", "include_hidden": true})
    );
    assert_eq!(
        wire(&Method::from_parts("get_app", json!({"app_id": "org.mozilla.firefox"})).unwrap())
            ["params"],
        json!({"app_id": "org.mozilla.firefox"})
    );
    // `args` defaults to `[]` (§5.2).
    assert_eq!(
        wire(&Method::from_parts("launch_app", json!({"app_id": "org.mozilla.firefox"})).unwrap())
            ["params"],
        json!({"app_id": "org.mozilla.firefox", "args": []})
    );

    assert_eq!(
        wire(&Method::from_parts("list_windows", json!({})).unwrap())["params"],
        json!({})
    );
    assert_eq!(
        wire(&Method::from_parts("get_focus", json!({})).unwrap())["params"],
        json!({})
    );
    for name in ["get_window", "activate_window", "close_window"] {
        let method = Method::from_parts(name, json!({"window_id": 17})).unwrap();
        assert_eq!(
            wire(&method),
            json!({"method": name, "params": {"window_id": 17}})
        );
    }
    assert_eq!(
        wire(&Method::from_parts("unsubscribe_events", json!({"subscription_id": 3})).unwrap()),
        json!({"method": "unsubscribe_events", "params": {"subscription_id": 3}})
    );
    assert_eq!(
        wire(&Method::from_parts("ping", json!({})).unwrap()),
        json!({"method": "ping", "params": {}})
    );

    // Results mirror §5.2/§5.3.
    let launch = LaunchAppResult {
        launch_id: LaunchId(7),
        app_id: AppId::from("org.mozilla.firefox"),
        pid: Some(4242),
    };
    assert_eq!(
        wire(&launch),
        json!({"launch_id": 7, "app_id": "org.mozilla.firefox", "pid": 4242})
    );
    let focus = GetFocusResult {
        window_id: None,
        surface_focus: false,
    };
    assert_eq!(wire(&focus), json!({"surface_focus": false}));
    assert_eq!(wire(&UnsubscribeEventsResult {}), json!({}));
}

#[test]
fn observe_result_wire_shape_and_round_trip() {
    let result = ObserveResult {
        observation: observation(),
        image: Some(image_payload()),
    };
    let value = wire(&result);
    // Exactly one top-level key: `observation` (§4).
    assert_eq!(value.as_object().expect("object").len(), 1);
    assert_eq!(value["observation"]["commits"], json!(3));
    assert_eq!(value["observation"]["after_action"], json!(582));
    assert_eq!(value["observation"]["focus_changed"], json!(false));
    assert_eq!(value["observation"]["quiet"], json!(true));
    assert_eq!(value["observation"]["elapsed_ms"], json!(417));
    // `image` lives inside `observation` (§4), not beside it.
    assert_eq!(value["observation"]["image"]["width"], json!(1280));
    assert_eq!(value["observation"]["image"]["format"], json!("rgba8"));
    assert_eq!(
        value["observation"]["changed_regions"],
        json!([{"x": 630, "y": 220, "w": 410, "h": 180}])
    );
    assert_eq!(
        serde_json::from_value::<ObserveResult>(value).unwrap(),
        result
    );
}

#[test]
fn observe_result_image_is_null_when_absent() {
    let result = ObserveResult {
        observation: observation(),
        image: None,
    };
    let value = wire(&result);
    assert_eq!(value["observation"]["image"], json!(null));
    assert_eq!(
        serde_json::from_value::<ObserveResult>(value.clone()).unwrap(),
        result
    );

    // A missing `image` key is tolerated as `None` (§4 always sends it).
    let mut stripped = value;
    stripped["observation"]
        .as_object_mut()
        .expect("observation object")
        .remove("image");
    assert_eq!(
        serde_json::from_value::<ObserveResult>(stripped).unwrap(),
        result
    );

    // Malformed shapes are rejected, never panicking.
    assert!(serde_json::from_value::<ObserveResult>(json!({"observation": 3})).is_err());
    assert!(serde_json::from_value::<ObserveResult>(json!({})).is_err());
    assert!(serde_json::from_value::<ObserveResult>(json!({"observation": {}})).is_err());
    assert!(serde_json::from_value::<ObserveResult>(
        json!({"observation": {"image": {"width": "wide"}}})
    )
    .is_err());
}

#[test]
fn result_structs_match_spec_shapes() {
    // §5.1 ping.
    let ping = ping_result();
    assert_eq!(
        wire(&ping),
        json!({
            "protocol_version": 1,
            "runtime_version": "0.1.0",
            "uptime_ms": 5,
            "renderer": "gl",
            "output": {"w": 1280, "h": 800}
        })
    );
    assert!(ping.is_compatible());
    assert_eq!(wire(&RendererKind::Pixman), json!("pixman"));

    // §5.2/§5.3 registry and window results.
    assert_eq!(wire(&ListAppsResult { apps: vec![] }), json!({"apps": []}));
    assert_eq!(
        wire(&GetAppResult { app: app_info() })["app"]["id"],
        json!("org.mozilla.firefox")
    );
    assert_eq!(
        wire(&ListWindowsResult {
            windows: vec![window_info()],
            active_window_id: Some(WindowId(17)),
        })["windows"][0]["id"],
        json!(17)
    );
    assert_eq!(
        wire(&GetWindowResult {
            window: window_info()
        })["window"]["state"],
        json!("active")
    );

    // §5.4 capture result.
    let capture = CaptureResult {
        image: image_payload(),
        window: window_info(),
        commit_seq: 8291,
        changed_regions: vec![Rect::new(630, 220, 410, 180)],
    };
    assert_eq!(
        wire(&capture),
        json!({
            "image": {
                "width": 1280,
                "height": 800,
                "format": "rgba8",
                "stride": 5120,
                "data": "AAAA",
                "scale": 1.0
            },
            "window": {
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
            },
            "commit_seq": 8291,
            "changed_regions": [{"x": 630, "y": 220, "w": 410, "h": 180}]
        })
    );

    // §5.6/§5.7 subscription and action results.
    assert_eq!(
        wire(&SubscribeEventsResult { subscription_id: 3 }),
        json!({"subscription_id": 3})
    );
    assert_eq!(
        wire(&InspectSubscribeResult { subscription_id: 4 }),
        json!({"subscription_id": 4})
    );
    assert_eq!(
        wire(&ActionResult {
            action_id: ActionId(582)
        }),
        json!({"action_id": 582})
    );
}

#[test]
fn request_frame_flattens_method_and_params() {
    // `#[serde(flatten)]` on `RequestFrame::method` must see the same wire map.
    let request = RequestFrame::new(
        1,
        Method::Click(ClickParams {
            window_id: WindowId(17),
            position: Some(Position::normalized(0.72, 0.41)),
            button: Button::Left,
            count: 1,
        }),
    );
    let value = wire(&request);
    assert_eq!(
        value,
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
    assert_eq!(
        serde_json::from_value::<RequestFrame>(value).unwrap(),
        request
    );

    let ping = RequestFrame::new(1, Method::Ping(PingParams {}));
    for line in [
        json!({"id": 1, "method": "ping", "params": {}}),
        json!({"id": 1, "method": "ping"}),
        json!({"id": 1, "method": "ping", "params": null}),
        json!({"id": 1, "method": "ping", "params": {}, "future_field": 42}),
    ] {
        assert_eq!(
            serde_json::from_value::<RequestFrame>(line.clone()).unwrap(),
            ping,
            "line {line}"
        );
    }
}
