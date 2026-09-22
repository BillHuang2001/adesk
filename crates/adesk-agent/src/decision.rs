//! The decision vocabulary: what the agent asked the runtime to do.
//!
//! One [`AgentDecision`] is produced by a provider and executed by the loop.
//! The enum deliberately separates *runtime-native* operations (compositor state:
//! list/launch/activate/capture/observe/wait — never synthesized input) from
//! *application input* (click/type/keypress/scroll — delivered through the Wayland
//! seat). The wire shape is a single internally-tagged JSON object (`{"op": ...}`),
//! which is also the schema the provider is prompted to emit.

use adesk_core::{AccessibleId, ActionId, AppId, Button, Position, Rect, WindowId};
use serde::{Deserialize, Serialize};

/// One decision produced by an [`crate::provider::LlmProvider`] and executed by
/// [`crate::agent_loop::AgentLoop`].
///
/// Field names mirror AGP (`docs/protocol.md` §2/§5); `after_action` is wired by
/// the loop: when it is `None`, the loop substitutes the id of the most recently
/// executed action so the resulting observation describes causal history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentDecision {
    /// Runtime-native: enumerate applications known to the registry.
    ListApps {
        /// Optional substring filter (case-insensitive).
        #[serde(default)]
        query: Option<String>,
    },
    /// Runtime-native: enumerate windows and the active window.
    ListWindows,
    /// Runtime-native: fetch one window's metadata.
    GetWindow {
        /// Target window.
        window_id: WindowId,
    },
    /// Runtime-native: launch an application through the registry.
    LaunchApp {
        /// Desktop-file id, e.g. `org.mozilla.firefox`.
        app_id: AppId,
        /// Extra argv entries appended after the expanded `Exec` line.
        #[serde(default)]
        args: Vec<String>,
    },
    /// Runtime-native: make a window active by mutating compositor state
    /// (never `Alt+Tab` or any other synthetic input).
    ActivateWindow {
        /// Target window.
        window_id: WindowId,
    },
    /// Runtime-native: close a window.
    CloseWindow {
        /// Target window.
        window_id: WindowId,
    },
    /// Runtime-native: request pixels (counts as a GPU readback).
    Capture {
        /// Target window.
        window_id: WindowId,
        /// Optional window-relative crop.
        #[serde(default)]
        region: Option<Rect>,
        /// Optional downscale target (longest edge).
        #[serde(default)]
        max_dimension: Option<u32>,
    },
    /// Runtime-native: wait for a condition and return a temporal observation.
    Observe {
        /// Window to filter on; `None` = all windows.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Causal filter; `None` = use the loop's last action id.
        #[serde(default)]
        after_action: Option<ActionId>,
        /// Condition to wait for.
        #[serde(default)]
        until: ObserveCondition,
        /// Overrides [`crate::agent_loop::LoopConfig::observe_timeout_ms`].
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// Overrides the loop's default image policy for this observation.
        #[serde(default)]
        include_image: Option<bool>,
        /// Optional downscale target (longest edge).
        #[serde(default)]
        max_dimension: Option<u32>,
        /// Optional window-relative crop.
        #[serde(default)]
        region: Option<Rect>,
    },
    /// Runtime-native: observation without an image (shorthand for `Observe`).
    Wait {
        /// Window to filter on; `None` = all windows.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Condition to wait for.
        #[serde(default)]
        until: ObserveCondition,
        /// Overrides [`crate::agent_loop::LoopConfig::observe_timeout_ms`].
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Runtime-native: read a window's UI as text through the accessibility stack.
    AccessibilityTree {
        /// Target window; `None` = the active window.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Maximum total node count; `None` = the protocol default.
        #[serde(default)]
        max_nodes: Option<u32>,
    },
    /// Runtime-native: find the accessible elements matching the AND-ed filters.
    FindAccessible {
        /// Target window; `None` = the active window.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Exact lowercase role name to match.
        #[serde(default)]
        role: Option<String>,
        /// Exact accessible name to match.
        #[serde(default)]
        name: Option<String>,
        /// Case-insensitive substring of the accessible name.
        #[serde(default)]
        name_contains: Option<String>,
        /// Case-insensitive substring of the node's value.
        #[serde(default)]
        value_contains: Option<String>,
        /// Maximum number of matches to return.
        #[serde(default)]
        max_results: Option<u32>,
    },
    /// Runtime-native: invoke an accessible element's action.
    InvokeAccessibleAction {
        /// Element whose action is invoked.
        node_id: AccessibleId,
        /// Action to invoke; `None` = the element's default action.
        #[serde(default)]
        action: Option<String>,
    },
    /// Application input: click through the Wayland seat.
    Click {
        /// Target window.
        window_id: WindowId,
        /// Window-relative position.
        position: Position,
        /// Mouse button.
        #[serde(default)]
        button: Button,
        /// Click count (1 = single, 2 = double).
        #[serde(default = "default_click_count")]
        count: u32,
    },
    /// Application input: type UTF-8 text through the seat.
    Type {
        /// Window to focus first; `None` = current focus.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Text to type.
        text: String,
    },
    /// Application input: press a key or chord (`["CTRL","L"]`).
    Keypress {
        /// Window to focus first; `None` = current focus.
        #[serde(default)]
        window_id: Option<WindowId>,
        /// Keysym-style names, modifiers first.
        keys: Vec<String>,
    },
    /// Application input: scroll through the seat.
    Scroll {
        /// Target window.
        window_id: WindowId,
        /// Window-relative position.
        position: Position,
        /// Horizontal axis delta.
        #[serde(default)]
        dx: f64,
        /// Vertical axis delta.
        dy: f64,
    },
    /// Finish the task.
    Finish {
        /// Whether the agent believes the task succeeded.
        #[serde(default)]
        success: bool,
        /// One-paragraph summary for the report.
        summary: String,
    },
}

