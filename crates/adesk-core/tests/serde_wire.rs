//! Wire-format tests: exact AGP JSON shapes and serde round-trips.
//!
//! These pin the contract that `adesk-proto`, `adesk-server` and `adesk-client`
//! rely on, using the examples from `docs/protocol.md`.

use adesk_core::{
    ActionId, AppId, AppInfo, Button, ImageBuffer, LaunchId, Observation, OverlayKind, Point,
    Position, Rect, Region, RuntimeEvent, Size, WindowId, WindowInfo, WindowState,
};
use serde_json::json;

fn sample_rect() -> Rect {
    Rect {
        x: 630,
        y: 220,
        w: 410,
        h: 180,
    }
}

fn sample_window_info() -> WindowInfo {
    WindowInfo {
        id: WindowId(17),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        title: Some("GitHub".into()),
        geometry: Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        },
        state: WindowState::Active,
        mapped: true,
        pid: Some(4242),
        created_seq: 800,
        last_commit_seq: 8291,
        popup_count: 0,
    }
}

fn sample_app_info() -> AppInfo {
    AppInfo {
        id: AppId::from("org.mozilla.firefox"),
        name: "Firefox".into(),
        icon: Some("firefox".into()),
        exec: Some("/usr/bin/firefox %u".into()),
        terminal: false,
        categories: vec!["Network".into()],
        startup_wm_class: Some("firefox".into()),
        dbus_activatable: false,
        hidden: false,
        no_display: false,
        try_exec: None,
    }
}

fn sample_observation() -> Observation {
    Observation {
        window_id: Some(WindowId(17)),
        after_action: Some(ActionId(582)),
        commits: 3,
        changed_regions: vec![sample_rect()],
        focus_changed: None,
        title_changed: false,
        new_windows: vec![WindowId(18)],
        destroyed_windows: Vec::new(),
        popups_appeared: vec![7],
        popups_disappeared: Vec::new(),
        elapsed_ms: 417,
        quiet: true,
        timed_out: false,
        last_commit_seq: 8291,
        seq: 8300,
    }
}

#[test]
fn geometry_json_shapes() {
    assert_eq!(
        serde_json::to_value(Point { x: 1, y: -2 }).unwrap(),
        json!({"x": 1, "y": -2})
    );
    assert_eq!(
        serde_json::to_value(Size { w: 3, h: 4 }).unwrap(),
        json!({"w": 3, "h": 4})
    );
    assert_eq!(
        serde_json::to_value(sample_rect()).unwrap(),
        json!({"x": 630, "y": 220, "w": 410, "h": 180})
    );
    assert_eq!(
        serde_json::from_value::<Rect>(json!({"x": 630, "y": 220, "w": 410, "h": 180})).unwrap(),
        sample_rect()
    );
}

#[test]
fn region_serializes_as_a_rect_list() {
    let mut region = Region::empty();
    region.push(Rect {
        x: 0,
        y: 0,
        w: 10,
        h: 10,
    });
    region.push(Rect {
        x: 20,
        y: 20,
        w: 5,
        h: 5,
    });
    let value = serde_json::to_value(&region).unwrap();
    assert_eq!(
        value,
        json!([
            {"x": 0, "y": 0, "w": 10, "h": 10},
            {"x": 20, "y": 20, "w": 5, "h": 5}
        ])
    );
    assert_eq!(serde_json::from_value::<Region>(value).unwrap(), region);
    assert_eq!(serde_json::to_value(Region::empty()).unwrap(), json!([]));
}

#[test]
fn position_json_is_a_tagged_union() {
    assert_eq!(
        serde_json::to_value(Position::Pixels(Point { x: 100, y: 50 })).unwrap(),
        json!({"type": "pixels", "x": 100, "y": 50})
    );
    assert_eq!(
        serde_json::to_value(Position::Normalized { x: 0.72, y: 0.41 }).unwrap(),
        json!({"type": "normalized", "x": 0.72, "y": 0.41})
    );
    assert_eq!(
        serde_json::from_value::<Position>(json!({"type": "pixels", "x": 100, "y": 50})).unwrap(),
        Position::Pixels(Point { x: 100, y: 50 })
    );
    assert_eq!(
        serde_json::from_value::<Position>(json!({"type": "normalized", "x": 0.72, "y": 0.41}))
            .unwrap(),
        Position::Normalized { x: 0.72, y: 0.41 }
    );
}

#[test]
fn ids_are_transparent() {
    assert_eq!(serde_json::to_value(WindowId(17)).unwrap(), json!(17));
    assert_eq!(serde_json::to_value(ActionId(582)).unwrap(), json!(582));
    assert_eq!(serde_json::to_value(LaunchId(3)).unwrap(), json!(3));
    assert_eq!(
        serde_json::to_value(AppId::from("org.mozilla.firefox")).unwrap(),
        json!("org.mozilla.firefox")
    );
    assert_eq!(
        serde_json::from_value::<AppId>(json!("code")).unwrap(),
        AppId::from("code")
    );
}

