//! The decision vocabulary: what the agent asked the runtime to do.
//!
//! One [`AgentDecision`] is produced by a provider and executed by the loop.
//! The enum deliberately separates *runtime-native* operations (compositor state:
//! list/launch/activate/capture/observe/wait — never synthesized input) from
//! *application input* (click/type/keypress/scroll — delivered through the Wayland
//! seat). The wire shape is a single internally-tagged JSON object (`{"op": ...}`),
//! which is also the schema the provider is prompted to emit.

use adesk_core::{ActionId, AppId, Button, Position, Rect, WindowId};
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
        todo!("phase 2: map decision -> ActionKind")
    }

    /// True for decisions delivered through the Wayland seat (application input).
    pub fn is_input(&self) -> bool {
        todo!("phase 2: Click/Type/Keypress/Scroll")
    }

    /// True for decisions that read or mutate runtime state directly
    /// (list/launch/activate/close/capture/observe/wait).
    pub fn is_runtime(&self) -> bool {
        todo!("phase 2: runtime-native operations")
    }
}

/// Stable classifier for decisions, used in metrics and scenario expectations.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
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
    pub fn as_str(&self) -> &'static str {
        todo!("phase 2: snake_case names")
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
