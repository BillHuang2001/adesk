//! The accessibility domain vocabulary (`docs/protocol.md` §5.11).
//!
//! ADesk reads a window as TEXT through the Linux accessibility stack (AT-SPI2
//! over D-Bus) instead of pixels: an accessibility tree exposes roles, names,
//! values, state flags, actions and element bounds. This module carries only the
//! value types; the AT-SPI backend and its service live in `adesk-a11y`, the AGP
//! wire methods live in `adesk-proto` and dispatch lives in `adesk-server`.
//!
//! Field and variant names are the wire names; optional metadata carries `null`
//! when absent (like [`crate::WindowInfo`]).

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::ids::{AccessibleId, AppId, WindowId};

/// A state flag reported for an accessible element.
///
/// The wire names are the snake_case variants (e.g. `"read_only"`,
/// `"expanded"`). Only the flags that are *set* appear in
/// [`AccessibleNode::states`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccessibleState {
    /// The element is enabled (accepts input).
    Enabled,
    /// The element is sensitive (does not ignore input).
    Sensitive,
    /// The element is mapped and its ancestors are showing.
    Showing,
    /// The element is showing and within its parents' visible extents.
    Visible,
    /// The element can accept keyboard focus.
    Focusable,
    /// The element currently holds keyboard focus.
    Focused,
    /// The element can be checked and unchecked.
    Checkable,
    /// The element is checked.
    Checked,
    /// The element is selected.
    Selected,
    /// The element can be selected.
    Selectable,
    /// The element can be expanded and collapsed.
    Expandable,
    /// The element is expanded.
    Expanded,
    /// The element is collapsed.
    Collapsed,
    /// The element's content can be edited.
    Editable,
    /// The element accepts multiple lines of text.
    Multiline,
    /// The element's content is read-only.
    ReadOnly,
    /// The element is pressed (e.g. a toggle button).
    Pressed,
    /// The element's window is active.
    Active,
    /// The element is busy and not yet ready for interaction.
    Busy,
    /// The element is a modal window.
    Modal,
    /// The element is defunct (its backing object is gone).
    Defunct,
    /// The element attempts to reference an invalid object.
    Invalid,
}

/// One element of an accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibleNode {
    /// Stable handle usable with the AGP `invoke_accessible_action` method.
    pub id: AccessibleId,
    /// Toolkit accessibility role name normalized to lowercase snake_case
    /// (e.g. `"push_button"`, `"entry"`, `"label"`, `"frame"`, `"menu_item"`).
    ///
    /// It is the toolkit's own name kept as a string so no role is ever lost —
    /// never a lossy enum.
    pub role: String,
    /// Accessible name (may be empty).
    pub name: String,
    /// Extended help text; `null` when the toolkit reports none.
    pub description: Option<String>,
    /// Text/value content for value-bearing roles (entry text, label text,
    /// slider value); `null` otherwise.
    pub value: Option<String>,
    /// State flags that are set, sorted and de-duplicated.
    pub states: Vec<AccessibleState>,
    /// Element extents in window-relative pixel coordinates (relative to the
    /// window's own accessible frame origin); `null` when the toolkit reports
    /// none.
    pub bounds: Option<Rect>,
    /// Names of the actions the element exposes (e.g. `["click","activate"]`);
    /// empty when it exposes none.
    pub actions: Vec<String>,
    /// Child elements, in the toolkit's reported order.
    pub children: Vec<AccessibleNode>,
}

/// A whole accessibility snapshot of one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibleTree {
    /// Window this snapshot describes.
    pub window_id: WindowId,
    /// Desktop-file id of the window's application, once correlated.
    pub app_id: Option<AppId>,
    /// Application name as reported by the accessibility toolkit.
    pub app_name: Option<String>,
    /// Root element of the tree.
    pub root: AccessibleNode,
    /// Number of nodes in the tree, counting every node including the root.
    pub node_count: u32,
    /// Whether a depth or node bound stopped the walk early.
    pub truncated: bool,
}

