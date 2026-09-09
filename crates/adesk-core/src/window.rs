//! Window descriptions: lifecycle-independent state exposed to agents.

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::ids::{AppId, WindowId};

/// Visibility state of a window under the single-visible-toplevel policy.
///
/// `Active` means the window is the visible one (focused and tiled to the whole
/// virtual output); `Inactive` windows stay mapped but are not shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    /// The visible window.
    Active,
    /// Mapped but not visible.
    #[default]
    Inactive,
}

/// A window as reported by `list_windows` / `get_window` and embedded in
/// capture results.
///
/// Field names match AGP (`docs/protocol.md` §4) and serialize as snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Stable window id.
    pub id: WindowId,
    /// Desktop-file id of the owning application, once correlated.
    pub app_id: Option<AppId>,
    /// Current toplevel title.
    pub title: Option<String>,
    /// Window-relative geometry in output pixels.
    pub geometry: Rect,
    /// Visibility state under the tiling policy.
    pub state: WindowState,
    /// Whether the toplevel is currently mapped.
    pub mapped: bool,
    /// Process id of the client, when known.
    pub pid: Option<i32>,
    /// Global event sequence at which the window was created.
    pub created_seq: u64,
    /// Per-surface-tree commit counter of the last observed commit.
    pub last_commit_seq: u64,
    /// Number of currently mapped popups belonging to this window.
    pub popup_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn info() -> WindowInfo {
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

    #[test]
    fn window_state_default_is_inactive() {
        assert_eq!(WindowState::default(), WindowState::Inactive);
    }

    #[test]
    fn window_info_round_trips() {
        let json = serde_json::to_string(&info()).unwrap();
        assert_eq!(serde_json::from_str::<WindowInfo>(&json).unwrap(), info());
    }

    #[test]
    fn window_state_wire_names() {
        assert_eq!(
            serde_json::to_string(&WindowState::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&WindowState::Inactive).unwrap(),
            "\"inactive\""
        );
    }
}
