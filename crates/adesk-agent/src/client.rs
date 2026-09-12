//! The AGP client seam: the subset of the Agent GUI Protocol the agent drives.
//!
//! [`AgentClient`] is the agent's abstraction over `adesk-client`. It exists so
//! that the loop can be exercised with a scripted fake (`testing::ScriptedClient`)
//! without a socket or compositor, and so that the concrete SDK is adapted in
//! exactly one place ([`crate::agp`]).
//!
//! Request/result types mirror the AGP results in `docs/protocol.md` §5; they are
//! the agent's own vocabulary, deliberately not `adesk-proto` wire frames — the
//! adapter converts.

use adesk_core::{
    ActionId, AppId, AppInfo, Button, EventKind, LaunchId, Observation, Position, Rect,
    RuntimeEvent, Size, WindowId, WindowInfo,
};
use adesk_proto::ImagePayload;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::decision::ObserveCondition;
use crate::Result;

/// AGP version implemented by this crate (`docs/protocol.md` §1).
///
/// The loop refuses to run against a runtime that reports a different version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Protocol default for [`WaitForEventsRequest::max_events`] (`docs/protocol.md`
/// §5.10): the most events a single wait answers with.
const DEFAULT_WAIT_MAX_EVENTS: u32 = 32;

/// Runtime identity and capabilities, from `ping`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// AGP protocol version the runtime speaks.
    pub protocol_version: u32,
    /// Runtime crate version string.
    pub runtime_version: String,
    /// Milliseconds since runtime start.
    pub uptime_ms: u64,
    /// Chosen renderer: `"gl"` or `"pixman"`.
    pub renderer: String,
    /// Virtual output size in pixels.
    pub output: Size,
}

/// Result of `launch_app`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchOutcome {
    /// Id correlating the launch with the resulting `window_created` event.
    pub launch_id: LaunchId,
    /// Resolved application id.
    pub app_id: AppId,
    /// Process id, when known.
    #[serde(default)]
    pub pid: Option<i32>,
}

/// Result of `list_windows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowList {
    /// All known windows.
    pub windows: Vec<WindowInfo>,
    /// Active window, when one exists.
    #[serde(default)]
    pub active_window_id: Option<WindowId>,
}

/// Parameters of a `capture_window` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRequest {
    /// Window to capture.
    pub window_id: WindowId,
    /// Optional window-relative crop.
    #[serde(default)]
    pub region: Option<Rect>,
    /// Optional downscale target (longest edge).
    #[serde(default)]
    pub max_dimension: Option<u32>,
}

/// Result of `capture_window`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureOutcome {
    /// Encoded image (PNG base64 by default).
    pub image: ImagePayload,
    /// Window metadata at capture time.
    pub window: WindowInfo,
    /// Commit sequence the frame was rendered from.
    pub commit_seq: u64,
    /// Damage accumulated since the previous commit, simplified.
    #[serde(default)]
    pub changed_regions: Vec<Rect>,
}

/// Parameters of an `observe` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObserveRequest {
    /// Window to filter on; `None` = all windows.
    #[serde(default)]
    pub window_id: Option<WindowId>,
    /// Causal filter: only events after this action count.
    #[serde(default)]
    pub after_action: Option<ActionId>,
    /// Condition to wait for.
    pub until: ObserveCondition,
    /// Deadline for the wait.
    pub timeout_ms: u64,
    /// Whether the runtime should attach an image at resolution time.
    pub include_image: bool,
    /// Optional downscale target (longest edge).
    #[serde(default)]
    pub max_dimension: Option<u32>,
    /// Optional window-relative crop.
    #[serde(default)]
    pub region: Option<Rect>,
}

/// Result of `observe` / `wait_for_*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObserveOutcome {
    /// Temporal summary relative to `after_action`.
    pub observation: Observation,
    /// Image taken after the condition resolved, when requested.
    #[serde(default)]
    pub image: Option<ImagePayload>,
}

/// Parameters of a `click` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Window-relative position.
    pub position: Position,
    /// Mouse button.
    pub button: Button,
    /// Click count (1 = single, 2 = double).
    pub count: u32,
}

/// Parameters of a `scroll` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Window-relative position.
    pub position: Position,
    /// Horizontal axis delta.
    pub dx: f64,
    /// Vertical axis delta.
    pub dy: f64,
}

/// Result of `type_text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeOutcome {
    /// Runtime-assigned action id.
    pub action_id: ActionId,
    /// Characters that had no keymap entry and were skipped.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// Parameters of a `wait_for_events` request (`docs/protocol.md` §5.10).
