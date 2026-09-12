//! Notification methods (§5.9): the runtime's programmable event source.

use std::collections::BTreeMap;

use adesk_core::{
    Notification, NotificationAction, NotificationCloseReason, NotificationId, NotificationUrgency,
};
use serde::{Deserialize, Serialize};

/// Params of `post_notification` (§5.9).
///
/// A notification with an empty `title` is invalid (§5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostNotificationParams {
    /// Originator name; `null`/omitted means an unnamed runtime source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Short summary (required; must not be empty).
    pub title: String,
    /// Longer body text (default `""`).
    #[serde(default)]
    pub body: String,
    /// Priority hint (default `"normal"`).
    #[serde(default)]
    pub urgency: NotificationUrgency,
    /// Free-form class, e.g. `"message"`, `"email"`, `"progress"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Selectable actions a consumer may invoke (default `[]`).
    #[serde(default)]
    pub actions: Vec<NotificationAction>,
    /// Opaque string key→value hints passed through verbatim (default `{}`).
    #[serde(default)]
    pub hints: BTreeMap<String, String>,
    /// Advisory lifetime hint in milliseconds, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Result of `post_notification` (§5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostNotificationResult {
    /// Runtime-assigned notification id (monotonic, never reused).
    pub notification_id: NotificationId,
    /// Global event sequence of the resulting `notification` event.
    pub seq: u64,
}

/// Params of `list_notifications` (§5.9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListNotificationsParams {
    /// Include dismissed notifications (default `false`).
    #[serde(default)]
    pub include_dismissed: bool,
}

/// Result of `list_notifications` (§5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListNotificationsResult {
    /// Non-dismissed notifications, newest first (`posted_seq` descending).
    pub notifications: Vec<Notification>,
}

/// Params of `close_notification` (§5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseNotificationParams {
    /// Notification to dismiss.
    pub notification_id: NotificationId,
    /// Why it is being closed (default `"dismissed"`).
    #[serde(default)]
    pub reason: NotificationCloseReason,
}

/// Result of `close_notification` (§5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseNotificationResult {
    /// The dismissed notification.
    pub notification_id: NotificationId,
    /// Global event sequence of the `notification_closed` event.
    pub seq: u64,
}

/// Params of `invoke_notification_action` (§5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvokeNotificationActionParams {
    /// Notification whose action is invoked.
    pub notification_id: NotificationId,
    /// Action to invoke; must be present in the notification's `actions` (§5.9).
    pub action_key: String,
}

/// Result of `invoke_notification_action` (§5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvokeNotificationActionResult {
    /// The notification whose action was invoked.
    pub notification_id: NotificationId,
    /// The invoked action's key.
    pub action_key: String,
    /// Global event sequence of the `notification_action` event.
    pub seq: u64,
}
