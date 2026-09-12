//! Idle/watch mode: the agent stays idle and is awakened by events.
//!
//! A one-shot [`crate::AgentLoop::run`] drives a task to completion and stops. Watch
//! mode is the complement: an agent that *stands by* with a standing job
//! (a [`TaskDescription`](crate::context::TaskDescription)) and waits, without
//! busy-polling, for a notification or other event to wake it. The cycle is:
//!
//! ```text
//! idle wait (AGP §5.10 wait_for_events, bounded by wait_timeout_ms)
//!    │  timed out → still idle
//!    │  events    → wake: record the events into the provider's context,
//!    ▼              run the standing job (one bounded AgentLoop::run),
//! handle job         then return to idle
//!    │
//!    └─▶ idle wait …
//! ```
//!
//! The wake source is [`AgentClient::wait_for_events`](crate::AgentClient::wait_for_events):
//! the runtime performs the wait, so there is no sleep loop and no wall-clock
//! polling. Every wait is bounded by [`WatchConfig::wait_timeout_ms`]; the whole
//! mode is bounded by [`WatchConfig::max_wakeups`] and
//! [`WatchConfig::max_idle_waits`] (both optional).
//!
//! The first wait's filter point is [`WatchConfig::since_seq`] (`None` = only
//! events published after the watch starts; `Some(0)` = also deliver a
//! notification that was already pending). After every wait resolves,
//! [`crate::AgentLoop::run_watch`] advances the filter point to the runtime's
//! watermark at resolution ([`crate::WaitOutcome::seq`]), so an event published
//! between the end of one wait and the start of the next is never dropped.
//!
//! [`crate::AgentLoop::run_watch`] is driven from [`crate::agent_loop`]; the metrics
//! and step history of the loop are **cumulative across the whole watch run**,
//! so a report of the last handled wakeup carries every step taken since the
//! watch started.
//!
//! The wake→handle→idle cycle is implemented by
//! [`crate::AgentLoop::run_watch`].

use adesk_core::{EventKind, RuntimeEvent};
use serde::{Deserialize, Serialize};

use crate::agent_loop::LoopOutcome;

/// Idle/watch tuning knobs.
///
/// `None` on a budget field means **unbounded** — the watch loop runs until the
/// runtime disconnects or a fatal error, which is the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchConfig {
    /// Event kinds that count as a wakeup.
    ///
    /// Defaults to notifications only (`[Notification, NotificationAction]`).
    /// `None` wakes on every emitted kind.
    #[serde(default)]
    pub wake_kinds: Option<Vec<EventKind>>,
    /// Bound on a single idle wait, in milliseconds (default `30 000`).
    pub wait_timeout_ms: u64,
    /// Most events a single wait answers with (default `32`).
    pub max_events: u32,
    /// Stop after handling this many wakeups; `None` (the default) is unbounded.
    #[serde(default)]
    pub max_wakeups: Option<u32>,
    /// Stop after this many consecutive idle (timed-out) waits; `None` (the
    /// default) is unbounded.
    #[serde(default)]
    pub max_idle_waits: Option<u32>,
    /// Filter point for the **first** idle wait.
    ///
    /// `None` (the default) means "only events published after the watch
    /// starts": the runtime captures its watermark when the first wait begins,
    /// so a notification posted *before* the agent's first wait (e.g. while the
    /// process was starting) is not delivered. `Some(n)` additionally collects
    /// events already pending with `seq > n`, so `Some(0)` catches any pending
    /// notification.
    ///
    /// Only the first wait uses this value: [`crate::AgentLoop::run_watch`]
    /// advances the filter point to the runtime's watermark at each wait's
    /// resolution ([`crate::WaitOutcome::seq`]), so an event published between
    /// two waits is never dropped.
    #[serde(default)]
    pub since_seq: Option<u64>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            wake_kinds: Some(vec![EventKind::Notification, EventKind::NotificationAction]),
            wait_timeout_ms: 30_000,
            max_events: 32,
            max_wakeups: None,
            max_idle_waits: None,
            since_seq: None,
        }
    }
}

/// Why the watch loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchStopReason {
    /// [`WatchConfig::max_wakeups`] wakeups were handled.
    WakeupBudget,
    /// [`WatchConfig::max_idle_waits`] consecutive idle waits elapsed with no
    /// events.
    IdleBudget,
    /// A fatal error ended the watch. A fatal error from a wakeup's job is
    /// propagated as `Err` from [`crate::AgentLoop::run_watch`], so this variant
    /// is reserved for callers that surface a fatal stop as an outcome.
    Fatal,
}

/// One handled wakeup: the events that woke the agent and the job's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wakeup {
    /// The events that woke the agent (recorded into the handling context).
    pub events: Vec<RuntimeEvent>,
    /// The runtime's event watermark when the wait resolved.
    pub seq: u64,
    /// Outcome of the standing job the wakeup triggered.
    pub outcome: LoopOutcome,
}

/// Result of a whole watch run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchOutcome {
    /// Every handled wakeup, in order.
    pub wakeups: Vec<Wakeup>,
    /// Total number of idle (timed-out) waits observed.
    pub idle_waits: u32,
    /// Why the watch loop stopped.
    pub stop_reason: WatchStopReason,
}
