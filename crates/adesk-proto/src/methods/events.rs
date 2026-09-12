//! Event wait method (§5.10): one request that answers once a matching event has
//! been published (the pull counterpart of the §5.6 push subscription).

use adesk_core::WindowId;
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::event::EventKind;

/// Params of `wait_for_events` (§5.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitForEventsParams {
    /// Kinds to collect; defaults to all fourteen filterable kinds (§5.6/§5.9).
    #[serde(default = "defaults::event_kinds")]
    pub kinds: Vec<EventKind>,
    /// Restrict to events carrying this window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Upper bound on the wait (default `5000`).
    #[serde(default = "defaults::timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum number of events to answer with (default `32`).
    #[serde(default = "defaults::max_events")]
    pub max_events: u32,
    /// Filter point: only events with `seq` greater than this count, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
}

impl Default for WaitForEventsParams {
    fn default() -> WaitForEventsParams {
        WaitForEventsParams {
            kinds: EventKind::SUBSCRIBABLE.to_vec(),
            window_id: None,
            timeout_ms: defaults::timeout_ms(),
            max_events: defaults::max_events(),
            since_seq: None,
        }
    }
}

/// One collected event (§5.10): the same envelope as an event frame (§1) minus
/// the frame id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    /// The event kind.
    pub event: EventKind,
    /// Global monotonic event sequence.
    pub seq: u64,
    /// Monotonic milliseconds since runtime start.
    pub ts_ms: u64,
    /// The event `data` object (the variant fields, without the kind tag).
    pub data: serde_json::Value,
}

/// Result of `wait_for_events` (§5.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitForEventsResult {
    /// Collected events, oldest first, at most `max_events` of them.
    pub events: Vec<EventRecord>,
    /// Whether `timeout_ms` elapsed before any matching event arrived.
    pub timed_out: bool,
    /// Milliseconds from wait creation to resolution.
    pub elapsed_ms: u64,
    /// The runtime's current event watermark.
    pub seq: u64,
}
