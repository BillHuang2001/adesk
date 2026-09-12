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

use std::cmp::Reverse;

use adesk_core::{
    ActionId, AppId, AppInfo, EventKind, Observation, Position, Rect, RuntimeEvent, Size, WindowId,
    WindowInfo, WindowState,
};
use adesk_proto::ImagePayload;
use serde::{Deserialize, Serialize};

use crate::client::RuntimeInfo;
use crate::decision::ActionKind;
use crate::text::truncate;

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
        Self {
            kind: event.kind(),
            seq: event.seq(),
            ts_ms: event.ts_ms(),
            window_id: event.window_id(),
            // Details are elided silently (no marker): the summary is already a
            // projection, so a trailing ellipsis would be noise.
            detail: truncate(&describe_event(event), max_detail_chars, ""),
        }
    }
}

/// Human-readable, payload-free description of one runtime event.
///
/// Damage is reduced to a rectangle *count* — never a rect list, never pixels.
fn describe_event(event: &RuntimeEvent) -> String {
    match event {
        RuntimeEvent::WindowCreated {
            app_id, title, pid, ..
        } => {
            let mut detail = String::from("window created");
            if let Some(app_id) = app_id {
                detail.push_str(&format!(" app={app_id}"));
            }
            if let Some(title) = title {
                detail.push_str(&format!(" title={title}"));
            }
            if let Some(pid) = pid {
                detail.push_str(&format!(" pid={pid}"));
            }
            detail
        }
        RuntimeEvent::WindowDestroyed { .. } => String::from("window destroyed"),
        RuntimeEvent::WindowActivated { previous, .. } => match previous {
            Some(previous) => format!("window activated (previous {previous})"),
            None => String::from("window activated"),
        },
        RuntimeEvent::TitleChanged { title, .. } => match title {
            Some(title) => format!("title changed to {title}"),
            None => String::from("title cleared"),
        },
        RuntimeEvent::SurfaceCommit {
            commit_seq, damage, ..
        } => commit_detail(*commit_seq, 1, damage.len()),
        RuntimeEvent::FocusChanged { window_id, .. } => match window_id {
            Some(window_id) => format!("focus changed to {window_id}"),
            None => String::from("focus cleared"),
        },
        RuntimeEvent::PopupAppeared { popup_id, .. } => format!("popup {popup_id} appeared"),
        RuntimeEvent::PopupDisappeared { popup_id, .. } => {
            format!("popup {popup_id} disappeared")
        }
        RuntimeEvent::AppLaunched { app_id, pid, .. } => {
            let mut detail = format!("app launched {app_id}");
            if let Some(pid) = pid {
                detail.push_str(&format!(" pid={pid}"));
            }
            detail
        }
        RuntimeEvent::Notification { notification, .. } => {
            let mut detail = format!("notification {} posted", notification.id);
            if let Some(source) = &notification.source {
                detail.push_str(&format!(" source={source}"));
            }
            // The protocol forbids an empty title, so it is always present.
            detail.push_str(&format!(" title={}", notification.title));
            detail
        }
        RuntimeEvent::NotificationClosed {
            notification_id,
            reason,
            ..
        } => format!("notification {notification_id} closed ({reason:?})"),
        RuntimeEvent::NotificationAction {
            notification_id,
            action_key,
            ..
        } => format!("notification {notification_id} action {action_key} invoked"),
    }
}

