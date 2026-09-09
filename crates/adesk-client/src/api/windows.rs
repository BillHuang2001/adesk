//! AGP §5.3 — window management.

use adesk_core::{ActionId, WindowId, WindowInfo};
use serde::{Deserialize, Serialize};

use crate::api::ActionIdResult;
use crate::{Client, Result};

/// `list_windows` result (protocol §5.3).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct WindowList {
    /// Every known window, in runtime order.
    pub windows: Vec<WindowInfo>,
    /// The active (visible, focused) window, if any.
    #[serde(default)]
    pub active_window_id: Option<WindowId>,
}

/// `get_window` params.
#[derive(Debug, Clone, Serialize)]
struct GetWindowParams {
    window_id: WindowId,
}

/// `get_window` result.
#[derive(Debug, Clone, Deserialize)]
struct GetWindowResult {
    window: WindowInfo,
}

/// `activate_window` / `close_window` params.
#[derive(Debug, Clone, Serialize)]
struct WindowActionParams {
    window_id: WindowId,
}

/// `get_focus` result (protocol §5.3).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct FocusInfo {
    /// Window holding keyboard focus, if any.
    #[serde(default)]
    pub window_id: Option<WindowId>,
    /// Whether a surface currently holds the seat's keyboard focus.
    pub surface_focus: bool,
}

impl Client {
    /// `list_windows` — all windows plus the active one.
    pub async fn list_windows(&self) -> Result<WindowList> {
        self.request("list_windows", &crate::api::NoParams {}).await
    }

    /// `get_window` — one window by id.
    pub async fn get_window(&self, window_id: WindowId) -> Result<WindowInfo> {
        let result: GetWindowResult = self
            .request("get_window", &GetWindowParams { window_id })
            .await?;
        Ok(result.window)
    }

    /// `activate_window` — make a window the single visible, focused toplevel.
    ///
    /// This mutates compositor state directly (keyboard focus + tiling); it is
    /// never synthetic input (protocol §5.3, design invariant 3). Returns the
    /// [`ActionId`] that later observations can reference via `after_action`.
    pub async fn activate_window(&self, window_id: WindowId) -> Result<ActionId> {
        let result: ActionIdResult = self
            .request("activate_window", &WindowActionParams { window_id })
            .await?;
        Ok(result.action_id)
    }

    /// `close_window` — ask the window to close.
    ///
    /// Returns the action id; the window's disappearance is observed via a
    /// `window_destroyed` event or an observation, never assumed.
    pub async fn close_window(&self, window_id: WindowId) -> Result<ActionId> {
        let result: ActionIdResult = self
            .request("close_window", &WindowActionParams { window_id })
            .await?;
        Ok(result.action_id)
    }

    /// `get_focus` — current keyboard focus state.
    pub async fn get_focus(&self) -> Result<FocusInfo> {
        self.request("get_focus", &crate::api::NoParams {}).await
    }
}
