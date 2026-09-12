//! Event subscription methods (§5.6).

use adesk_core::WindowId;
use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::event::EventKind;

/// Params of `subscribe_events` (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeEventsParams {
    /// Kinds to receive; defaults to all fourteen filterable kinds (§5.6/§5.9).
    #[serde(default = "defaults::event_kinds")]
    pub kinds: Vec<EventKind>,
    /// Restrict the subscription to one window, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
}

impl Default for SubscribeEventsParams {
    fn default() -> SubscribeEventsParams {
        SubscribeEventsParams {
            kinds: EventKind::SUBSCRIBABLE.to_vec(),
            window_id: None,
        }
    }
}

/// Result of `subscribe_events` (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeEventsResult {
    /// Id used to cancel the subscription with `unsubscribe_events`.
    pub subscription_id: u64,
}

/// Params of `unsubscribe_events` (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeEventsParams {
    /// Subscription to cancel.
    pub subscription_id: u64,
}

/// Result of `unsubscribe_events` (§5.6) — the empty object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeEventsResult {}
