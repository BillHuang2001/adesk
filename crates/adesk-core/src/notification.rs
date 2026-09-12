//! The notification domain vocabulary (`docs/protocol.md` §4/§5.9).
//!
//! A notification is a structured, runtime-scoped event source: any AGP client
//! may post one, the runtime assigns it a monotonic [`NotificationId`], stores
//! it and publishes a `notification` [`crate::RuntimeEvent`]. This module carries
//! only the value types; the store and the event inbox live in `adesk-notify`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::NotificationId;

/// How urgent a notification is.
///
/// The wire names are the snake_case variants (`"low"`, `"normal"`,
/// `"critical"`); the default is [`NotificationUrgency::Normal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationUrgency {
    /// Low priority; may be presented unobtrusively.
    Low,
    /// Ordinary priority (the default).
    #[default]
    Normal,
    /// High priority; expected to demand attention.
    Critical,
}

/// A selectable action attached to a [`Notification`].
///
/// `key` is the stable identifier a consumer passes to
/// `invoke_notification_action`; `label` is the human-readable button text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    /// Stable action identifier used to invoke the action.
    pub key: String,
    /// Human-readable button label.
    pub label: String,
}

/// Why a notification left the active inbox.
///
/// The wire names are the snake_case variants (`"dismissed"`, `"action"`,
/// `"expired"`, `"closed"`); the default is
/// [`NotificationCloseReason::Dismissed`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCloseReason {
    /// The user dismissed it (the default).
    #[default]
    Dismissed,
    /// A consumer invoked one of its actions.
    Action,
    /// Its advisory timeout elapsed.
    Expired,
    /// It was closed programmatically without a more specific reason.
    Closed,
}

/// A stored notification as carried by the `notification` event and
/// `list_notifications` (`docs/protocol.md` §4/§5.9).
///
/// Field names are the wire names; optional metadata carries `null` when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Runtime-assigned identifier, monotonic and never reused.
    pub id: NotificationId,
    /// Originator name (`None` means an unnamed runtime source).
    pub source: Option<String>,
    /// Short summary; a notification with an empty title is invalid (§5.9).
    pub title: String,
    /// Longer body text, possibly empty.
    pub body: String,
    /// Priority hint.
    pub urgency: NotificationUrgency,
    /// Free-form class, e.g. `"message"`, `"email"`, `"progress"`.
    pub category: Option<String>,
    /// Selectable actions a consumer may invoke.
    pub actions: Vec<NotificationAction>,
    /// Opaque string key→value hints passed through verbatim.
    pub hints: BTreeMap<String, String>,
    /// Global event sequence of the `notification` event.
    pub posted_seq: u64,
    /// Monotonic ms since runtime start when the notification was posted.
    pub posted_ts_ms: u64,
    /// Whether the notification has been closed.
    pub dismissed: bool,
    /// Global event sequence of the `notification_closed` event, when closed.
    pub closed_seq: Option<u64>,
    /// Why it was closed, when closed.
    pub close_reason: Option<NotificationCloseReason>,
    /// Advisory lifetime hint in milliseconds (stored and echoed, not enforced).
    pub timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Notification {
        let mut hints = BTreeMap::new();
        hints.insert("sound-name".into(), "message-new-instant".into());
        Notification {
            id: NotificationId(5),
            source: Some("user".into()),
            title: "Build finished".into(),
            body: "The workspace compiled".into(),
            urgency: NotificationUrgency::Critical,
            category: Some("message".into()),
            actions: vec![
                NotificationAction {
                    key: "view".into(),
                    label: "View".into(),
                },
                NotificationAction {
                    key: "dismiss".into(),
                    label: "Dismiss".into(),
                },
            ],
            hints,
            posted_seq: 8300,
            posted_ts_ms: 64000,
            dismissed: false,
            closed_seq: None,
            close_reason: None,
            timeout_ms: Some(5000),
        }
    }

    #[test]
    fn defaults_are_normal_and_dismissed() {
        assert_eq!(NotificationUrgency::default(), NotificationUrgency::Normal);
        assert_eq!(
            NotificationCloseReason::default(),
            NotificationCloseReason::Dismissed
        );
    }

    #[test]
    fn urgency_wire_names() {
        let expected = [
            (NotificationUrgency::Low, "low"),
            (NotificationUrgency::Normal, "normal"),
            (NotificationUrgency::Critical, "critical"),
        ];
        for (value, name) in expected {
            assert_eq!(serde_json::to_value(value).unwrap(), serde_json::json!(name));
            assert_eq!(
                value,
                serde_json::from_value(serde_json::json!(name)).unwrap()
            );
        }
    }

    #[test]
    fn close_reason_wire_names() {
        let expected = [
            (NotificationCloseReason::Dismissed, "dismissed"),
            (NotificationCloseReason::Action, "action"),
            (NotificationCloseReason::Expired, "expired"),
            (NotificationCloseReason::Closed, "closed"),
        ];
        for (value, name) in expected {
            assert_eq!(serde_json::to_value(value).unwrap(), serde_json::json!(name));
            assert_eq!(
                value,
                serde_json::from_value(serde_json::json!(name)).unwrap()
            );
        }
    }

    #[test]
    fn notification_round_trips_through_serde() {
        let notification = sample();
        let json = serde_json::to_string(&notification).unwrap();
        let back: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, notification);
    }

    #[test]
    fn json_string_keys_are_snake_case() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["urgency"], serde_json::json!("critical"));
        assert_eq!(value["id"], serde_json::json!(5));
        assert_eq!(
            value["actions"][0],
            serde_json::json!({"key": "view", "label": "View"})
        );
        assert_eq!(
            value["hints"],
            serde_json::json!({"sound-name": "message-new-instant"})
        );
    }
}
