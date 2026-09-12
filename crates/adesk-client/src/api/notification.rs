//! AGP §5.9 — notifications: the runtime's programmable event source.
//!
//! A notification is a structured, runtime-scoped event source: any client may
//! post one, the runtime assigns it a monotonic [`NotificationId`], stores it
//! and publishes a `notification` event (§5.6). Notifications are not
//! connection-scoped, so every connection sees every notification and may close
//! or invoke actions on any of them.
//!
//! The value vocabulary ([`Notification`], [`NotificationAction`],
//! [`NotificationUrgency`], [`NotificationCloseReason`], [`NotificationId`])
//! lives in `adesk-core` and is used here directly.

use std::collections::BTreeMap;

use adesk_core::{
    Notification, NotificationAction, NotificationCloseReason, NotificationId, NotificationUrgency,
};
use serde::{Deserialize, Serialize};

use crate::{Client, Result};

/// `post_notification` params (protocol §5.9).
///
/// `title` is required and must not be empty (the runtime rejects an empty
/// title with `invalid_request`). `body`, `urgency`, `actions` and `hints`
/// carry serde defaults that the runtime fills identically; `source`,
/// `category` and `timeout_ms` are omitted when unset.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PostNotificationRequest {
    /// Originator name; `None` means an unnamed runtime source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Short summary (required; must not be empty).
    pub title: String,
    /// Longer body text (default `""`).
    pub body: String,
    /// Priority hint (default `normal`).
    pub urgency: NotificationUrgency,
    /// Free-form class, e.g. `"message"`, `"email"`, `"progress"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Selectable actions a consumer may invoke (default `[]`).
    pub actions: Vec<NotificationAction>,
    /// Opaque string key→value hints passed through verbatim (default `{}`).
    pub hints: BTreeMap<String, String>,
    /// Advisory lifetime hint in milliseconds, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl PostNotificationRequest {
    /// A notification with an empty body, normal urgency and no metadata.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            source: None,
            title: title.into(),
            body: String::new(),
            urgency: NotificationUrgency::Normal,
            category: None,
            actions: Vec::new(),
            hints: BTreeMap::new(),
            timeout_ms: None,
        }
    }

    /// Name the originator.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the longer body text.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Set the priority hint.
    pub fn urgency(mut self, urgency: NotificationUrgency) -> Self {
        self.urgency = urgency;
        self
    }

    /// Set the free-form class.
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Append a selectable action (`key` invokes it, `label` is its button text).
    pub fn action(mut self, key: impl Into<String>, label: impl Into<String>) -> Self {
        self.actions.push(NotificationAction {
            key: key.into(),
            label: label.into(),
        });
        self
    }

    /// Insert an opaque string hint passed through verbatim.
    pub fn hint(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.hints.insert(key.into(), value.into());
        self
    }

    /// Set the advisory lifetime hint.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }
}

/// `list_notifications` params (protocol §5.9).
#[derive(Debug, Clone, Copy, Serialize)]
struct ListNotificationsParams {
    /// Include dismissed notifications (default `false`).
    include_dismissed: bool,
}

/// `list_notifications` result envelope.
#[derive(Debug, Clone, Deserialize)]
struct ListNotificationsResult {
    notifications: Vec<Notification>,
}

/// `close_notification` params (protocol §5.9).
#[derive(Debug, Clone, Copy, Serialize)]
struct CloseNotificationParams {
    notification_id: NotificationId,
    reason: NotificationCloseReason,
}

/// `invoke_notification_action` params (protocol §5.9).
#[derive(Debug, Clone, Serialize)]
struct InvokeNotificationActionParams<'a> {
    notification_id: NotificationId,
    action_key: &'a str,
}

/// Result of `post_notification` (protocol §5.9).
#[derive(Debug, Clone, Copy, Deserialize)]
#[non_exhaustive]
pub struct PostNotificationResult {
    /// Runtime-assigned notification id (monotonic, never reused).
    pub notification_id: NotificationId,
    /// Global event sequence of the resulting `notification` event.
    pub seq: u64,
}

/// Result of `close_notification` (protocol §5.9).
#[derive(Debug, Clone, Copy, Deserialize)]
#[non_exhaustive]
pub struct CloseNotificationResult {
    /// The dismissed notification.
    pub notification_id: NotificationId,
    /// Global event sequence of the `notification_closed` event.
    pub seq: u64,
}

/// Result of `invoke_notification_action` (protocol §5.9).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct InvokeNotificationActionResult {
    /// The notification whose action was invoked.
    pub notification_id: NotificationId,
    /// The invoked action's key.
    pub action_key: String,
    /// Global event sequence of the `notification_action` event.
    pub seq: u64,
}

impl Client {
    /// `post_notification` — inject a notification into the runtime inbox.
    ///
    /// The runtime assigns a [`NotificationId`] and publishes a `notification`
    /// event carrying the resulting [`Notification`] (§5.9).
    pub async fn post_notification(
        &self,
        request: PostNotificationRequest,
    ) -> Result<PostNotificationResult> {
        self.request("post_notification", &request).await
    }

    /// `list_notifications` — the stored notifications, newest first.
    ///
    /// Returns the non-dismissed notifications unless `include_dismissed` is
    /// set (protocol §5.9).
    pub async fn list_notifications(&self, include_dismissed: bool) -> Result<Vec<Notification>> {
        let result: ListNotificationsResult = self
            .request(
                "list_notifications",
                &ListNotificationsParams { include_dismissed },
            )
            .await?;
        Ok(result.notifications)
    }

    /// `close_notification` — dismiss a notification.
    ///
    /// Publishes a `notification_closed` event carrying `reason`; closing an
    /// already-dismissed notification is a successful no-op (§5.9).
    pub async fn close_notification(
        &self,
        notification_id: NotificationId,
        reason: NotificationCloseReason,
    ) -> Result<CloseNotificationResult> {
        self.request(
            "close_notification",
            &CloseNotificationParams {
                notification_id,
                reason,
            },
        )
        .await
    }

    /// `invoke_notification_action` — report that an action was invoked.
    ///
    /// Publishes a `notification_action` event; the runtime performs no action
    /// of its own and does not dismiss the notification (§5.9). An `action_key`
    /// not present in the notification's actions fails with `invalid_request`.
    pub async fn invoke_notification_action(
        &self,
        notification_id: NotificationId,
        action_key: impl Into<String>,
    ) -> Result<InvokeNotificationActionResult> {
        let action_key = action_key.into();
        self.request(
            "invoke_notification_action",
            &InvokeNotificationActionParams {
                notification_id,
                action_key: &action_key,
            },
        )
        .await
    }
}
