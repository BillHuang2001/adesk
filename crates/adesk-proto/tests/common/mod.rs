//! Shared helpers and fixtures for the `adesk-proto` integration tests.
//!
//! Each integration test file is compiled as its own crate, so this module is
//! compiled once per test binary; `#![allow(dead_code)]` silences the
//! per-target warnings for the helpers a given binary does not use.

#![allow(dead_code)]

use std::collections::BTreeMap;

use adesk_core::{
    AccessibleId, AccessibleMatch, AccessibleNode, AccessibleState, AccessibleTree, ActionId,
    AppId, AppInfo, NotificationId, Observation, Rect, Region, Size, WindowId, WindowInfo,
    WindowState,
};
use adesk_proto::{
    ImageFormat, ImagePayload, Notification, NotificationAction, NotificationUrgency, PingResult,
    RendererKind,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Serializes a value into a [`serde_json::Value`] for golden comparisons.
pub fn wire<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize")
}

/// Asserts that a value survives a JSON round-trip unchanged.
pub fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&back, value, "round-trip mismatch for {json}");
}

/// A `WindowInfo` fixture (§5.3).
pub fn window_info() -> WindowInfo {
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

/// An `AppInfo` fixture (§5.2).
pub fn app_info() -> AppInfo {
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

/// An `ImagePayload` fixture (§4).
pub fn image_payload() -> ImagePayload {
    ImagePayload {
        width: 1280,
        height: 800,
        format: ImageFormat::Rgba8,
        stride: Some(5120),
        data: "AAAA".to_owned(),
        scale: 1.0,
    }
}

/// An `Observation` fixture (§5.4).
pub fn observation() -> Observation {
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

/// A non-empty damage region fixture (§2).
pub fn damage() -> Region {
    let mut region = Region::empty();
    region.push(Rect::new(630, 220, 410, 180));
    region
}

/// A `Notification` fixture (§4/§5.9).
pub fn notification() -> Notification {
    Notification {
        id: NotificationId(5),
        source: Some("adesk-agent".to_owned()),
        title: "Build finished".to_owned(),
        body: "workspace compiled".to_owned(),
        urgency: NotificationUrgency::Critical,
        category: Some("progress".to_owned()),
        actions: vec![NotificationAction {
            key: "open".to_owned(),
            label: "Open".to_owned(),
        }],
        hints: BTreeMap::from([("sender-pid".to_owned(), "4242".to_owned())]),
        posted_seq: 900,
        posted_ts_ms: 1234,
        dismissed: false,
        closed_seq: None,
        close_reason: None,
        timeout_ms: Some(5000),
    }
}

/// An `AccessibleNode` fixture (§5.11): a root frame with one child button.
pub fn accessible_node() -> AccessibleNode {
    AccessibleNode {
        id: AccessibleId(1),
        role: "frame".to_owned(),
        name: "Document".to_owned(),
        description: None,
        value: None,
        states: vec![AccessibleState::Showing],
        bounds: Some(Rect::new(0, 0, 1280, 800)),
        actions: Vec::new(),
        children: vec![AccessibleNode {
            id: AccessibleId(3),
            role: "push_button".to_owned(),
            name: "Save".to_owned(),
            description: Some("Save the document".to_owned()),
            value: Some("Save".to_owned()),
            states: vec![
                AccessibleState::Enabled,
                AccessibleState::Focusable,
                AccessibleState::Showing,
            ],
            bounds: Some(Rect::new(10, 20, 80, 30)),
            actions: vec!["click".to_owned(), "activate".to_owned()],
            children: Vec::new(),
        }],
    }
}

/// An `AccessibleTree` fixture (§4/§5.11).
pub fn accessible_tree() -> AccessibleTree {
    AccessibleTree {
        window_id: WindowId(17),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        app_name: Some("Firefox".to_owned()),
        root: accessible_node(),
        node_count: 2,
        truncated: false,
    }
}

/// An `AccessibleMatch` fixture (§4/§5.11).
pub fn accessible_match() -> AccessibleMatch {
    AccessibleMatch {
        id: AccessibleId(3),
        role: "push_button".to_owned(),
        name: "Save".to_owned(),
        value: Some("Save".to_owned()),
        states: vec![AccessibleState::Enabled, AccessibleState::Focusable],
        bounds: Some(Rect::new(10, 20, 80, 30)),
        actions: vec!["click".to_owned()],
        path: vec!["frame".to_owned(), "toolbar".to_owned()],
    }
}

/// A `PingResult` fixture (§5.1).
pub fn ping_result() -> PingResult {
    PingResult {
        protocol_version: 1,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 5,
        renderer: RendererKind::Gl,
        output: Size::new(1280, 800),
    }
}
