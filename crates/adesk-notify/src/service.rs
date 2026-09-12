//! [`NotificationService`] — the crate's facade, composing the store and the inbox.
//!
//! Threading model (`docs/architecture.md` §11):
//!
//! - The service is a cheap, cloneable handle (`Arc` inside). One clone lives in
//!   the server's event-pump task and calls [`NotificationService::handle_event`]
//!   for every broadcast [`RuntimeEvent`]; every request task holds another clone.
//! - The two concerns sit behind **two separate** `std::sync::Mutex`es so a store
//!   mutation and an inbox feed never contend. Critical sections are short and
//!   **never held across `.await`** — the only await points are
//!   `tokio::sync::watch::Receiver::changed()` and `tokio::time::sleep_until`,
//!   both inside the inbox.
//! - The store is mutated **only** by [`NotificationService::post`],
//!   [`NotificationService::close`] and [`NotificationService::invoke_action`],
//!   never by [`NotificationService::handle_event`].
//! - The service spawns no background tasks: the caller drives it, exactly like
//!   the observer.

use std::sync::{Arc, Mutex, MutexGuard};

use adesk_core::{Notification, NotificationCloseReason, NotificationId, RuntimeEvent};

use crate::error::Result;
use crate::inbox::{EventBatch, EventInbox, EventWaitSpec};
use crate::store::{CloseOutcome, NewNotification, NotificationStore};
use crate::DEFAULT_INBOX_CAPACITY;