#[test]
fn window_info_json_matches_protocol() {
    let value = serde_json::to_value(sample_window_info()).unwrap();
    assert_eq!(
        value,
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
    assert_eq!(
        serde_json::from_value::<WindowInfo>(value).unwrap(),
        sample_window_info()
    );

    let nulls = serde_json::to_value(WindowInfo {
        app_id: None,
        title: None,
        pid: None,
        ..sample_window_info()
    })
    .unwrap();
    assert!(nulls["app_id"].is_null());
    assert!(nulls["title"].is_null());
    assert!(nulls["pid"].is_null());
}

#[test]
fn app_info_json_matches_protocol() {
    let value = serde_json::to_value(sample_app_info()).unwrap();
    assert_eq!(
        value,
        json!({
            "id": "org.mozilla.firefox",
            "name": "Firefox",
            "icon": "firefox",
            "exec": "/usr/bin/firefox %u",
            "terminal": false,
            "categories": ["Network"],
            "startup_wm_class": "firefox",
            "dbus_activatable": false,
            "hidden": false,
            "no_display": false,
            "try_exec": null
        })
    );
    assert_eq!(
        serde_json::from_value::<AppInfo>(value).unwrap(),
        sample_app_info()
    );
}

#[test]
fn observation_json_matches_protocol() {
    let value = serde_json::to_value(sample_observation()).unwrap();
    assert_eq!(
        value,
        json!({
            "window_id": 17,
            "after_action": 582,
            "commits": 3,
            "changed_regions": [{"x": 630, "y": 220, "w": 410, "h": 180}],
            "focus_changed": null,
            "title_changed": false,
            "new_windows": [18],
            "destroyed_windows": [],
            "popups_appeared": [7],
            "popups_disappeared": [],
            "elapsed_ms": 417,
            "quiet": true,
            "timed_out": false,
            "last_commit_seq": 8291,
            "seq": 8300
        })
    );
    assert_eq!(
        serde_json::from_value::<Observation>(value).unwrap(),
        sample_observation()
    );
}

#[test]
fn runtime_event_json_is_internally_tagged() {
    let event = RuntimeEvent::SurfaceCommit {
        seq: 8291,
        ts_ms: 51234,
        window_id: WindowId(17),
        commit_seq: 8291,
        damage: Region::from_rect(sample_rect()),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], json!("surface_commit"));
    assert_eq!(value["seq"], json!(8291));
    assert_eq!(value["ts_ms"], json!(51234));
    assert_eq!(value["window_id"], json!(17));
    assert_eq!(value["commit_seq"], json!(8291));
    assert_eq!(
        value["damage"],
        json!([{"x": 630, "y": 220, "w": 410, "h": 180}])
    );
    assert_eq!(
        serde_json::from_value::<RuntimeEvent>(value).unwrap(),
        event
    );
}

#[test]
fn every_runtime_event_variant_round_trips_with_its_tag() {
    let events = vec![
        (
            RuntimeEvent::WindowCreated {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                app_id: Some(AppId::from("app")),
                pid: Some(42),
                launch_id: Some(LaunchId(7)),
                title: Some("t".into()),
            },
            "window_created",
        ),
        (
            RuntimeEvent::WindowDestroyed {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
            },
            "window_destroyed",
        ),
        (
            RuntimeEvent::WindowActivated {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                previous: None,
            },
            "window_activated",
        ),
        (
            RuntimeEvent::TitleChanged {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                title: None,
            },
            "title_changed",
        ),
        (
            RuntimeEvent::SurfaceCommit {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                commit_seq: 5,
                damage: Region::empty(),
            },
            "surface_commit",
        ),
        (
            RuntimeEvent::FocusChanged {
                seq: 1,
                ts_ms: 2,
                window_id: None,
            },
            "focus_changed",
        ),
        (
            RuntimeEvent::PopupAppeared {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                popup_id: 9,
            },
            "popup_appeared",
        ),
        (
            RuntimeEvent::PopupDisappeared {
                seq: 1,
                ts_ms: 2,
                window_id: WindowId(1),
                popup_id: 9,
            },
            "popup_disappeared",
        ),
        (
            RuntimeEvent::AppLaunched {
                seq: 1,
                ts_ms: 2,
                launch_id: LaunchId(7),
                app_id: AppId::from("app"),
                pid: Some(42),
            },
            "app_launched",
        ),
    ];
    for (event, tag) in events {
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], json!(tag));
        assert_eq!(
            serde_json::from_value::<RuntimeEvent>(value).unwrap(),
            event
        );
    }
}

#[test]
fn enum_wire_names() {
    assert_eq!(
        serde_json::to_value(Button::Middle).unwrap(),
        json!("middle")
    );
    assert_eq!(serde_json::to_value(Button::Side).unwrap(), json!("side"));

    let overlays = [
        (OverlayKind::WindowIds, "window_ids"),
        (OverlayKind::AppIds, "app_ids"),
        (OverlayKind::Focus, "focus"),
        (OverlayKind::Damage, "damage"),
        (OverlayKind::SurfaceBounds, "surface_bounds"),
        (OverlayKind::Cursor, "cursor"),
        (OverlayKind::Actions, "actions"),
        (OverlayKind::CommitTiming, "commit_timing"),
    ];
    for (kind, name) in overlays {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<OverlayKind>(json!(name)).unwrap(),
            kind
        );
    }
}

#[test]
fn image_buffer_is_not_wire_facing_but_is_constructible_from_raw_data() {
    // The protocol carries base64 payloads in adesk-proto; core only validates
    // and exposes pixels.
    let buf = ImageBuffer::from_rgba(1, 1, vec![10, 20, 30, 255]).unwrap();
    assert_eq!(buf.pixel(0, 0), Some([10, 20, 30, 255]));
    assert_eq!(buf.stride, 4);
}