/// Detail string for a commit group, e.g. `"commit 8291 (3 damage rects)"` or,
/// once commits collapsed, `"commit 8291 (12 commits, 3 damage rects)"`.
fn commit_detail(commit_seq: u64, commits: u32, damage_rects: usize) -> String {
    let damage = match damage_rects {
        0 => String::from("no damage"),
        1 => String::from("1 damage rect"),
        n => format!("{n} damage rects"),
    };
    if commits > 1 {
        format!("commit {commit_seq} ({commits} commits, {damage})")
    } else {
        format!("commit {commit_seq} ({damage})")
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
        Self {
            id: window.id,
            app_id: window.app_id.clone(),
            title: window.title.clone(),
            active: window.state == WindowState::Active,
            geometry: window.geometry,
            popup_count: window.popup_count,
            last_commit_seq: window.last_commit_seq,
        }
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
        Self {
            id: app.id.clone(),
            name: app.name.clone(),
            categories: app.categories.clone(),
            terminal: app.terminal,
        }
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
        Self {
            protocol_version: info.protocol_version,
            renderer: info.renderer.clone(),
            output: info.output,
            uptime_ms: info.uptime_ms,
        }
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

/// Consecutive `surface_commit`s of one window, collapsed into one summary.
///
/// Collapse state is *only* valid while `events.first()` is the commit summary
/// this group describes; every non-commit event and every reset clears it.
#[derive(Debug, Clone, Copy)]
struct CommitGroup {
    window_id: WindowId,
    commits: u32,
    damage_rects: usize,
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
    commit_group: Option<CommitGroup>,
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
            commit_group: None,
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
        if self.budget.max_actions == 0 {
            return;
        }
        // Most recent first, so `build` is a plain clone and the oldest falls
        // off the tail.
        self.actions.insert(0, record);
        self.actions.truncate(self.budget.max_actions);
    }

    /// Summarize and record runtime events, trimming beyond `max_events` and
    /// collapsing consecutive commits of the same window.
    pub fn record_events(&mut self, events: &[RuntimeEvent]) {
        if self.budget.max_events == 0 {
            self.events.clear();
            self.commit_group = None;
            return;
        }
        for event in events {
            self.record_event(event);
        }
    }

    /// Record one event, collapsing it into the head summary when it is a
    /// commit of the same window as the head.
    fn record_event(&mut self, event: &RuntimeEvent) {
        let summary = EventSummary::from_event(event, self.budget.max_detail_chars);
        if let RuntimeEvent::SurfaceCommit {
            window_id,
            commit_seq,
            damage,
            ..
        } = event
        {
            let collapsed = match &mut self.commit_group {
                Some(group) if group.window_id == *window_id => {
                    group.commits += 1;
                    group.damage_rects += damage.len();
                    Some((group.commits, group.damage_rects))
                }
                _ => None,
            };
            if let Some((commits, damage_rects)) = collapsed {
                if let Some(head) = self.events.first_mut() {
                    head.seq = summary.seq;
                    head.ts_ms = summary.ts_ms;
                    head.detail = truncate(
                        &commit_detail(*commit_seq, commits, damage_rects),
                        self.budget.max_detail_chars,
                        "",
                    );
                    return;
                }
            }
            self.commit_group = Some(CommitGroup {
                window_id: *window_id,
                commits: 1,
                damage_rects: damage.len(),
            });
        } else {
            self.commit_group = None;
        }
        self.events.insert(0, summary);
        self.events.truncate(self.budget.max_events);
    }

    /// Set the current image, rotating the previous one into `keyframe`.
    ///
    /// Passing `None` clears both slots. When the budget allows fewer than two
    /// images the keyframe is dropped instead of stored.
    pub fn set_image(&mut self, image: Option<ImagePayload>) {
        let Some(image) = image else {
            self.clear_image();
            return;
        };
        if self.budget.max_images == 0 {
            self.clear_image();
            return;
        }
        let previous = self.current_image.take();
        self.current_image = Some(image);
        self.keyframe = if self.budget.max_images >= 2 {
            previous
        } else {
            None
        };
    }

    /// Drop both image slots.
    pub fn clear_image(&mut self) {
        self.current_image = None;
        self.keyframe = None;
    }

    /// Drop all accumulated state (used when starting a new task).
    pub fn reset(&mut self) {
        self.actions.clear();
        self.events.clear();
        self.commit_group = None;
        self.clear_image();
    }

    /// Assemble the bounded context for the next decision.
    pub fn build(&self, input: ContextInput<'_>) -> AgentContext {
        let mut windows: Vec<WindowSummary> = input
            .windows
            .iter()
            .map(WindowSummary::from_window)
            .collect();
        // `active_window` is authoritative from `list_windows`; honour it even
        // when the window state lags behind.
        if let Some(active) = input.active_window {
            for window in &mut windows {
                window.active |= window.id == active;
            }
        }
        // Active first, then most recent commit; id keeps the order total.
        windows.sort_by_key(|window| (!window.active, Reverse(window.last_commit_seq), window.id));
        windows.truncate(self.budget.max_windows);

        let mut apps: Vec<AppSummary> = input.apps.iter().map(AppSummary::from_app).collect();
        apps.truncate(self.budget.max_apps);

        AgentContext {
            task: input.task.goal.clone(),
            success_criteria: input.task.success_criteria.clone(),
            step: input.step,
            max_steps: input.max_steps,
            runtime: input.runtime.map(RuntimeSummary::from_runtime),
            windows,
            active_window: input.active_window,
            apps,
            recent_actions: self.actions.clone(),
            recent_events: self.events.clone(),
            observation: input
                .observation
                .map(|observation| self.trim_observation(observation)),
            last_error: input.last_error.map(str::to_owned),
            image: self.current_image.clone(),
            keyframe: self.keyframe.clone(),
        }
    }

    /// Clone `observation` with `changed_regions` reduced to the budget.
    ///
    /// The largest rects are kept; everything dropped is folded into the bounds
    /// of the last kept rect, so the damage evidence is never lost and the cap
    /// is still hard.
    fn trim_observation(&self, observation: &Observation) -> Observation {
        let mut trimmed = observation.clone();
        trimmed.changed_regions = trim_regions(
            &observation.changed_regions,
            self.budget.max_changed_regions,
        );
        trimmed
    }
}

/// Reduces `regions` to at most `cap` rects, largest area first.
fn trim_regions(regions: &[Rect], cap: usize) -> Vec<Rect> {
    let mut kept: Vec<Rect> = regions
        .iter()
        .copied()
        .filter(|rect| !rect.is_empty())
        .collect();
    if kept.len() <= cap {
        return kept;
    }
    if cap == 0 {
        return Vec::new();
    }
    kept.sort_by(|a, b| {
        b.area()
            .cmp(&a.area())
            .then_with(|| (a.y, a.x, a.h, a.w).cmp(&(b.y, b.x, b.h, b.w)))
    });
    let dropped = kept.split_off(cap);
    let bounds = dropped
        .iter()
        .fold(Rect::EMPTY, |acc, rect| acc.union(rect));
    if let Some(last) = kept.last_mut() {
        let merged = last.union(&bounds);
        *last = merged;
    }
    kept
}