/// Serde default for [`AgentDecision::Click::count`].
fn default_click_count() -> u32 {
    1
}

impl AgentDecision {
    /// The kind of action this decision represents (used by metrics and
    /// scenario expectations).
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::ListApps { .. } => ActionKind::ListApps,
            Self::ListWindows => ActionKind::ListWindows,
            Self::GetWindow { .. } => ActionKind::GetWindow,
            Self::LaunchApp { .. } => ActionKind::LaunchApp,
            Self::ActivateWindow { .. } => ActionKind::ActivateWindow,
            Self::CloseWindow { .. } => ActionKind::CloseWindow,
            Self::Capture { .. } => ActionKind::Capture,
            Self::Observe { .. } => ActionKind::Observe,
            Self::Wait { .. } => ActionKind::Wait,
            Self::AccessibilityTree { .. } => ActionKind::AccessibilityTree,
            Self::FindAccessible { .. } => ActionKind::FindAccessible,
            Self::InvokeAccessibleAction { .. } => ActionKind::InvokeAccessibleAction,
            Self::Click { .. } => ActionKind::Click,
            Self::Type { .. } => ActionKind::TypeText,
            Self::Keypress { .. } => ActionKind::Keypress,
            Self::Scroll { .. } => ActionKind::Scroll,
            Self::Finish { .. } => ActionKind::Finish,
        }
    }

    /// True for decisions delivered through the Wayland seat (application input).
    ///
    /// [`Self::Finish`] is neither input nor runtime-native: it changes no
    /// runtime state and injects no events.
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            Self::Click { .. } | Self::Type { .. } | Self::Keypress { .. } | Self::Scroll { .. }
        )
    }

    /// True for decisions that read or mutate runtime state directly
    /// (list/launch/activate/close/capture/observe/wait/accessibility_tree and
    /// the §5.11 `find_accessible` / `invoke_accessible_action` operations).
    pub fn is_runtime(&self) -> bool {
        matches!(
            self,
            Self::ListApps { .. }
                | Self::ListWindows
                | Self::GetWindow { .. }
                | Self::LaunchApp { .. }
                | Self::ActivateWindow { .. }
                | Self::CloseWindow { .. }
                | Self::Capture { .. }
                | Self::Observe { .. }
                | Self::Wait { .. }
                | Self::AccessibilityTree { .. }
                | Self::FindAccessible { .. }
                | Self::InvokeAccessibleAction { .. }
        )
    }
}

/// Stable classifier for decisions, used in metrics and scenario expectations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// `list_apps`.
    ListApps,
    /// `list_windows`.
    ListWindows,
    /// `get_window`.
    GetWindow,
    /// `launch_app`.
    LaunchApp,
    /// `activate_window`.
    ActivateWindow,
    /// `close_window`.
    CloseWindow,
    /// `capture_window` (GPU readback).
    Capture,
    /// `observe` (may include a GPU readback).
    Observe,
    /// `wait` (observation without image).
    Wait,
    /// `accessibility_tree` (read a window as structured text).
    AccessibilityTree,
    /// `find_accessible` (search a window's UI for named elements).
    FindAccessible,
    /// `invoke_accessible_action` (runtime-native element actuation).
    InvokeAccessibleAction,
    /// `click` / `double_click`.
    Click,
    /// `type_text`.
    TypeText,
    /// `keypress`.
    Keypress,
    /// `scroll`.
    Scroll,
    /// Task completion marker; not counted as an action in metrics.
    Finish,
}

