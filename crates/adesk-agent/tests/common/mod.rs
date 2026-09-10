//! Fixtures shared by the `adesk-agent` integration-test targets.
//!
//! Each test target pulls this in with `mod common;` and uses only the helpers
//! it needs.

// This module is compiled into several separate integration-test binaries and
// each one uses only a subset of the fixtures, so the unused ones must not trip
// `dead_code` under `-D warnings`.
#![allow(dead_code)]

use adesk_agent::{RuntimeInfo, WindowList, PROTOCOL_VERSION};
use adesk_core::{ActionId, Observation, Rect, Size, WindowId};
use adesk_proto::{ImageFormat, ImagePayload};

/// Runtime identity as a conforming runtime reports it.
pub fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        protocol_version: PROTOCOL_VERSION,
        runtime_version: "0.1.0".to_owned(),
        uptime_ms: 42,
        renderer: "pixman".to_owned(),
        output: Size::new(1280, 800),
    }
}

/// An empty window list (no windows known yet).
pub fn empty_windows() -> WindowList {
    WindowList {
        windows: Vec::new(),
        active_window_id: None,
    }
}

/// A quiet observation of `window_id` causally after `after_action`.
pub fn observation(window_id: Option<WindowId>, after_action: Option<ActionId>) -> Observation {
    Observation {
        window_id,
        after_action,
        commits: 1,
        changed_regions: vec![Rect::new(0, 0, 10, 10)],
        focus_changed: None,
        title_changed: false,
        new_windows: Vec::new(),
        destroyed_windows: Vec::new(),
        popups_appeared: Vec::new(),
        popups_disappeared: Vec::new(),
        elapsed_ms: 12,
        quiet: true,
        timed_out: false,
        last_commit_seq: 3,
        seq: 9,
    }
}

/// A payload carrying only dimensions: the agent never inspects pixels.
pub fn image(width: u32, height: u32) -> ImagePayload {
    ImagePayload {
        width,
        height,
        format: ImageFormat::Png,
        stride: None,
        data: String::new(),
        scale: 1.0,
    }
}
