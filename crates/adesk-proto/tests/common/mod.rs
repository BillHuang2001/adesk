//! Shared helpers and fixtures for the `adesk-proto` integration tests.
//!
//! Each integration test file is compiled as its own crate, so this module is
//! compiled once per test binary; `#![allow(dead_code)]` silences the
//! per-target warnings for the helpers a given binary does not use.

#![allow(dead_code)]

use adesk_core::{
    ActionId, AppId, AppInfo, Observation, Rect, Region, Size, WindowId, WindowInfo, WindowState,
};
use adesk_proto::{ImageFormat, ImagePayload, PingResult, RendererKind};
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
