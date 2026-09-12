//! The synchronous notification store (`docs/protocol.md` §5.9).
//!
//! The store owns identifier allocation and notification lifecycle. It is mutated
//! **only** by the §5.9 request handlers ([`crate::NotificationService::post`],
//! [`crate::NotificationService::close`],
//! [`crate::NotificationService::invoke_action`]) and **never** by the event pump,
//! so a store mutation and the event it publishes can never disagree
//! (`docs/notifications.md`, `docs/architecture.md` §11).
//!
//! The store owns no clock and no event `seq` counter: both the `posted_seq`/
//! `closed_seq` and the `posted_ts_ms` are supplied by the server, which reserves
//! them from the compositor's single event counter before calling in.

use std::collections::BTreeMap;

use adesk_core::{
    Notification, NotificationAction, NotificationCloseReason, NotificationId, NotificationUrgency,
};

use crate::error::{Error, Result};

/// The accepted fields of `post_notification` minus the ids and sequences the
/// runtime assigns (`docs/protocol.md` §5.9).
///
/// Field names and defaults mirror the wire parameters exactly: `body` defaults
/// to the empty string, `urgency` to [`NotificationUrgency::Normal`], `actions`
/// to `[]` and `hints` to `{}`. An empty `title` is rejected by `post`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotification {
    /// Originator name (`None` means an unnamed runtime source).
    pub source: Option<String>,
    /// Short summary; must not be empty (§5.9).
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
    /// Advisory lifetime hint in milliseconds (stored and echoed, not enforced).
    pub timeout_ms: Option<u64>,
}

impl Default for NewNotification {
    fn default() -> Self {
        Self {
            source: None,
            title: String::new(),
            body: String::new(),
            urgency: NotificationUrgency::Normal,
            category: None,
            actions: Vec::new(),
            hints: BTreeMap::new(),
            timeout_ms: None,
        }
    }
}

impl NewNotification {
    /// A notification with the given title and every other field at its §5.9
    /// default.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// Sets the originator name.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Sets the body text.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets the priority hint.
    pub fn urgency(mut self, urgency: NotificationUrgency) -> Self {
        self.urgency = urgency;
        self
    }

    /// Sets the free-form category.
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Replaces the whole action list.
    pub fn actions(mut self, actions: Vec<NotificationAction>) -> Self {
        self.actions = actions;
        self
    }

    /// Appends one selectable action.
    pub fn action(mut self, key: impl Into<String>, label: impl Into<String>) -> Self {
        self.actions.push(NotificationAction {
            key: key.into(),
            label: label.into(),
        });
        self
    }

    /// Replaces the whole hint map.
    pub fn hints(mut self, hints: BTreeMap<String, String>) -> Self {
        self.hints = hints;
        self
    }

    /// Inserts one opaque hint.
    pub fn hint(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.hints.insert(key.into(), value.into());
        self
    }

    /// Sets the advisory timeout hint.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }
}

/// The result of `NotificationStore::close`.
///
/// `newly_dismissed` tells the §5.9 handler whether this call actually changed the
/// notification's state: only then does it publish a `notification_closed` event.
/// A no-op close still consumed the `seq` the server reserved for it, leaving a
/// harmless gap in the published sequence — the store never rewrites it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseOutcome {
    /// The notification after the (possibly no-op) close.
    pub notification: Notification,
    /// `true` when this call dismissed the notification; `false` when it was
    /// already dismissed.
    pub newly_dismissed: bool,
}

/// The runtime's notification store. `pub(crate)`: clients reach it through
/// [`crate::NotificationService`].
#[derive(Debug)]
pub(crate) struct NotificationStore {
    /// Next identifier to allocate; starts at `1` and never decreases. Independent
    /// of the event `seq` domain (`docs/protocol.md` §5.9).
    next_id: u64,
    /// Stored notifications by id (ascending).
    notifications: BTreeMap<NotificationId, Notification>,
}

