//! AGP §5.10 — event waits: one request that answers once a matching event has
//! been published (the pull counterpart of the §5.6 push subscription).
//!
//! `wait_for_events` is the agent's idle primitive: it blocks until at least one
//! event matching the filter has been published after the filter point, or until
//! the timeout elapses, then answers with the collected events and the runtime's
//! current event watermark. Each collected event is mapped onto the crate's
//! [`AgpEvent`] vocabulary, so a waiter and a `subscribe_events` subscription
//! describe an event identically.

use adesk_core::WindowId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::capture::DEFAULT_TIMEOUT_MS;
use crate::events::{agp_event_from_raw, AgpEvent, EventKind};
use crate::wire::RawEvent;
use crate::{Client, Result};

/// Default `max_events` for `wait_for_events` (protocol §5.10).
pub const DEFAULT_MAX_EVENTS: u32 = 32;

/// `wait_for_events` params (protocol §5.10).
///
/// `kinds` restricts which event kinds count (`None` means every emitted kind,
/// the §5.6 default). `window_id` restricts to events carrying that window;
/// `since_seq` sets the filter point (only events with `seq` greater than it
/// count) — `None` means "capture the runtime watermark when the wait begins".
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WaitForEventsRequest {
    /// Kinds to collect; `None` means every emitted kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<EventKind>>,
    /// Restrict to events carrying this window, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
    /// Upper bound on the wait (default [`DEFAULT_TIMEOUT_MS`]).
    pub timeout_ms: u64,
    /// Maximum number of events to answer with (default [`DEFAULT_MAX_EVENTS`]).
    pub max_events: u32,
    /// Filter point: only events with `seq` greater than this count, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
}

impl Default for WaitForEventsRequest {
    fn default() -> Self {
        Self {
            kinds: None,
            window_id: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_events: DEFAULT_MAX_EVENTS,
            since_seq: None,
        }
    }
}

impl WaitForEventsRequest {
    /// A wait for every kind, every window, with protocol defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Collect only the given event kinds.
    pub fn kinds(mut self, kinds: impl IntoIterator<Item = EventKind>) -> Self {
        self.kinds = Some(kinds.into_iter().collect());
        self
    }

    /// Restrict to events carrying `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Override the wait bound.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
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

/// Result of `wait_for_events` (protocol §5.10).
///
/// Each collected event is a typed [`AgpEvent`]; a record whose kind this client
/// does not model is preserved as [`AgpEvent::Other`] (protocol §7 forward
/// compatibility) instead of failing the whole response.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WaitForEventsResult {
    /// Collected events, oldest first, at most `max_events` of them.
    pub events: Vec<AgpEvent>,
    /// Whether the timeout elapsed before any matching event arrived.
    pub timed_out: bool,
    /// Milliseconds from wait creation to resolution.
    pub elapsed_ms: u64,
    /// The runtime's current event watermark.
    pub seq: u64,
}

/// One wire `EventRecord` (§5.10): the same envelope as an event frame.
///
/// `event` is decoded as a plain string so an unknown/future kind degrades to
/// [`AgpEvent::Other`] rather than failing deserialisation.
#[derive(Debug, Deserialize)]
struct WireEventRecord {
    event: String,
    seq: u64,
    ts_ms: u64,
    #[serde(default = "empty_object")]
    data: Value,
}

/// `wait_for_events` result envelope (protocol §5.10).
#[derive(Debug, Deserialize)]
struct WireWaitForEventsResult {
    events: Vec<WireEventRecord>,
    timed_out: bool,
    elapsed_ms: u64,
    seq: u64,
}

/// Default `data` for an event record that omits it: an empty object, matching
/// the lenient wire treatment of an event frame (§7).
fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

impl Client {
    /// `wait_for_events` — block until a matching event is published or the
    /// timeout elapses.
    ///
    /// The runtime performs the wait; the client only sends the request. A
    /// `timed_out = true` result is a semantic outcome (the horizon was
    /// reached), not an error.
    pub async fn wait_for_events(
        &self,
        request: WaitForEventsRequest,
    ) -> Result<WaitForEventsResult> {
        let wire: WireWaitForEventsResult = self.request("wait_for_events", &request).await?;
        Ok(WaitForEventsResult {
            events: wire
                .events
                .into_iter()
                .map(|record| {
                    agp_event_from_raw(RawEvent {
                        name: record.event,
                        seq: record.seq,
                        ts_ms: record.ts_ms,
                        data: record.data,
                    })
                })
                .collect(),
            timed_out: wire.timed_out,
            elapsed_ms: wire.elapsed_ms,
            seq: wire.seq,
        })
    }
}
