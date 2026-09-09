//! Bounded context assembly: what the LLM sees on every step.
//!
//! [`AgentContext`] is deliberately compact. The runtime may commit thousands of
//! frames per task, but a context carries **at most one current image and one
//! previous keyframe**, plus capped lists of actions, events, windows and apps.
//! The exact caps live in [`ContextBudget`] and are enforced by
//! [`ContextBuilder`] — the context never grows with step count.
//!
//! Budgeting rules (defaults):
//!
//! | Field | Default | Rule |
//! |---|---|---|
//! | `max_actions` | 8 | keep the most recent executed actions, oldest dropped |
//! | `max_events` | 16 | keep the most recent summarized events; consecutive `surface_commit`s for one window collapse into one summary with a count |
//! | `max_windows` | 8 | active window first, then by most recent commit |
//! | `max_apps` | 12 | only populated after a `list_apps` decision |
//! | `max_changed_regions` | 4 | `Observation::changed_regions` truncated to the largest rects; extras merge into their bounds |
//! | `max_images` | 2 | exactly one current image + one previous keyframe, never a history |
//! | `max_detail_chars` | 160 | each summary `detail` string is truncated |
//! | `max_dimension` | `Some(1024)` | default downscale for images sent to the provider |

use adesk_core::{
    ActionId, AppId, AppInfo, EventKind, Observation, Position, Rect, RuntimeEvent, Size,
    WindowId, WindowInfo,
};
use adesk_proto::ImagePayload;
use serde::{Deserialize, Serialize};

use crate::client::RuntimeInfo;
use crate::decision::ActionKind;

/// Task description handed to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDescription {
    /// What the agent must accomplish.
    pub goal: String,
    /// Optional machine-checkable success criterion.
    #[serde(default)]
    pub success_criteria: Option<String>,
    /// Optional strategy hints injected into the system prompt.
    #[serde(default)]
    pub hints: Vec<String>,
}

impl TaskDescription {
    /// Build a task from a goal string.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            success_criteria: None,
            hints: Vec::new(),
        }
    }
}

/// Hard caps applied when assembling [`AgentContext`].
///
/// Every field is a hard cap, not a hint: the builder truncates and the loop
/// never exceeds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    /// Maximum `recent_actions` kept (most recent first).
    pub max_actions: usize,
    /// Maximum `recent_events` kept after summarization.
    pub max_events: usize,
    /// Maximum `windows` entries kept (active window first).
    pub max_windows: usize,
    /// Maximum `apps` entries kept.
    pub max_apps: usize,
    /// Maximum rects kept from `Observation::changed_regions`.
    pub max_changed_regions: usize,
    /// Images per context: one current image plus one previous keyframe.
    pub max_images: usize,
    /// Maximum characters per summary `detail` string.
    pub max_detail_chars: usize,
    /// Default downscale target for images sent to the provider.
    pub max_dimension: Option<u32>,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_actions: 8,
            max_events: 16,
            max_windows: 8,
            max_apps: 12,
            max_changed_regions: 4,
            max_images: 2,
            max_detail_chars: 160,
            max_dimension: Some(1024),
        }
    }
}

/// One executed action, as remembered by the context builder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRecord {
    /// Loop step that executed the action.
    pub step: u32,
    /// Runtime-assigned id (input and state-mutating operations only).
    #[serde(default)]
    pub action_id: Option<ActionId>,
    /// Kind of action.
    pub kind: ActionKind,
    /// Target window, when applicable.
    #[serde(default)]
    pub window_id: Option<WindowId>,
    /// Window-relative position, when applicable.
    #[serde(default)]
    pub position: Option<Position>,
    /// Short human-readable detail (arguments, typed-text summary, ...).
    pub detail: String,
    /// Whether the action executed without error.
    pub ok: bool,
}