/// A flat search hit returned by the `find_accessible` request.
///
/// This is the flat projection of [`AccessibleNode`]: the same fields minus
/// `description` and `children`, plus `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibleMatch {
    /// Stable handle usable with the AGP `invoke_accessible_action` method.
    pub id: AccessibleId,
    /// Toolkit accessibility role name normalized to lowercase snake_case
    /// (e.g. `"push_button"`, `"entry"`, `"label"`, `"frame"`, `"menu_item"`).
    ///
    /// It is the toolkit's own name kept as a string so no role is ever lost —
    /// never a lossy enum.
    pub role: String,
    /// Accessible name (may be empty).
    pub name: String,
    /// Text/value content for value-bearing roles (entry text, label text,
    /// slider value); `null` otherwise.
    pub value: Option<String>,
    /// State flags that are set, sorted and de-duplicated.
    pub states: Vec<AccessibleState>,
    /// Element extents in window-relative pixel coordinates (relative to the
    /// window's own accessible frame origin); `null` when the toolkit reports
    /// none.
    pub bounds: Option<Rect>,
    /// Names of the actions the element exposes (e.g. `["click","activate"]`);
    /// empty when it exposes none.
    pub actions: Vec<String>,
    /// Ancestor names from the window root down to (excluding) the matched node.
    pub path: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_node() -> AccessibleNode {
        AccessibleNode {
            id: AccessibleId(3),
            role: "push_button".into(),
            name: "Save".into(),
            description: Some("Save the document".into()),
            value: Some("Save".into()),
            states: vec![
                AccessibleState::Enabled,
                AccessibleState::Focusable,
                AccessibleState::Showing,
            ],
            bounds: Some(Rect {
                x: 10,
                y: 20,
                w: 80,
                h: 30,
            }),
            actions: vec!["click".into(), "activate".into()],
            children: Vec::new(),
        }
    }

    fn bare_node() -> AccessibleNode {
        AccessibleNode {
            id: AccessibleId(1),
            role: "frame".into(),
            name: String::new(),
            description: None,
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    fn sample_tree() -> AccessibleTree {
        let mut root = bare_node();
        root.children = vec![full_node()];
        AccessibleTree {
            window_id: WindowId(17),
            app_id: Some(AppId::from("org.gnome.Calculator")),
            app_name: Some("Calculator".into()),
            root,
            node_count: 2,
            truncated: false,
        }
    }

    fn sample_match() -> AccessibleMatch {
        AccessibleMatch {
            id: AccessibleId(3),
            role: "push_button".into(),
            name: "Save".into(),
            value: Some("Save".into()),
            states: vec![AccessibleState::Enabled, AccessibleState::Focusable],
            bounds: Some(Rect {
                x: 10,
                y: 20,
                w: 80,
                h: 30,
            }),
            actions: vec!["click".into()],
            path: vec!["frame".into(), "toolbar".into()],
        }
    }

    #[test]
    fn state_wire_names() {
        let expected = [
            (AccessibleState::Enabled, "enabled"),
            (AccessibleState::Sensitive, "sensitive"),
            (AccessibleState::Showing, "showing"),
            (AccessibleState::Visible, "visible"),
            (AccessibleState::Focusable, "focusable"),
            (AccessibleState::Focused, "focused"),
            (AccessibleState::Checkable, "checkable"),
            (AccessibleState::Checked, "checked"),
            (AccessibleState::Selected, "selected"),
            (AccessibleState::Selectable, "selectable"),
            (AccessibleState::Expandable, "expandable"),
            (AccessibleState::Expanded, "expanded"),
            (AccessibleState::Collapsed, "collapsed"),
            (AccessibleState::Editable, "editable"),
            (AccessibleState::Multiline, "multiline"),
            (AccessibleState::ReadOnly, "read_only"),
            (AccessibleState::Pressed, "pressed"),
            (AccessibleState::Active, "active"),
            (AccessibleState::Busy, "busy"),
            (AccessibleState::Modal, "modal"),
            (AccessibleState::Defunct, "defunct"),
            (AccessibleState::Invalid, "invalid"),
        ];
        for (value, name) in expected {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::json!(name)
            );
            assert_eq!(
                value,
                serde_json::from_value(serde_json::json!(name)).unwrap()
            );
        }
    }

    #[test]
    fn node_round_trips_through_serde() {
        for node in [full_node(), bare_node()] {
            let json = serde_json::to_string(&node).unwrap();
            assert_eq!(serde_json::from_str::<AccessibleNode>(&json).unwrap(), node);
        }
    }

    #[test]
    fn tree_round_trips_through_serde() {
        let tree = sample_tree();
        let json = serde_json::to_string(&tree).unwrap();
        assert_eq!(serde_json::from_str::<AccessibleTree>(&json).unwrap(), tree);
    }

    #[test]
    fn match_round_trips_through_serde() {
        let hit = sample_match();
        let json = serde_json::to_string(&hit).unwrap();
        assert_eq!(serde_json::from_str::<AccessibleMatch>(&json).unwrap(), hit);
    }

    #[test]
    fn node_golden_json_carries_explicit_nulls() {
        let value = serde_json::to_value(bare_node()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": 1,
                "role": "frame",
                "name": "",
                "description": null,
                "value": null,
                "states": [],
                "bounds": null,
                "actions": [],
                "children": [],
            })
        );

        let value = serde_json::to_value(full_node()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": 3,
                "role": "push_button",
                "name": "Save",
                "description": "Save the document",
                "value": "Save",
                "states": ["enabled", "focusable", "showing"],
                "bounds": {"x": 10, "y": 20, "w": 80, "h": 30},
                "actions": ["click", "activate"],
                "children": [],
            })
        );
    }

    #[test]
    fn tree_golden_json_shape() {
        let value = serde_json::to_value(sample_tree()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "window_id": 17,
                "app_id": "org.gnome.Calculator",
                "app_name": "Calculator",
                "root": {
                    "id": 1,
                    "role": "frame",
                    "name": "",
                    "description": null,
                    "value": null,
                    "states": [],
                    "bounds": null,
                    "actions": [],
                    "children": [
                        {
                            "id": 3,
                            "role": "push_button",
                            "name": "Save",
                            "description": "Save the document",
                            "value": "Save",
                            "states": ["enabled", "focusable", "showing"],
                            "bounds": {"x": 10, "y": 20, "w": 80, "h": 30},
                            "actions": ["click", "activate"],
                            "children": [],
                        }
                    ],
                },
                "node_count": 2,
                "truncated": false,
            })
        );
    }

    #[test]
    fn match_golden_json_shape() {
        let value = serde_json::to_value(sample_match()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": 3,
                "role": "push_button",
                "name": "Save",
                "value": "Save",
                "states": ["enabled", "focusable"],
                "bounds": {"x": 10, "y": 20, "w": 80, "h": 30},
                "actions": ["click"],
                "path": ["frame", "toolbar"],
            })
        );

        let bare = AccessibleMatch {
            id: AccessibleId(9),
            role: "label".into(),
            name: "Ready".into(),
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            path: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            serde_json::json!({
                "id": 9,
                "role": "label",
                "name": "Ready",
                "value": null,
                "states": [],
                "bounds": null,
                "actions": [],
                "path": [],
            })
        );
    }

    #[test]
    fn tree_carries_explicit_nulls_for_uncorrelated_app() {
        let tree = AccessibleTree {
            window_id: WindowId(4),
            app_id: None,
            app_name: None,
            root: bare_node(),
            node_count: 1,
            truncated: true,
        };
        let value = serde_json::to_value(&tree).unwrap();
        assert_eq!(value["app_id"], serde_json::Value::Null);
        assert_eq!(value["app_name"], serde_json::Value::Null);
        assert_eq!(value["truncated"], serde_json::json!(true));
    }
}
