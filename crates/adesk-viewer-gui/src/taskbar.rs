//! The pure task-bar view model derived from a VAP `DesktopState`.
//!
//! The viewer's task bar lists the runtime's windows and highlights the active
//! (visible) one so a human can switch between them
//! (`docs/viewer.md` §3, §5). This module owns the projection from
//! `adesk_viewer_proto::DesktopState` to a flat list of rows — no GTK, so it is
//! unit-testable without a display.

use adesk_core::{WindowId, WindowInfo};
use adesk_viewer_proto::DesktopState;

/// One row of the viewer task bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBarEntry {
    /// The window this entry switches to (the runtime-native `activate_window`
    /// target).
    pub window_id: WindowId,
    /// Human-readable label: the toplevel title, else the desktop-file id, else
    /// a `window <id>` fallback.
    pub label: String,
    /// Whether this window is the currently active (visible) one.
    pub active: bool,
}

/// Builds the task-bar entries from a full [`DesktopState`].
///
/// One entry is produced per `state.windows`, in the same order; the `active`
/// flag is set from `state.active_window_id`.
#[must_use]
pub fn entries(state: &DesktopState) -> Vec<TaskBarEntry> {
    state
        .windows
        .iter()
        .map(|window| TaskBarEntry {
            window_id: window.id,
            label: label_for(window),
            active: state.active_window_id == Some(window.id),
        })
        .collect()
}

/// Updates the `active` flags of `entries` from a frame's `active_window_id`,
/// without a full state refresh.
///
/// This is the refresh-on-frame path: the window list rarely changes but the
/// active window can change in any frame.
pub fn set_active(entries: &mut [TaskBarEntry], active: Option<WindowId>) {
    for entry in entries {
        entry.active = active == Some(entry.window_id);
    }
}

/// Chooses a row label for `window`: the title, else the app id, else a
/// `window <id>` fallback.
fn label_for(window: &WindowInfo) -> String {
    if let Some(title) = window.title.as_deref().filter(|title| !title.is_empty()) {
        return title.to_owned();
    }
    if let Some(app_id) = window.app_id.as_ref().filter(|app_id| !app_id.0.is_empty()) {
        return app_id.0.clone();
    }
    format!("window {}", window.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_core::{AppId, Rect, WindowState};

    /// A window fixture with the given id, title, app id and state.
    fn window(
        id: u64,
        title: Option<&str>,
        app_id: Option<&str>,
        state: WindowState,
    ) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: app_id.map(AppId::from),
            title: title.map(str::to_owned),
            geometry: Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            state,
            mapped: true,
            pid: Some(4242),
            created_seq: 1,
            last_commit_seq: 2,
            popup_count: 0,
        }
    }

    #[test]
    fn labels_prefer_the_title() {
        let state = DesktopState {
            active_window_id: Some(WindowId(1)),
            windows: vec![window(
                1,
                Some("Files"),
                Some("org.gnome.Nautilus"),
                WindowState::Active,
            )],
        };
        let rows = entries(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Files");
    }

    #[test]
    fn labels_fall_back_to_the_app_id() {
        let state = DesktopState {
            active_window_id: None,
            windows: vec![window(
                2,
                None,
                Some("org.gnome.Nautilus"),
                WindowState::Inactive,
            )],
        };
        let rows = entries(&state);
        assert_eq!(rows[0].label, "org.gnome.Nautilus");
    }

    #[test]
    fn labels_fall_back_to_the_window_id() {
        let state = DesktopState {
            active_window_id: None,
            windows: vec![window(7, None, None, WindowState::Inactive)],
        };
        let rows = entries(&state);
        assert_eq!(rows[0].label, "window 7");
    }

    #[test]
    fn an_empty_title_and_app_id_use_the_fallback() {
        let state = DesktopState {
            active_window_id: None,
            windows: vec![window(9, Some(""), Some(""), WindowState::Inactive)],
        };
        let rows = entries(&state);
        assert_eq!(rows[0].label, "window 9");
    }

    #[test]
    fn the_active_window_is_flagged() {
        let state = DesktopState {
            active_window_id: Some(WindowId(3)),
            windows: vec![
                window(1, Some("one"), None, WindowState::Inactive),
                window(3, Some("three"), None, WindowState::Active),
            ],
        };
        let rows = entries(&state);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].window_id, WindowId(1));
        assert!(!rows[0].active);
        assert_eq!(rows[1].window_id, WindowId(3));
        assert!(rows[1].active);
    }

    #[test]
    fn set_active_flips_the_flags() {
        let state = DesktopState {
            active_window_id: Some(WindowId(1)),
            windows: vec![
                window(1, Some("one"), None, WindowState::Active),
                window(2, Some("two"), None, WindowState::Inactive),
            ],
        };
        let mut rows = entries(&state);
        set_active(&mut rows, Some(WindowId(2)));
        assert!(!rows[0].active);
        assert!(rows[1].active);

        set_active(&mut rows, None);
        assert!(!rows[0].active);
        assert!(!rows[1].active);
    }

    #[test]
    fn an_empty_state_yields_no_entries() {
        let state = DesktopState {
            active_window_id: None,
            windows: Vec::new(),
        };
        assert!(entries(&state).is_empty());
    }
}