/// Compact projection of a [`RuntimeEvent`] (never the raw payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSummary {
    /// Event kind.
    pub kind: EventKind,
    /// Global event sequence number.
    pub seq: u64,
    /// Monotonic milliseconds since runtime start.
    pub ts_ms: u64,
    /// Window the event belongs to, when applicable.
    #[serde(default)]
    pub window_id: Option<WindowId>,
    /// Short human-readable detail, e.g. `"commit 8291 (3 damage rects)"`.
    pub detail: String,
}

impl EventSummary {
    /// Summarize one runtime event, truncating `detail` to `max_detail_chars`.
    ///
    /// Damage regions are reduced to counts/bounds — pixel payloads and full rect
    /// lists never enter the context.
    pub fn from_event(event: &RuntimeEvent, max_detail_chars: usize) -> Self {
        let _ = (event, max_detail_chars);
        todo!("phase 2: summarize event kind + window + short detail")
    }
}

/// Bounded window metadata for the prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowSummary {
    /// Window id.
    pub id: WindowId,
    /// Resolved application id, when known.
    #[serde(default)]
    pub app_id: Option<AppId>,
    /// Title, when known.
    #[serde(default)]
    pub title: Option<String>,
    /// Whether this is the active window.
    pub active: bool,
    /// Window geometry (window-relative coordinate space).
    pub geometry: Rect,
    /// Number of mapped popups.
    pub popup_count: u32,
    /// Last surface commit sequence.
    pub last_commit_seq: u64,
}

impl WindowSummary {
    /// Project a [`WindowInfo`] into the bounded form.
    pub fn from_window(window: &WindowInfo) -> Self {
        let _ = window;
        todo!("phase 2: project WindowInfo")
    }
}

/// Bounded application metadata for the prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSummary {
    /// Desktop-file id.
    pub id: AppId,
    /// Human-readable name.
    pub name: String,
    /// Freedesktop categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Whether the app needs a terminal.
    pub terminal: bool,
}

impl AppSummary {
    /// Project an [`AppInfo`] into the bounded form.
    pub fn from_app(app: &AppInfo) -> Self {
        let _ = app;
        todo!("phase 2: project AppInfo")
    }
}

/// Runtime capabilities carried into every context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSummary {
    /// AGP protocol version.
    pub protocol_version: u32,
    /// Chosen renderer (`"gl"` / `"pixman"`).
    pub renderer: String,
    /// Virtual output size.
    pub output: Size,
    /// Milliseconds since runtime start.
    pub uptime_ms: u64,
}

impl RuntimeSummary {
    /// Project [`RuntimeInfo`] into the context form.
    pub fn from_runtime(info: &RuntimeInfo) -> Self {
        let _ = info;
        todo!("phase 2: project RuntimeInfo")
    }
}

/// The complete, bounded prompt context for one decision.
///
/// Constructed only by [`ContextBuilder::build`]; every collection is already
/// capped. There is no field that can grow with step count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    /// Task goal.
    pub task: String,
    /// Optional success criterion.
    #[serde(default)]
    pub success_criteria: Option<String>,
    /// Current step (0-based).
    pub step: u32,
    /// Step budget.
    pub max_steps: u32,
    /// Runtime capabilities from `ping`, when known.
    #[serde(default)]
    pub runtime: Option<RuntimeSummary>,
    /// Bounded window list (active first).
    #[serde(default)]
    pub windows: Vec<WindowSummary>,
    /// Active window id, when known.
    #[serde(default)]
    pub active_window: Option<WindowId>,
    /// Bounded application list; only populated after `list_apps`.
    #[serde(default)]
    pub apps: Vec<AppSummary>,
    /// Recent executed actions, most recent first.
    #[serde(default)]
    pub recent_actions: Vec<ActionRecord>,
    /// Recent runtime events, summarized, most recent first.
    #[serde(default)]
    pub recent_events: Vec<EventSummary>,
    /// Latest observation, trimmed to the budget.
    #[serde(default)]
    pub observation: Option<Observation>,
    /// Last error the loop hit, for recovery reasoning.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Current image (at most one).
    #[serde(default)]
    pub image: Option<ImagePayload>,
    /// Previous keyframe (at most one; always older than `image`).
    #[serde(default)]
    pub keyframe: Option<ImagePayload>,
}

