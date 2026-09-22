//! AGP §5.11 — accessibility: the runtime's text-only view of a window's UI.
//!
//! Instead of pixels, an agent can read a window as structured text through the
//! toolkit accessibility stack (AT-SPI2 over D-Bus):
//! [`Client::accessibility_tree`] returns a window's element tree plus a rendered
//! outline, [`Client::find_accessible`] searches that tree for the elements
//! matching a set of AND-ed filters, and [`Client::invoke_accessible_action`]
//! actuates an element through the toolkit's accessibility `Action` interface —
//! runtime-native actuation, never synthesized input (design invariant 3).
//!
//! Observation is on demand only: there are no accessibility events and no
//! accessibility [`EventKind`](crate::EventKind). The value vocabulary
//! ([`AccessibleTree`], [`AccessibleNode`](adesk_core::AccessibleNode),
//! [`AccessibleMatch`], [`AccessibleState`](adesk_core::AccessibleState),
//! [`AccessibleId`]) lives in `adesk-core` and is used here directly, never
//! forked.

use adesk_core::{AccessibleId, AccessibleMatch, AccessibleTree, ActionId, WindowId};
use serde::{Deserialize, Serialize};

use crate::{Client, Result};

/// `accessibility_tree` params (protocol §5.11).
///
/// Every field is optional: an unset field is left off the wire and the runtime
/// applies its §5.11 default (`max_depth` 12, `max_nodes` 2000, and all four
/// projection flags `true`). `window_id` omitted resolves like an unscoped
/// observation (§5.4): the runtime's active window, else the keyboard-focus
/// window; an unknown `window_id` fails with `unknown_window`.
///
/// The projection flags remove data from the returned nodes *and* from the
/// rendered text: `include_states`/`include_bounds`/`include_actions` false
/// yields an empty list (or `null`), and `include_text` false returns `""`.
#[derive(Debug, Clone, Default, Serialize)]
#[non_exhaustive]
pub struct AccessibilityTreeRequest {
    /// Window to snapshot; `None` resolves to the active/focused window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Maximum recursion depth below the window root (`0` = the root alone).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum total node count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u32>,
    /// Include element bounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_bounds: Option<bool>,
    /// Include state flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_states: Option<bool>,
    /// Include action names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_actions: Option<bool>,
    /// Render the outline `text` (`false` returns `""`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_text: Option<bool>,
}

impl AccessibilityTreeRequest {
    /// The whole tree of the runtime's active window with protocol defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot `window_id` instead of the active/focused window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Bound recursion below the window root (`0` = the root alone).
    pub fn max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = Some(max_depth);
        self
    }

    /// Bound the total node count.
    pub fn max_nodes(mut self, max_nodes: u32) -> Self {
        self.max_nodes = Some(max_nodes);
        self
    }

    /// Include (or drop) element bounds.
    pub fn include_bounds(mut self, include: bool) -> Self {
        self.include_bounds = Some(include);
        self
    }

    /// Include (or drop) state flags.
    pub fn include_states(mut self, include: bool) -> Self {
        self.include_states = Some(include);
        self
    }

    /// Include (or drop) action names.
    pub fn include_actions(mut self, include: bool) -> Self {
        self.include_actions = Some(include);
        self
    }

    /// Render (or skip) the outline `text`.
    pub fn include_text(mut self, include: bool) -> Self {
        self.include_text = Some(include);
        self
    }
}

/// `accessibility_tree` result (protocol §5.11).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct AccessibilityTreeResult {
    /// The window's accessibility tree (a partial tree when truncated).
    pub tree: AccessibleTree,
    /// Rendered outline of the returned tree (`""` when `include_text` is false).
    pub text: String,
}