/// The runtime's notification store and event inbox (cloneable handle).
///
/// One instance is owned by `ServerContext`; every clone shares the same state.
#[derive(Debug, Clone)]
pub struct NotificationService {
    inner: Arc<Inner>,
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared service state. `pub(crate)` so the tests can inspect it.
#[derive(Debug)]
pub(crate) struct Inner {
    /// Notification store (its own mutex, never taken while the inbox is held).
    pub(crate) store: Mutex<NotificationStore>,
    /// Event inbox: bounded journal + generation counter.
    pub(crate) inbox: EventInbox,
}

/// Locks the store mutex, ignoring poisoning.
///
/// A poisoned lock (another thread panicked while holding it) still holds usable
/// data; panicking on every later request would be far worse than reading it.
fn lock_store(inner: &Inner) -> MutexGuard<'_, NotificationStore> {
    inner
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl NotificationService {
    /// Fresh service with protocol defaults (inbox capacity
    /// [`DEFAULT_INBOX_CAPACITY`], empty store).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                store: Mutex::new(NotificationStore::new()),
                inbox: EventInbox::new(DEFAULT_INBOX_CAPACITY),
            }),
        }
    }

    /// Implements `post_notification` (`docs/protocol.md` §5.9): allocates the
    /// next id, stores the notification and returns it for publication.
    ///
    /// `seq`/`ts_ms` are reserved by the server (this crate owns no seq counter
    /// and no clock). An empty `title` is [`crate::Error::InvalidRequest`].
    pub fn post(&self, new: NewNotification, seq: u64, ts_ms: u64) -> Result<Notification> {
        let notification = lock_store(&self.inner).post(new, seq, ts_ms)?;
        tracing::debug!(
            notification_id = %notification.id,
            posted_seq = seq,
            "notify: notification posted"
        );
        Ok(notification)
    }

    /// Implements `close_notification` (`docs/protocol.md` §5.9).
    ///
    /// `seq` is reserved by the server. When [`CloseOutcome::newly_dismissed`] is
    /// `false` the notification was already dismissed, the server publishes
    /// nothing, and the reserved `seq` is simply skipped — a harmless gap in the
    /// published sequence, never a rewrite.
    pub fn close(
        &self,
        id: NotificationId,
        reason: NotificationCloseReason,
        seq: u64,
    ) -> Result<CloseOutcome> {
        let outcome = lock_store(&self.inner).close(id, reason, seq)?;
        tracing::debug!(
            notification_id = %id,
            newly_dismissed = outcome.newly_dismissed,
            closed_seq = seq,
            "notify: notification closed"
        );
        Ok(outcome)
    }

    /// Implements `invoke_notification_action` (`docs/protocol.md` §5.9).
    ///
    /// Validates the id and the action key only; the notification stays live and
    /// the server publishes the `notification_action` event.
    pub fn invoke_action(&self, id: NotificationId, action_key: &str) -> Result<()> {
        lock_store(&self.inner).invoke_action(id, action_key)?;
        tracing::debug!(
            notification_id = %id,
            action_key,
            "notify: notification action invoked"
        );
        Ok(())
    }

    /// Implements `list_notifications` (`docs/protocol.md` §5.9): newest first
    /// (`posted_seq` descending), dismissed ones only when `include_dismissed`.
    pub fn list(&self, include_dismissed: bool) -> Vec<Notification> {
        lock_store(&self.inner).list(include_dismissed)
    }

    /// A copy of the stored notification, or `None` when the id is unknown.
    pub fn get(&self, id: NotificationId) -> Option<Notification> {
        lock_store(&self.inner).get(id)
    }

    /// Feeds one broadcast event into the inbox. Called by the server's event-pump
    /// task for every event, in `seq` order. Never touches the store.
    pub fn handle_event(&self, event: &RuntimeEvent) {
        self.inner.inbox.handle_event(event);
    }

    /// Implements `wait_for_events` (`docs/protocol.md` §5.10): resolves with the
    /// matching events after the filter point, or a timeout.
    pub async fn wait_for_events(&self, spec: EventWaitSpec) -> Result<EventBatch> {
        self.inner.inbox.wait_for_events(spec).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::EventWaitSpec;
    use adesk_core::{EventKind, NotificationUrgency, WindowId};

    fn created(seq: u64) -> RuntimeEvent {
        RuntimeEvent::WindowCreated {
            seq,
            ts_ms: seq,
            window_id: WindowId(1),
            app_id: None,
            pid: None,
            launch_id: None,
            title: None,
        }
    }

    #[test]
    fn new_matches_default() {
        let service = NotificationService::default();
        assert!(service.list(true).is_empty());
        assert_eq!(service.get(NotificationId(1)), None);
    }

    #[test]
    fn store_mutations_flow_through_the_facade() {
        let service = NotificationService::new();

        let posted = service
            .post(
                NewNotification::new("Build finished")
                    .source("user")
                    .urgency(NotificationUrgency::Critical)
                    .action("open", "Open"),
                10,
                55,
            )
            .expect("valid");
        assert_eq!(posted.id, NotificationId(1));
        assert_eq!(posted.posted_seq, 10);
        assert_eq!(posted.posted_ts_ms, 55);
        assert_eq!(service.get(posted.id), Some(posted.clone()));

        service
            .invoke_action(posted.id, "open")
            .expect("known action");
        assert!(service.invoke_action(posted.id, "gone").is_err());

        let outcome = service
            .close(posted.id, NotificationCloseReason::Action, 11)
            .expect("known id");
        assert!(outcome.newly_dismissed);
        assert!(service.list(false).is_empty(), "dismissed are excluded");
        assert_eq!(service.list(true).len(), 1);

        // A second close is a no-op that reports it.
        assert!(
            !service
                .close(posted.id, NotificationCloseReason::Closed, 12)
                .expect("known id")
                .newly_dismissed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clones_share_state_across_store_and_inbox() {
        let service = NotificationService::new();
        let clone = service.clone();

        // Store state is shared.
        clone.post(NewNotification::new("hi"), 1, 1).expect("valid");
        assert_eq!(service.list(true).len(), 1);

        // Inbox state is shared: an event fed through one clone is delivered by a
        // wait on the other.
        clone.handle_event(&created(1));
        let batch = service
            .wait_for_events(
                EventWaitSpec::new()
                    .kind(EventKind::WindowCreated)
                    .since_seq(0),
            )
            .await
            .expect("infallible");
        assert_eq!(batch.events.len(), 1);
        assert!(!batch.timed_out);
    }

    #[tokio::test(start_paused = true)]
    async fn handle_event_never_mutates_the_store() {
        let service = NotificationService::new();
        // Feed events that are not notifications: the store stays empty.
        service.handle_event(&created(1));
        assert!(service.list(true).is_empty());
        assert_eq!(service.get(NotificationId(1)), None);
    }
}