impl AgentContext {
    /// Number of embedded images; must never exceed [`ContextBudget::max_images`].
    pub fn image_count(&self) -> usize {
        usize::from(self.image.is_some()) + usize::from(self.keyframe.is_some())
    }

    /// Total base64 payload bytes carried by the embedded images.
    pub fn image_bytes(&self) -> usize {
        self.image.as_ref().map_or(0, |i| i.data.len())
            + self.keyframe.as_ref().map_or(0, |i| i.data.len())
    }
}

/// Live runtime facts the loop supplies when building a context.
#[derive(Debug, Clone, Copy)]
pub struct ContextInput<'a> {
    /// Task description.
    pub task: &'a TaskDescription,
    /// Current step index (0-based).
    pub step: u32,
    /// Step budget.
    pub max_steps: u32,
    /// Runtime info from `ping`, when known.
    pub runtime: Option<&'a RuntimeInfo>,
    /// Current window list.
    pub windows: &'a [WindowInfo],
    /// Active window id.
    pub active_window: Option<WindowId>,
    /// Apps discovered by the most recent `list_apps`.
    pub apps: &'a [AppInfo],
    /// Latest observation.
    pub observation: Option<&'a Observation>,
    /// Last error message shown to the agent.
    pub last_error: Option<&'a str>,
}

/// Stateful, bounded context assembler owned by the loop.
///
/// It accumulates only what the budget allows: actions and summarized events are
/// ring buffers, and images rotate — setting a new image pushes the previous one
/// into `keyframe` and drops whatever was there before. Older frames are gone
/// forever, by design.
#[derive(Debug, Clone)]
pub struct ContextBuilder {
    budget: ContextBudget,
    actions: Vec<ActionRecord>,
    events: Vec<EventSummary>,
    current_image: Option<ImagePayload>,
    keyframe: Option<ImagePayload>,
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new(ContextBudget::default())
    }
}

impl ContextBuilder {
    /// Create a builder with the given budget.
    pub fn new(budget: ContextBudget) -> Self {
        Self {
            budget,
            actions: Vec::new(),
            events: Vec::new(),
            current_image: None,
            keyframe: None,
        }
    }

    /// The active budget.
    pub fn budget(&self) -> &ContextBudget {
        &self.budget
    }

    /// Number of remembered actions (never exceeds `max_actions`).
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// Number of remembered events (never exceeds `max_events`).
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Number of remembered images (never exceeds `max_images`).
    pub fn image_count(&self) -> usize {
        usize::from(self.current_image.is_some()) + usize::from(self.keyframe.is_some())
    }

    /// Record one executed action, trimming the oldest beyond `max_actions`.
    pub fn record_action(&mut self, record: ActionRecord) {
        let _ = record;
        todo!("phase 2: push action, trim to budget.max_actions")
    }

    /// Summarize and record runtime events, trimming beyond `max_events` and
    /// collapsing consecutive commits of the same window.
    pub fn record_events(&mut self, events: &[RuntimeEvent]) {
        let _ = events;
        todo!("phase 2: summarize, collapse commits, trim to budget.max_events")
    }

    /// Set the current image, rotating the previous one into `keyframe`.
    ///
    /// Passing `None` clears both slots.
    pub fn set_image(&mut self, image: Option<ImagePayload>) {
        let _ = image;
        todo!("phase 2: rotate current -> keyframe, drop older")
    }

    /// Drop both image slots.
    pub fn clear_image(&mut self) {
        todo!("phase 2: clear image + keyframe")
    }

    /// Drop all accumulated state (used when starting a new task).
    pub fn reset(&mut self) {
        todo!("phase 2: clear actions, events, images")
    }

    /// Assemble the bounded context for the next decision.
    pub fn build(&self, input: ContextInput<'_>) -> AgentContext {
        let _ = input;
        todo!("phase 2: project live facts, apply all caps")
    }
}