/// `find_accessible` params (protocol §5.11).
///
/// The filters are AND-ed and an unset filter matches everything: `role` is an
/// exact lowercase role name, `name` an exact accessible name, and
/// `name_contains`/`value_contains` case-insensitive substrings of the
/// accessible name/value (a node without a value never matches
/// `value_contains`). `window_id` omitted resolves like an unscoped observation
/// (§5.4); `max_results` omitted uses the §5.11 default (`50`, and `0` is
/// invalid).
#[derive(Debug, Clone, Default, Serialize)]
#[non_exhaustive]
pub struct FindAccessibleRequest {
    /// Window to search; `None` resolves to the active/focused window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Exact lowercase role name to match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Exact accessible name to match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Case-insensitive substring of the accessible name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_contains: Option<String>,
    /// Case-insensitive substring of the node's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_contains: Option<String>,
    /// Maximum number of matches to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

impl FindAccessibleRequest {
    /// Every element of the runtime's active window (no filters).
    pub fn new() -> Self {
        Self::default()
    }

    /// Search `window_id` instead of the active/focused window.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Match an exact lowercase role name.
    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Match an exact accessible name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Match a case-insensitive substring of the accessible name.
    pub fn name_contains(mut self, name_contains: impl Into<String>) -> Self {
        self.name_contains = Some(name_contains.into());
        self
    }

    /// Match a case-insensitive substring of the node's value.
    pub fn value_contains(mut self, value_contains: impl Into<String>) -> Self {
        self.value_contains = Some(value_contains.into());
        self
    }

    /// Cap the number of matches.
    pub fn max_results(mut self, max_results: u32) -> Self {
        self.max_results = Some(max_results);
        self
    }
}

/// `find_accessible` result (protocol §5.11).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct FindAccessibleResult {
    /// Window the search was scoped to.
    pub window_id: WindowId,
    /// Matching elements in tree (pre-order) order, at most `max_results`.
    pub matches: Vec<AccessibleMatch>,
    /// Whether more matches existed beyond `max_results`.
    pub truncated: bool,
}

/// `invoke_accessible_action` params (protocol §5.11).
///
/// `action` omitted means the element's default (first) action; the response
/// carries the name actually invoked.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct InvokeAccessibleActionRequest {
    /// Element whose action is invoked.
    pub node_id: AccessibleId,
    /// Action to invoke; `None` means the element's default action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl InvokeAccessibleActionRequest {
    /// Invoke `node_id`'s default (first) action.
    pub fn new(node_id: AccessibleId) -> Self {
        Self {
            node_id,
            action: None,
        }
    }

    /// Invoke a named action the element exposes.
    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }
}

/// `invoke_accessible_action` result (protocol §5.11).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct InvokeAccessibleActionResult {
    /// Id of the action, usable with `after_action` (§5.4).
    pub action_id: ActionId,
    /// The element whose action was invoked.
    pub node_id: AccessibleId,
    /// The name of the action actually invoked (the default when the request
    /// omitted `action`).
    pub action: String,
}

impl Client {
    /// `accessibility_tree` — read a window as a structured accessibility tree
    /// plus a rendered outline.
    ///
    /// Observation is on demand; no accessibility event stream exists (§5.11).
    /// A window with no accessible subtree, or a runtime with no accessibility
    /// backend, fails with `not_supported` — never a guessed or empty tree.
    pub async fn accessibility_tree(
        &self,
        request: AccessibilityTreeRequest,
    ) -> Result<AccessibilityTreeResult> {
        self.request("accessibility_tree", &request).await
    }

    /// `find_accessible` — search a window's accessibility tree for elements
    /// matching the AND-ed filters.
    ///
    /// Matches arrive in tree (pre-order) order; `truncated` reports that more
    /// matches existed beyond `max_results` (§5.11).
    pub async fn find_accessible(
        &self,
        request: FindAccessibleRequest,
    ) -> Result<FindAccessibleResult> {
        self.request("find_accessible", &request).await
    }

    /// `invoke_accessible_action` — actuate an accessible element through the
    /// toolkit's `Action` interface.
    ///
    /// This is runtime-native actuation, not synthesized input (§5.11, design
    /// invariant 3). An unknown or vanished node fails with `unknown_accessible`;
    /// an action the element does not expose fails with `invalid_request`; no
    /// accessibility backend fails with `not_supported`. The returned
    /// [`ActionId`] is an ordinary AGP action id, usable with `after_action`.
    pub async fn invoke_accessible_action(
        &self,
        request: InvokeAccessibleActionRequest,
    ) -> Result<InvokeAccessibleActionResult> {
        self.request("invoke_accessible_action", &request).await
    }
}
