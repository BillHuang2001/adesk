//! Window management methods (§5.3). Action methods answer [`ActionResult`](crate::ActionResult).

use adesk_core::{WindowId, WindowInfo};
use serde::{Deserialize, Serialize};

/// Params of `list_windows` (§5.3) — the empty object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWindowsParams {}

/// Result of `list_windows` (§5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWindowsResult {
    /// All known windows, ordered by id.
    pub windows: Vec<WindowInfo>,
    /// Currently active (visible) window, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_window_id: Option<WindowId>,
}

/// Params of `get_window` (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetWindowParams {
    /// Window to describe.
    pub window_id: WindowId,
}

/// Result of `get_window` (§5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetWindowResult {
    /// The requested window.
    pub window: WindowInfo,
}

/// Params of `activate_window` (§5.3).
///
/// Mutates compositor state directly (keyboard focus + tiling), never as
/// synthetic input (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateWindowParams {
    /// Window to make active.
    pub window_id: WindowId,
}

/// Params of `close_window` (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseWindowParams {
    /// Window to close.
    pub window_id: WindowId,
}

/// Params of `get_focus` (§5.3) — the empty object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetFocusParams {}

/// Result of `get_focus` (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetFocusResult {
    /// Window holding keyboard focus, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Whether any surface currently holds keyboard focus.
    pub surface_focus: bool,
}
