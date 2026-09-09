//! Shared event fixtures for the observer integration tests.
//!
//! Events are built by hand so every test controls `seq` and `ts_ms` exactly;
//! combined with `tokio::time::pause()` that makes waits deterministic.
#![allow(dead_code)]

use adesk_core::{AppId, LaunchId, Rect, Region, RuntimeEvent, WindowId};

/// A window-relative rectangle.
pub fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

/// A region built from rectangles, in order.
pub fn region(rects: &[Rect]) -> Region {
    let mut region = Region::empty();
    for rect in rects {
        region.push(*rect);
    }
    region
}

/// `WindowCreated` for `id`.
pub fn created(seq: u64, ts_ms: u64, id: u64) -> RuntimeEvent {
    RuntimeEvent::WindowCreated {
        seq,
        ts_ms,
        window_id: WindowId(id),
        app_id: None,
        pid: None,
        launch_id: None,
        title: None,
    }
}

/// `WindowDestroyed` for `id`.
pub fn destroyed(seq: u64, ts_ms: u64, id: u64) -> RuntimeEvent {
    RuntimeEvent::WindowDestroyed {
        seq,
        ts_ms,
        window_id: WindowId(id),
    }
}

/// `WindowActivated` for `id`.
pub fn activated(seq: u64, ts_ms: u64, id: u64, previous: Option<u64>) -> RuntimeEvent {
    RuntimeEvent::WindowActivated {
        seq,
        ts_ms,
        window_id: WindowId(id),
        previous: previous.map(WindowId),
    }
}

/// `TitleChanged` for `id`.
pub fn title_changed(seq: u64, ts_ms: u64, id: u64, title: Option<&str>) -> RuntimeEvent {
    RuntimeEvent::TitleChanged {
        seq,
        ts_ms,
        window_id: WindowId(id),
        title: title.map(str::to_owned),
    }
}

/// `SurfaceCommit` on `id` with the given per-window `commit_seq` and damage.
pub fn commit(seq: u64, ts_ms: u64, id: u64, commit_seq: u64, damage: &[Rect]) -> RuntimeEvent {
    RuntimeEvent::SurfaceCommit {
        seq,
        ts_ms,
        window_id: WindowId(id),
        commit_seq,
        damage: region(damage),
    }
}

/// `FocusChanged` to `id` (or to nothing when `None`).
pub fn focus(seq: u64, ts_ms: u64, id: Option<u64>) -> RuntimeEvent {
    RuntimeEvent::FocusChanged {
        seq,
        ts_ms,
        window_id: id.map(WindowId),
    }
}

/// `PopupAppeared` on `id`.
pub fn popup_appeared(seq: u64, ts_ms: u64, id: u64, popup_id: u64) -> RuntimeEvent {
    RuntimeEvent::PopupAppeared {
        seq,
        ts_ms,
        window_id: WindowId(id),
        popup_id,
    }
}

/// `PopupDisappeared` on `id`.
pub fn popup_disappeared(seq: u64, ts_ms: u64, id: u64, popup_id: u64) -> RuntimeEvent {
    RuntimeEvent::PopupDisappeared {
        seq,
        ts_ms,
        window_id: WindowId(id),
        popup_id,
    }
}

/// `AppLaunched` (never counted by the observer, only advances the watermark).
pub fn app_launched(seq: u64, ts_ms: u64, launch_id: u64) -> RuntimeEvent {
    RuntimeEvent::AppLaunched {
        seq,
        ts_ms,
        launch_id: LaunchId(launch_id),
        app_id: AppId::from("test.app"),
        pid: Some(4242),
    }
}