///
/// The agent's idle primitive: block until at least one event matching `kinds`
/// (restricted to `window_id` when given) is published after the filter point,
/// or until `timeout_ms` elapses. A timed-out wait is a semantic outcome
/// ([`WaitOutcome::timed_out`]), never an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitForEventsRequest {
    /// Event kinds to collect; `None` means every emitted kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<EventKind>>,
    /// Restrict to events carrying this window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Upper bound on the wait in milliseconds.
    pub timeout_ms: u64,
    /// Maximum number of events to answer with.
    pub max_events: u32,
    /// Filter point: only events with `seq` greater than this count, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
}

impl WaitForEventsRequest {
    /// A wait over `timeout_ms` for every kind and every window, capped at the
    /// protocol default of 32 events.
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            kinds: None,
            window_id: None,
            timeout_ms,
            max_events: DEFAULT_WAIT_MAX_EVENTS,
            since_seq: None,
        }
    }

    /// The idle wake filter: only `notification` and `notification_action`
    /// events count as a wakeup ([`crate::watch`]'s default).
    pub fn wake(timeout_ms: u64) -> Self {
        Self::new(timeout_ms).kinds([EventKind::Notification, EventKind::NotificationAction])
    }

    /// Collect only `kinds`.
    pub fn kinds(mut self, kinds: impl IntoIterator<Item = EventKind>) -> Self {
        self.kinds = Some(kinds.into_iter().collect());
        self
    }

    /// Restrict to events carrying `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Cap the number of collected events.
    pub fn max_events(mut self, max_events: u32) -> Self {
        self.max_events = max_events;
        self
    }

    /// Count only events with `seq` greater than `since_seq`.
    pub fn since_seq(mut self, since_seq: u64) -> Self {
        self.since_seq = Some(since_seq);
        self
    }
}

/// Result of `wait_for_events` (`docs/protocol.md` §5.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitOutcome {
    /// Collected runtime events, oldest first, at most the requested `max_events`.
    pub events: Vec<RuntimeEvent>,
    /// Whether the timeout elapsed before any matching event arrived.
    pub timed_out: bool,
    /// Milliseconds from wait creation to resolution.
    pub elapsed_ms: u64,
    /// The runtime's current event watermark.
    pub seq: u64,
}

/// The AGP operations the agent needs, expressed in domain types.
///
/// Implementations: [`crate::agp::AgpClient`] (real socket) and the
/// `testing::ScriptedClient` fake. All methods return [`crate::Error`] so that
/// runtime errors keep their AGP `ErrorCode` for recovery classification.
#[async_trait]
pub trait AgentClient: Send + Sync {
    /// `ping` — runtime identity; the loop validates `protocol_version`.
    async fn ping(&self) -> Result<RuntimeInfo>;

    /// `list_apps` — application discovery, optionally filtered.
    async fn list_apps(&self, query: Option<&str>) -> Result<Vec<AppInfo>>;

    /// `launch_app` — resolve, spawn and correlate; returns immediately.
    async fn launch_app(&self, app_id: &AppId, args: &[String]) -> Result<LaunchOutcome>;

    /// `list_windows` — window metadata plus the active window.
    async fn list_windows(&self) -> Result<WindowList>;

    /// `get_window` — one window's metadata.
    async fn get_window(&self, window_id: WindowId) -> Result<WindowInfo>;

    /// `activate_window` — mutate compositor focus state (never synthetic input).
    async fn activate_window(&self, window_id: WindowId) -> Result<ActionId>;

    /// `close_window` — request window close.
    async fn close_window(&self, window_id: WindowId) -> Result<ActionId>;

    /// `capture_window` — force a render + readback, return pixels.
    async fn capture_window(&self, request: &CaptureRequest) -> Result<CaptureOutcome>;

    /// `observe` — wait for a condition and return a temporal observation.
    ///
    /// Covers `wait_for_change` / `wait_for_quiet` via [`ObserveCondition`].
    async fn observe(&self, request: &ObserveRequest) -> Result<ObserveOutcome>;

    /// `click` — pointer button press/release through the seat.
    async fn click(&self, request: &ClickRequest) -> Result<ActionId>;

    /// `scroll` — pointer axis events through the seat.
    async fn scroll(&self, request: &ScrollRequest) -> Result<ActionId>;

    /// `keypress` — key or chord through the seat.
    async fn keypress(&self, keys: &[String], window_id: Option<WindowId>) -> Result<ActionId>;

    /// `type_text` — UTF-8 text through the seat's keymap.
    async fn type_text(&self, text: &str, window_id: Option<WindowId>) -> Result<TypeOutcome>;

    /// `wait_for_events` — block until a matching event is published or the
    /// timeout elapses (`docs/protocol.md` §5.10).
    ///
    /// The runtime performs the wait; a timed-out result is reported through
    /// [`WaitOutcome::timed_out`], never as an error. This is the agent's idle
    /// primitive: [`crate::AgentLoop::run_watch`] uses it to sleep until a
    /// notification wakes it.
    async fn wait_for_events(&self, request: &WaitForEventsRequest) -> Result<WaitOutcome>;
}