impl ActionKind {
    /// Snake-case name, stable for reports and logs.
    ///
    /// These are the AGP method names the decision maps to (`capture_window`,
    /// `type_text`, ...), so logs and metrics line up with `docs/protocol.md`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ListApps => "list_apps",
            Self::ListWindows => "list_windows",
            Self::GetWindow => "get_window",
            Self::LaunchApp => "launch_app",
            Self::ActivateWindow => "activate_window",
            Self::CloseWindow => "close_window",
            Self::Capture => "capture_window",
            Self::Observe => "observe",
            Self::Wait => "wait",
            Self::AccessibilityTree => "accessibility_tree",
            Self::FindAccessible => "find_accessible",
            Self::InvokeAccessibleAction => "invoke_accessible_action",
            Self::Click => "click",
            Self::TypeText => "type_text",
            Self::Keypress => "keypress",
            Self::Scroll => "scroll",
            Self::Finish => "finish",
        }
    }
}

/// The `until` condition of an observation, mirroring AGP `Condition`
/// (`docs/protocol.md` §5.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObserveCondition {
    /// Wait until the window has been quiet for `quiet_ms`.
    ///
    /// Quietness is *evidence*, never proof of semantic completion.
    Quiet {
        /// Required quiet period in milliseconds.
        quiet_ms: u64,
    },
    /// Return on the first surface commit or window lifecycle event (default).
    #[default]
    Change,
    /// Wait the full timeout and report what accumulated (animation sampling).
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> WindowId {
        WindowId(7)
    }

    /// One decision per variant, with the kind it must report.
    fn vocabulary() -> Vec<(AgentDecision, ActionKind)> {
        use ActionKind as K;
        vec![
            (
                AgentDecision::ListApps {
                    query: Some("fire".into()),
                },
                K::ListApps,
            ),
            (AgentDecision::ListWindows, K::ListWindows),
            (
                AgentDecision::GetWindow {
                    window_id: window(),
                },
                K::GetWindow,
            ),
            (
                AgentDecision::LaunchApp {
                    app_id: AppId("org.example.app".into()),
                    args: vec!["--flag".into()],
                },
                K::LaunchApp,
            ),
            (
                AgentDecision::ActivateWindow {
                    window_id: window(),
                },
                K::ActivateWindow,
            ),
            (
                AgentDecision::CloseWindow {
                    window_id: window(),
                },
                K::CloseWindow,
            ),
            (
                AgentDecision::Capture {
                    window_id: window(),
                    region: Some(Rect::new(0, 0, 10, 10)),
                    max_dimension: Some(512),
                },
                K::Capture,
            ),
            (
                AgentDecision::Observe {
                    window_id: Some(window()),
                    after_action: Some(ActionId(3)),
                    until: ObserveCondition::Quiet { quiet_ms: 250 },
                    timeout_ms: Some(1_000),
                    include_image: Some(false),
                    max_dimension: None,
                    region: None,
                },
                K::Observe,
            ),
            (
                AgentDecision::Wait {
                    window_id: None,
                    until: ObserveCondition::Change,
                    timeout_ms: None,
                },
                K::Wait,
            ),
            (
                AgentDecision::AccessibilityTree {
                    window_id: Some(window()),
                    max_nodes: Some(2000),
                },
                K::AccessibilityTree,
            ),
            (
                AgentDecision::FindAccessible {
                    window_id: Some(window()),
                    role: Some("push_button".into()),
                    name: Some("Save".into()),
                    name_contains: None,
                    value_contains: None,
                    max_results: Some(5),
                },
                K::FindAccessible,
            ),
            (
                AgentDecision::InvokeAccessibleAction {
                    node_id: AccessibleId(3),
                    action: Some("click".into()),
                },
                K::InvokeAccessibleAction,
            ),
            (
                AgentDecision::Click {
                    window_id: window(),
                    position: Position::normalized(0.5, 0.5),
                    button: Button::Left,
                    count: 1,
                },
                K::Click,
            ),
            (
                AgentDecision::Type {
                    window_id: None,
                    text: "hello".into(),
                },
                K::TypeText,
            ),
            (
                AgentDecision::Keypress {
                    window_id: None,
                    keys: vec!["CTRL".into(), "L".into()],
                },
                K::Keypress,
            ),
            (
                AgentDecision::Scroll {
                    window_id: window(),
                    position: Position::pixels(10, 20),
                    dx: 0.0,
                    dy: -3.0,
                },
                K::Scroll,
            ),
            (
                AgentDecision::Finish {
                    success: true,
                    summary: "done".into(),
                },
                K::Finish,
            ),
        ]
    }

    #[test]
    fn kind_maps_every_variant() {
        let vocabulary = vocabulary();
        assert_eq!(vocabulary.len(), 17);
        for (decision, expected) in &vocabulary {
            assert_eq!(decision.kind(), *expected, "kind of {decision:?}");
        }
    }

    /// Runtime-native and seat-input decisions partition everything except
    /// `Finish`, which is neither.
    #[test]
    fn input_and_runtime_are_disjoint_and_leave_finish_out() {
        for (decision, kind) in vocabulary() {
            let input = decision.is_input();
            let runtime = decision.is_runtime();
            assert!(
                !(input && runtime),
                "{decision:?} is both input and runtime"
            );
            match kind {
                ActionKind::Click
                | ActionKind::TypeText
                | ActionKind::Keypress
                | ActionKind::Scroll => assert!(input, "{decision:?} should be input"),
                ActionKind::Finish => {
                    assert!(!input && !runtime, "Finish is neither input nor runtime")
                }
                _ => assert!(runtime, "{decision:?} should be runtime-native"),
            }
        }
    }

    /// The wire shape is one flat object tagged by `op`, with serde defaults for
    /// the optional fields — this is the schema the LLM is prompted to emit.
    #[test]
    fn json_shape_is_flat_and_defaults_are_applied() {
        assert_eq!(
            serde_json::to_value(AgentDecision::ListWindows).unwrap(),
            serde_json::json!({"op": "list_windows"})
        );
        assert_eq!(
            serde_json::to_value(AgentDecision::ListApps { query: None }).unwrap(),
            serde_json::json!({"op": "list_apps", "query": null})
        );

        // `until` defaults to `change` and `count` defaults to 1 when omitted.
        let click: AgentDecision = serde_json::from_value(serde_json::json!({
            "op": "click",
            "window_id": 7,
            "position": {"type": "normalized", "x": 0.5, "y": 0.25},
        }))
        .unwrap();
        assert_eq!(
            click,
            AgentDecision::Click {
                window_id: window(),
                position: Position::normalized(0.5, 0.25),
                button: Button::Left,
                count: 1,
            }
        );

        let wait: AgentDecision = serde_json::from_value(serde_json::json!({
            "op": "wait",
            "window_id": 7,
        }))
        .unwrap();
        assert_eq!(
            wait,
            AgentDecision::Wait {
                window_id: Some(window()),
                until: ObserveCondition::Change,
                timeout_ms: None,
            }
        );

        assert_eq!(
            serde_json::to_value(ObserveCondition::Quiet { quiet_ms: 250 }).unwrap(),
            serde_json::json!({"type": "quiet", "quiet_ms": 250})
        );
        assert_eq!(ObserveCondition::default(), ObserveCondition::Change);
        assert_eq!(
            serde_json::to_value(ObserveCondition::Timeout).unwrap(),
            serde_json::json!({"type": "timeout"})
        );
    }

    /// Every kind has a distinct, stable snake-case name; the serde name is the
    /// same vocabulary minus the `_window` suffix AGP adds to `capture`.
    #[test]
    fn action_kind_names_are_stable() {
        let mut names: Vec<&str> = vocabulary()
            .into_iter()
            .map(|(_, kind)| kind.as_str())
            .collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "duplicate ActionKind::as_str names");

        assert_eq!(ActionKind::ListApps.as_str(), "list_apps");
        assert_eq!(ActionKind::Capture.as_str(), "capture_window");
        assert_eq!(ActionKind::AccessibilityTree.as_str(), "accessibility_tree");
        assert_eq!(ActionKind::FindAccessible.as_str(), "find_accessible");
        assert_eq!(
            ActionKind::InvokeAccessibleAction.as_str(),
            "invoke_accessible_action"
        );
        assert_eq!(ActionKind::TypeText.as_str(), "type_text");
        assert_eq!(ActionKind::Finish.as_str(), "finish");
        for name in names {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not snake_case"
            );
        }
    }
}