impl NotificationStore {
    /// An empty store whose first allocation is `NotificationId(1)`.
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            notifications: BTreeMap::new(),
        }
    }

    /// Allocates the next id and stores a freshly posted notification.
    ///
    /// `seq`/`ts_ms` are the server-reserved global event sequence and the
    /// monotonic posting time; they become `posted_seq`/`posted_ts_ms`. An empty
    /// `title` is rejected with [`Error::InvalidRequest`] (§5.9) before any id is
    /// consumed.
    pub(crate) fn post(
        &mut self,
        new: NewNotification,
        seq: u64,
        ts_ms: u64,
    ) -> Result<Notification> {
        if new.title.is_empty() {
            return Err(Error::InvalidRequest(
                "notification title must not be empty".into(),
            ));
        }

        let notification = Notification {
            id: NotificationId(self.next_id),
            source: new.source,
            title: new.title,
            body: new.body,
            urgency: new.urgency,
            category: new.category,
            actions: new.actions,
            hints: new.hints,
            posted_seq: seq,
            posted_ts_ms: ts_ms,
            dismissed: false,
            closed_seq: None,
            close_reason: None,
            timeout_ms: new.timeout_ms,
        };
        self.next_id = self.next_id.saturating_add(1);
        self.notifications
            .insert(notification.id, notification.clone());
        Ok(notification)
    }

    /// Marks a notification dismissed, recording `closed_seq`/`close_reason`.
    ///
    /// An unknown id is [`Error::UnknownNotification`]. Closing an
    /// already-dismissed notification is a successful no-op: the unchanged
    /// notification is returned with `newly_dismissed = false` and the store keeps
    /// its original `closed_seq`/`close_reason` (§5.9).
    pub(crate) fn close(
        &mut self,
        id: NotificationId,
        reason: NotificationCloseReason,
        seq: u64,
    ) -> Result<CloseOutcome> {
        let notification = self
            .notifications
            .get_mut(&id)
            .ok_or(Error::UnknownNotification(id))?;

        if notification.dismissed {
            return Ok(CloseOutcome {
                notification: notification.clone(),
                newly_dismissed: false,
            });
        }

        notification.dismissed = true;
        notification.closed_seq = Some(seq);
        notification.close_reason = Some(reason);
        Ok(CloseOutcome {
            notification: notification.clone(),
            newly_dismissed: true,
        })
    }

    /// Validates a `invoke_notification_action` request (§5.9).
    ///
    /// An unknown id is [`Error::UnknownNotification`]; an `action_key` not present
    /// in the notification's `actions` is [`Error::InvalidRequest`]. The call does
    /// **not** dismiss the notification and records no sequence: the runtime
    /// performs no action of its own, it only reports the invocation.
    pub(crate) fn invoke_action(&self, id: NotificationId, action_key: &str) -> Result<()> {
        let notification = self
            .notifications
            .get(&id)
            .ok_or(Error::UnknownNotification(id))?;
        if !notification.actions.iter().any(|a| a.key == action_key) {
            return Err(Error::InvalidRequest(format!(
                "notification {id} has no action {action_key:?}"
            )));
        }
        Ok(())
    }

    /// Notifications newest first (`posted_seq` descending; ties broken by id),
    /// optionally including dismissed ones (§5.9).
    pub(crate) fn list(&self, include_dismissed: bool) -> Vec<Notification> {
        let mut out: Vec<Notification> = self
            .notifications
            .values()
            .filter(|n| include_dismissed || !n.dismissed)
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            b.posted_seq
                .cmp(&a.posted_seq)
                .then_with(|| b.id.cmp(&a.id))
        });
        out
    }

    /// A copy of the stored notification, or `None` when the id is unknown.
    pub(crate) fn get(&self, id: NotificationId) -> Option<Notification> {
        self.notifications.get(&id).cloned()
    }

    /// Number of stored notifications (dismissed included).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.notifications.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(store: &mut NotificationStore, title: &str, seq: u64, ts_ms: u64) -> Notification {
        store
            .post(NewNotification::new(title), seq, ts_ms)
            .expect("valid notification")
    }

    #[test]
    fn new_notification_defaults_match_the_protocol() {
        let default = NewNotification::default();
        assert_eq!(default.source, None);
        assert_eq!(default.title, "");
        assert_eq!(default.body, "");
        assert_eq!(default.urgency, NotificationUrgency::Normal);
        assert_eq!(default.category, None);
        assert!(default.actions.is_empty());
        assert!(default.hints.is_empty());
        assert_eq!(default.timeout_ms, None);

        let built = NewNotification::new("hello")
            .source("user")
            .body("world")
            .category("message")
            .action("open", "Open")
            .hint("urgency", "high")
            .timeout_ms(1_500);
        assert_eq!(built.title, "hello");
        assert_eq!(built.source.as_deref(), Some("user"));
        assert_eq!(built.body, "world");
        assert_eq!(built.category.as_deref(), Some("message"));
        assert_eq!(
            built.actions,
            vec![NotificationAction {
                key: "open".into(),
                label: "Open".into(),
            }]
        );
        assert_eq!(built.hints.get("urgency").map(String::as_str), Some("high"));
        assert_eq!(built.timeout_ms, Some(1_500));
    }

    #[test]
    fn post_sets_every_field_and_allocates_monotonic_ids() {
        let mut store = NotificationStore::new();

        let first = store
            .post(
                NewNotification::new("one")
                    .urgency(NotificationUrgency::Critical)
                    .body("b"),
                10,
                100,
            )
            .expect("valid");
        assert_eq!(first.id, NotificationId(1));
        assert_eq!(first.posted_seq, 10);
        assert_eq!(first.posted_ts_ms, 100);
        assert!(!first.dismissed);
        assert_eq!(first.closed_seq, None);
        assert_eq!(first.close_reason, None);
        assert_eq!(first.urgency, NotificationUrgency::Critical);
        assert_eq!(first.body, "b");

        let second = post(&mut store, "two", 11, 101);
        assert_eq!(second.id, NotificationId(2));
    }

    #[test]
    fn post_rejects_an_empty_title_without_consuming_an_id() {
        let mut store = NotificationStore::new();
        assert!(matches!(
            store.post(NewNotification::new(""), 5, 5),
            Err(Error::InvalidRequest(_))
        ));
        // The rejected post consumed no id: the first real post is id 1.
        assert_eq!(post(&mut store, "ok", 6, 6).id, NotificationId(1));
    }

    #[test]
    fn ids_are_never_reused_across_post_close_post() {
        let mut store = NotificationStore::new();
        let a = post(&mut store, "a", 1, 1);
        let b = post(&mut store, "b", 2, 2);
        store
            .close(a.id, NotificationCloseReason::Dismissed, 3)
            .expect("known id");
        let c = post(&mut store, "c", 4, 4);

        assert_eq!(a.id, NotificationId(1));
        assert_eq!(b.id, NotificationId(2));
        assert_eq!(c.id, NotificationId(3), "a closed id is never reused");
    }

    #[test]
    fn list_orders_newest_first_and_filters_dismissed() {
        let mut store = NotificationStore::new();
        let a = post(&mut store, "a", 1, 1);
        let b = post(&mut store, "b", 2, 2);
        let c = post(&mut store, "c", 3, 3);
        store
            .close(b.id, NotificationCloseReason::Action, 4)
            .expect("known id");

        let live: Vec<NotificationId> = store.list(false).into_iter().map(|n| n.id).collect();
        assert_eq!(live, vec![c.id, a.id], "newest first, dismissed excluded");

        let all: Vec<NotificationId> = store.list(true).into_iter().map(|n| n.id).collect();
        assert_eq!(all, vec![c.id, b.id, a.id]);
    }

    #[test]
    fn list_orders_by_posted_seq_even_when_ids_agree_on_order() {
        let mut store = NotificationStore::new();
        // Ids are allocated in call order; give the later id a *lower* posted_seq to
        // prove ordering follows `posted_seq`, not the id.
        let first = post(&mut store, "first", 100, 1);
        let second = post(&mut store, "second", 50, 1);

        let ordered: Vec<NotificationId> = store.list(true).into_iter().map(|n| n.id).collect();
        assert_eq!(ordered, vec![first.id, second.id]);
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn close_unknown_id_is_an_error() {
        let mut store = NotificationStore::new();
        assert!(matches!(
            store.close(NotificationId(9), NotificationCloseReason::Closed, 1),
            Err(Error::UnknownNotification(id)) if id == NotificationId(9)
        ));
    }

    #[test]
    fn close_dismisses_and_reports_newly_dismissed() {
        let mut store = NotificationStore::new();
        let n = post(&mut store, "hello", 7, 42);

        let outcome = store
            .close(n.id, NotificationCloseReason::Expired, 8)
            .expect("known id");
        assert!(outcome.newly_dismissed);
        assert!(outcome.notification.dismissed);
        assert_eq!(outcome.notification.closed_seq, Some(8));
        assert_eq!(
            outcome.notification.close_reason,
            Some(NotificationCloseReason::Expired)
        );
        assert_eq!(
            store.get(n.id).expect("stored").close_reason,
            Some(NotificationCloseReason::Expired)
        );
    }

    #[test]
    fn second_close_is_a_noop() {
        let mut store = NotificationStore::new();
        let n = post(&mut store, "hello", 7, 42);
        store
            .close(n.id, NotificationCloseReason::Dismissed, 8)
            .expect("known id");

        let second = store
            .close(n.id, NotificationCloseReason::Action, 99)
            .expect("known id");
        assert!(!second.newly_dismissed);
        assert_eq!(
            second.notification.closed_seq,
            Some(8),
            "the first close's seq is preserved"
        );
        assert_eq!(
            second.notification.close_reason,
            Some(NotificationCloseReason::Dismissed),
            "the first close's reason is preserved"
        );
    }

    #[test]
    fn invoke_action_validates_id_and_key_without_dismissing() {
        let mut store = NotificationStore::new();
        let n = store
            .post(
                NewNotification::new("hello")
                    .action("open", "Open")
                    .action("snooze", "Snooze"),
                1,
                1,
            )
            .expect("valid");

        assert!(store.invoke_action(n.id, "open").is_ok());

        assert!(matches!(
            store.invoke_action(n.id, "delete"),
            Err(Error::InvalidRequest(_))
        ));
        assert!(matches!(
            store.invoke_action(NotificationId(999), "open"),
            Err(Error::UnknownNotification(_))
        ));

        let stored = store.get(n.id).expect("stored");
        assert!(!stored.dismissed, "invoking an action never dismisses");
        assert_eq!(stored.closed_seq, None);
    }

    #[test]
    fn get_is_a_copy_and_none_for_unknown() {
        let mut store = NotificationStore::new();
        let n = post(&mut store, "hello", 1, 1);
        assert_eq!(store.get(n.id), Some(n));
        assert_eq!(store.get(NotificationId(404)), None);
    }

    #[test]
    fn len_counts_every_stored_notification() {
        let mut store = NotificationStore::new();
        assert_eq!(store.len(), 0);
        let a = post(&mut store, "a", 1, 1);
        post(&mut store, "b", 2, 2);
        store
            .close(a.id, NotificationCloseReason::Dismissed, 3)
            .expect("known id");
        assert_eq!(store.len(), 2, "dismissed notifications stay in the store");
    }
}
