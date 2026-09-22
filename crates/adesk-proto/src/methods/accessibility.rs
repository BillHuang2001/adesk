//! Accessibility methods (§5.11): the runtime's text-only view of a window's UI.
//!
//! The runtime reads a window as TEXT through the toolkit accessibility stack
//! (AT-SPI2 over D-Bus) instead of pixels. The value types live in `adesk_core`
//! and are reused here verbatim, never forked.

use adesk_core::{AccessibleId, AccessibleMatch, AccessibleTree, ActionId, WindowId};
use serde::{Deserialize, Serialize};

use crate::defaults;

/// Params of `accessibility_tree` (§5.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilityTreeParams {
    /// Window to snapshot; `null`/omitted resolves like an unscoped observation
    /// (§5.4): the active window, else the keyboard-focus window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Maximum recursion depth below the window root (default `12`; `0` = the
    /// root alone).
    #[serde(default = "defaults::accessibility_max_depth")]
    pub max_depth: u32,
    /// Maximum total node count (default `2000`).
    #[serde(default = "defaults::accessibility_max_nodes")]
    pub max_nodes: u32,
    /// Include element bounds (default `true`).
    #[serde(default = "defaults::accessibility_include")]
    pub include_bounds: bool,
    /// Include state flags (default `true`).
    #[serde(default = "defaults::accessibility_include")]
    pub include_states: bool,
    /// Include action names (default `true`).
    #[serde(default = "defaults::accessibility_include")]
    pub include_actions: bool,
    /// Render the outline `text` (default `true`).
    #[serde(default = "defaults::accessibility_include")]
    pub include_text: bool,
}

impl Default for AccessibilityTreeParams {
    fn default() -> AccessibilityTreeParams {
        AccessibilityTreeParams {
            window_id: None,
            max_depth: defaults::accessibility_max_depth(),
            max_nodes: defaults::accessibility_max_nodes(),
            include_bounds: true,
            include_states: true,
            include_actions: true,
            include_text: true,
        }
    }
}

/// Result of `accessibility_tree` (§5.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilityTreeResult {
    /// The window's accessibility tree (a partial tree when truncated).
    pub tree: AccessibleTree,
    /// Rendered outline of the returned tree (`""` when `include_text` is false).
    pub text: String,
}

/// Params of `find_accessible` (§5.11).
///
/// The filters are AND-ed; an omitted filter matches everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindAccessibleParams {
    /// Window to search; `null`/omitted resolves like an unscoped observation
    /// (§5.4): the active window, else the keyboard-focus window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Exact lowercase role name to match, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Exact accessible name to match, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Case-insensitive substring of the accessible name, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_contains: Option<String>,
    /// Case-insensitive substring of the node's value, when given (a node
    /// without a value never matches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_contains: Option<String>,
    /// Maximum number of matches to return (default `50`; `0` is invalid).
    #[serde(default = "defaults::find_max_results")]
    pub max_results: u32,
}

impl Default for FindAccessibleParams {
    fn default() -> FindAccessibleParams {
        FindAccessibleParams {
            window_id: None,
            role: None,
            name: None,
            name_contains: None,
            value_contains: None,
            max_results: defaults::find_max_results(),
        }
    }
}

/// Result of `find_accessible` (§5.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindAccessibleResult {
    /// Window the search was scoped to.
    pub window_id: WindowId,
    /// Matching elements in tree (pre-order) order, at most `max_results`.
    pub matches: Vec<AccessibleMatch>,
    /// Whether more matches existed beyond `max_results`.
    pub truncated: bool,
}

/// Params of `invoke_accessible_action` (§5.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvokeAccessibleActionParams {
    /// Element whose action is invoked.
    pub node_id: AccessibleId,
    /// Action to invoke; `null`/omitted means the element's default (first)
    /// action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// Result of `invoke_accessible_action` (§5.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvokeAccessibleActionResult {
    /// Id of the action, usable with `after_action` (§5.4).
    pub action_id: ActionId,
    /// The element whose action was invoked.
    pub node_id: AccessibleId,
    /// The name of the action actually invoked (the default when `action` was
    /// omitted).
    pub action: String,
}
