//! Input methods (§5.5). All are delivered through the Wayland seat; every one
//! answers [`ActionResult`](crate::ActionResult).

use adesk_core::{ActionId, Button, Position, WindowId};
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::types::KeySpec;

/// Params of `pointer_move` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PointerMoveParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative target position.
    pub position: Position,
}

/// Params of `click` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClickParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative position; defaults to the pointer position when inside
    /// the window, else the window center (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button (default `"left"`).
    #[serde(default)]
    pub button: Button,
    /// Number of clicks (default `1`).
    #[serde(default = "defaults::count")]
    pub count: u32,
}

/// Params of `double_click` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DoubleClickParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative position; defaults to pointer position / window center (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button (default `"left"`).
    #[serde(default)]
    pub button: Button,
}

/// Params of `mouse_down` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MouseDownParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative position; defaults to pointer position / window center (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button (default `"left"`).
    #[serde(default)]
    pub button: Button,
}

/// Params of `mouse_up` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MouseUpParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative position; defaults to pointer position / window center (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button (default `"left"`).
    #[serde(default)]
    pub button: Button,
}

/// Params of `scroll` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScrollParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative position; defaults to pointer position / window center (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Horizontal scroll amount (default `0.0`).
    #[serde(default)]
    pub dx: f64,
    /// Vertical scroll amount (required; positive = down).
    pub dy: f64,
}

/// Params of `drag` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DragParams {
    /// Window the coordinates are relative to.
    pub window_id: WindowId,
    /// Window-relative start position.
    pub from: Position,
    /// Window-relative end position.
    pub to: Position,
    /// Mouse button (default `"left"`).
    #[serde(default)]
    pub button: Button,
    /// Total drag duration in milliseconds (default `150`).
    #[serde(default = "defaults::duration_ms")]
    pub duration_ms: u64,
}

/// Params of `keypress` (§5.5) — a chord or a single key (§3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeypressParams {
    /// Keys to press: `["CTRL","L"]` presses modifiers, taps the final key and
    /// releases modifiers in reverse; `"a"` is a single key.
    pub keys: KeySpec,
    /// Window to activate first when given and different from the focused one (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

/// Params of `key_down` (§5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyDownParams {
    /// Key name (§3 vocabulary).
    pub key: String,
    /// Window to activate first when given and different from the focused one (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

/// Params of `key_up` (§5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyUpParams {
    /// Key name (§3 vocabulary).
    pub key: String,
    /// Window to activate first when given and different from the focused one (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

/// Params of `type_text` (§5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeTextParams {
    /// UTF-8 text to type through the xkb keymap (§3).
    pub text: String,
    /// Window to activate first when given and different from the focused one (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

/// Result of `type_text` (§5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeTextResult {
    /// Id of the action.
    pub action_id: ActionId,
    /// Characters that have no keymap entry and were skipped (§3).
    pub skipped: Vec<String>,
}
