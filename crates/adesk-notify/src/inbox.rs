//! The event inbox: a bounded journal of [`RuntimeEvent`]s plus the async
//! `wait_for_events` waiters (`docs/protocol.md` §5.10).
//!
//! The inbox is fed by the server's event-pump task ([`EventInbox::handle_event`],
//! one event per call in `seq` order), exactly like the observer — but where the
//! observer ignores the notification kinds (they are not counted), the inbox
//! records **every** kind so `wait_for_events` can deliver notifications too
//! (`docs/architecture.md` §11).
//!
//! Waits are woken by a `tokio::sync::watch` generation counter that
//! `handle_event` bumps. A waiter subscribes *before* it reads state and re-checks
//! the journal after every wakeup, so no matching event can be lost and no wait
//! ever polls. Time is `tokio::time`, which makes `timeout_ms` deterministic under
//! `tokio::time::pause()`.
//!
//! The batch carries [`RuntimeEvent`]s, **not** a `serde_json::Value`: this crate
//! must not depend on `serde_json` (dependencies are fixed), so the server/proto
//! layer assembles each wire `EventRecord { event, seq, ts_ms, data }` from the
//! event itself.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use adesk_core::{EventKind, RuntimeEvent, WindowId};
use tokio::sync::watch;

use crate::error::Result;
use crate::{DEFAULT_MAX_EVENTS, DEFAULT_TIMEOUT_MS};

/// Parameters of `wait_for_events` (`docs/protocol.md` §5.10).
///
/// The filter point is `since_seq` when given, else the inbox watermark captured
/// when the wait began. An empty `kinds` means "every emitted kind"; a
/// `window_id` restricts delivery to events carrying that window (window-less
/// events such as `app_launched` and notifications are then excluded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventWaitSpec {
    /// Kinds to deliver; empty means every kind.
    pub kinds: Vec<EventKind>,
    /// Restrict delivery to one window; `None` = any window (and window-less
    /// events).
    pub window_id: Option<WindowId>,
    /// Hard upper bound of the wait; on expiry the batch has `timed_out: true`.
    pub timeout_ms: u64,
    /// Maximum number of events to return (oldest first).
    pub max_events: u32,
    /// Filter point: only events with `seq > since_seq` count. `None` = the
    /// watermark at wait start.
    pub since_seq: Option<u64>,
}

impl Default for EventWaitSpec {
    fn default() -> Self {
        Self {
            kinds: Vec::new(),
            window_id: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_events: DEFAULT_MAX_EVENTS,
            since_seq: None,
        }
    }
}

impl EventWaitSpec {
    /// Spec with protocol defaults (`timeout_ms = 5000`, `max_events = 32`, no
    /// filters).
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the kind filter with `kinds`.
    pub fn kinds(mut self, kinds: Vec<EventKind>) -> Self {
        self.kinds = kinds;
        self
    }

    /// Appends one kind to the kind filter.
    pub fn kind(mut self, kind: EventKind) -> Self {
        self.kinds.push(kind);
        self
    }

    /// Restricts delivery to `window_id`.
    pub fn window(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    /// Overrides the timeout.
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Overrides the batch size cap.
    pub fn max_events(mut self, max_events: u32) -> Self {
        self.max_events = max_events;
        self
    }

    /// Overrides the filter point.
    pub fn since_seq(mut self, since_seq: u64) -> Self {
        self.since_seq = Some(since_seq);
        self
    }

    /// `true` when `event` passes the kind and window filters.
    pub(crate) fn matches(&self, event: &RuntimeEvent) -> bool {
        if let Some(window_id) = self.window_id {
            if event.window_id() != Some(window_id) {
                return false;
            }
        }
        self.kinds.is_empty() || self.kinds.contains(&event.kind())
    }
}

/// The result of `wait_for_events` (`docs/protocol.md` §5.10).
///
/// `events` is the matching window of the inbox, oldest first and at most
/// `max_events` long. `seq` is the inbox watermark at resolution: when
/// `events.len() == max_events` the caller should resume from the last returned
/// event's `seq` to avoid a truncation gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBatch {
    /// Matching events, oldest first.
    pub events: Vec<RuntimeEvent>,
    /// `true` when the horizon was reached before any matching event arrived (in
    /// which case `events` is empty).
    pub timed_out: bool,
    /// Monotonic ms from wait creation to resolution.
    pub elapsed_ms: u64,
    /// The inbox watermark at resolution.
    pub seq: u64,
}

/// Milliseconds since `started`, saturating instead of wrapping.
fn elapsed_ms(started: tokio::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The bounded event journal and its watermark. Pure data + sync helpers; the
/// waiting loop lives on [`EventInbox`].
#[derive(Debug)]
pub(crate) struct InboxState {
    /// Most recent events, oldest first.
    journal: VecDeque<RuntimeEvent>,
    /// Retained-event capacity, at least `1`.
    capacity: usize,
    /// Highest `seq` ever recorded; only ever raised.
    watermark: u64,
}

impl InboxState {
    /// An empty journal retaining at most `capacity` events.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            // One spare slot so `record` can append before evicting without ever
            // growing past its allocation.
            journal: VecDeque::with_capacity(capacity.saturating_add(1)),
            capacity,
            watermark: 0,
        }
    }

    /// Appends an event, raising the watermark and evicting the oldest when full.
    fn record(&mut self, event: &RuntimeEvent) {
        self.watermark = self.watermark.max(event.seq());
        self.journal.push_back(event.clone());
        if self.journal.len() > self.capacity {
            self.journal.pop_front();
        }
    }

    /// The running maximum `seq`; never decreases.
    fn watermark(&self) -> u64 {
        self.watermark
    }

    /// Matching events with `seq > filter_point`, oldest first, at most
    /// `spec.max_events`.
    ///
    /// O(1) when `filter_point >= watermark` (no stored event can exceed it) or
    /// when `max_events` is `0`.
    fn collect(&self, spec: &EventWaitSpec, filter_point: u64) -> Vec<RuntimeEvent> {
        let limit = spec.max_events as usize;
        if limit == 0 || filter_point >= self.watermark {
            return Vec::new();
        }
        self.journal
            .iter()
            .filter(|event| event.seq() > filter_point)
            .filter(|event| spec.matches(event))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Number of retained events.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.journal.len()
    }
}

/// The event inbox: the shared journal, its generation counter, and the
/// `wait_for_events` loop. `pub(crate)`: the server drives it through
/// [`crate::NotificationService`].
#[derive(Debug)]
pub(crate) struct EventInbox {
    /// Journal + watermark (its own mutex: never held across `.await`).
    state: Mutex<InboxState>,
    /// Event generation counter; bumped by every `handle_event`.
    events: watch::Sender<u64>,
}

impl EventInbox {
    /// An inbox retaining at most `capacity` events.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(InboxState::new(capacity)),
            // The generation starts at 0; waiters subscribe before they inspect
            // state, so an event that races the seed still wakes them.
            events: watch::channel(0).0,
        }
    }

    /// Locks the inbox mutex, ignoring poisoning (a poisoned lock still holds
    /// usable data).
    fn lock(&self) -> MutexGuard<'_, InboxState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The current watermark (highest `seq` recorded).
    pub(crate) fn watermark(&self) -> u64 {
        self.lock().watermark()
    }

    /// Consumes one broadcast event. Called by the server's event-pump task for
    /// every event, in `seq` order. The signature must stay cheap — `SurfaceCommit`
    /// is high-frequency.
    pub(crate) fn handle_event(&self, event: &RuntimeEvent) {
        {
            let mut state = self.lock();
            state.record(event);
        }
        // `send_modify` cannot fail while any receiver exists, and the generation
        // is the only wakeup source for waiters.
        self.events
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        tracing::trace!(
            seq = event.seq(),
            kind = ?event.kind(),
            "notify: inbox event recorded"
        );
    }

    /// Blocks until at least one matching event is available after the filter
    /// point, or the timeout elapses (`docs/protocol.md` §5.10).
    ///
    /// Never spins: it parks on a generation change or the deadline. The filter
    /// point is captured **once** — re-reading the watermark every iteration would
    /// skip the very event that woke the waiter.
    pub(crate) async fn wait_for_events(&self, spec: EventWaitSpec) -> Result<EventBatch> {
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_millis(spec.timeout_ms);

        // Subscribe *before* reading state: an event that lands between the
        // watermark read and the first park still bumps the generation, so the
        // wait cannot miss a wakeup.
        let mut events = self.events.subscribe();
        let filter_point = spec.since_seq.unwrap_or_else(|| self.watermark());

        loop {
            // Mark the current generation seen, then re-check the journal: an
            // event that arrived before this point is either already in the
            // journal (we collect it now) or bumps the generation after it (and
            // wakes us below).
            events.borrow_and_update();

            let (matching, seq) = {
                let state = self.lock();
                (state.collect(&spec, filter_point), state.watermark())
            };

            if !matching.is_empty() {
                let batch = EventBatch {
                    events: matching,
                    timed_out: false,
                    elapsed_ms: elapsed_ms(started),
                    seq,
                };
                tracing::debug!(
                    seq,
                    count = batch.events.len(),
                    elapsed_ms = batch.elapsed_ms,
                    "notify: wait_for_events resolved"
                );
                return Ok(batch);
            }

            if tokio::time::Instant::now() >= deadline {
                let batch = EventBatch {
                    events: Vec::new(),
                    timed_out: true,
                    elapsed_ms: elapsed_ms(started),
                    seq,
                };
                tracing::debug!(
                    seq,
                    elapsed_ms = batch.elapsed_ms,
                    "notify: wait_for_events timed out"
                );
                return Ok(batch);
            }

            // No polling: wait for the next event generation or the deadline,
            // whichever comes first.
            tokio::select! {
                changed = events.changed() => {
                    if changed.is_err() {
                        // Unreachable while the inbox holds the sender; never spin
                        // on it either.
                        tracing::trace!("notify: inbox generation sender dropped");
                    }
                }
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AppId, LaunchId};

    fn created(seq: u64, window_id: u64) -> RuntimeEvent {
        RuntimeEvent::WindowCreated {
            seq,
            ts_ms: seq,
            window_id: WindowId(window_id),
            app_id: None,
            pid: None,
            launch_id: None,
            title: None,
        }
    }

    fn destroyed(seq: u64, window_id: u64) -> RuntimeEvent {
        RuntimeEvent::WindowDestroyed {
            seq,
            ts_ms: seq,
            window_id: WindowId(window_id),
        }
    }

    fn title_changed(seq: u64, window_id: u64) -> RuntimeEvent {
        RuntimeEvent::TitleChanged {
            seq,
            ts_ms: seq,
            window_id: WindowId(window_id),
            title: Some("t".into()),
        }
    }

    /// A window-less event (`app_launched`), like notifications have no window.
    fn launched(seq: u64) -> RuntimeEvent {
        RuntimeEvent::AppLaunched {
            seq,
            ts_ms: seq,
            launch_id: LaunchId(1),
            app_id: AppId("app".into()),
            pid: None,
        }
    }

    fn seqs(batch: &EventBatch) -> Vec<u64> {
        batch.events.iter().map(RuntimeEvent::seq).collect()
    }

    // ---------------------------------------------------------- journal bounding

    #[test]
    fn inbox_state_evicts_oldest_and_keeps_the_watermark() {
        let mut state = InboxState::new(3);
        for seq in 1..=5 {
            state.record(&created(seq, 1));
        }
        assert_eq!(state.len(), 3, "capacity is a hard bound");
        assert_eq!(
            state.watermark(),
            5,
            "the watermark keeps the max even after eviction"
        );
        let kept: Vec<u64> = state.journal.iter().map(RuntimeEvent::seq).collect();
        assert_eq!(kept, vec![3, 4, 5], "oldest evicted first");

        // A lowering seq never moves the watermark back.
        state.record(&created(2, 1));
        assert_eq!(state.watermark(), 5);
    }

    #[test]
    fn collect_short_circuits_at_or_above_the_watermark() {
        let mut state = InboxState::new(8);
        for seq in [10, 11, 12] {
            state.record(&created(seq, 1));
        }
        let spec = EventWaitSpec::new().since_seq(0);

        assert_eq!(state.collect(&spec, 11).len(), 1);
        assert_eq!(state.collect(&spec, 12), Vec::<RuntimeEvent>::new());
        assert_eq!(state.collect(&spec, u64::MAX), Vec::<RuntimeEvent>::new());
    }

    // -------------------------------------------------------- wait resolution

    #[tokio::test]
    async fn explicit_since_seq_returns_history_immediately() {
        let inbox = EventInbox::new(16);
        for seq in 1..=3 {
            inbox.handle_event(&created(seq, 1));
        }

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(0).timeout_ms(60_000))
            .await
            .expect("infallible");
        assert!(!batch.timed_out);
        assert_eq!(seqs(&batch), vec![1, 2, 3], "oldest first");
        assert_eq!(batch.seq, 3, "watermark at resolution");
    }

    #[tokio::test]
    async fn since_seq_filters_out_already_seen_events() {
        let inbox = EventInbox::new(16);
        for seq in 1..=4 {
            inbox.handle_event(&created(seq, 1));
        }

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(2).timeout_ms(60_000))
            .await
            .expect("infallible");
        assert_eq!(seqs(&batch), vec![3, 4], "strictly greater than since_seq");
    }

    #[tokio::test]
    async fn default_filter_point_is_the_watermark_at_wait_start() {
        let inbox = EventInbox::new(16);
        // Events seen before the wait began must not be delivered.
        inbox.handle_event(&created(1, 1));

        let mut wait = Box::pin(inbox.wait_for_events(EventWaitSpec::new().timeout_ms(60_000)));
        tokio::select! {
            biased;
            batch = &mut wait => panic!("resolved before any new event: {batch:?}"),
            () = tokio::task::yield_now() => {}
        }

        // An event fed during the wait wakes it and is the only one delivered.
        inbox.handle_event(&created(2, 1));
        let batch = wait.await.expect("infallible");
        assert!(!batch.timed_out);
        assert_eq!(seqs(&batch), vec![2]);
        assert_eq!(batch.seq, 2);
    }

    #[tokio::test]
    async fn event_fed_before_the_waiter_starts_is_returned_via_since_seq() {
        let inbox = EventInbox::new(16);
        // Fed before any waiter exists — the no-lost-wakeup seed.
        inbox.handle_event(&created(7, 1));

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(0).timeout_ms(60_000))
            .await
            .expect("infallible");
        assert!(!batch.timed_out);
        assert_eq!(seqs(&batch), vec![7]);
    }

    #[tokio::test(start_paused = true)]
    async fn kind_filter_returns_only_matching_events() {
        let inbox = EventInbox::new(16);
        inbox.handle_event(&created(1, 1));
        inbox.handle_event(&title_changed(2, 1));
        inbox.handle_event(&destroyed(3, 1));

        let batch = inbox
            .wait_for_events(
                EventWaitSpec::new()
                    .kind(EventKind::TitleChanged)
                    .since_seq(0),
            )
            .await
            .expect("infallible");
        assert_eq!(seqs(&batch), vec![2]);
    }

    #[tokio::test]
    async fn empty_kinds_means_every_kind() {
        let inbox = EventInbox::new(16);
        inbox.handle_event(&created(1, 1));
        inbox.handle_event(&launched(2));

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(0))
            .await
            .expect("infallible");
        assert_eq!(seqs(&batch), vec![1, 2], "window-less events included");
    }

    #[tokio::test]
    async fn window_filter_excludes_window_less_events() {
        let inbox = EventInbox::new(16);
        inbox.handle_event(&created(1, 7));
        inbox.handle_event(&launched(2));
        inbox.handle_event(&created(3, 8));

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().window(WindowId(7)).since_seq(0))
            .await
            .expect("infallible");
        assert_eq!(seqs(&batch), vec![1], "only window 7's event");
    }

    #[tokio::test]
    async fn max_events_truncates_oldest_first() {
        let inbox = EventInbox::new(16);
        for seq in 1..=5 {
            inbox.handle_event(&created(seq, 1));
        }

        let batch = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(0).max_events(2))
            .await
            .expect("infallible");
        assert_eq!(seqs(&batch), vec![1, 2], "at most max_events, oldest first");
        assert_eq!(batch.seq, 5, "the watermark still reports the horizon");
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_reports_an_empty_batch_and_the_watermark() {
        let inbox = EventInbox::new(16);
        // A non-matching event advanced the watermark but must not resolve a
        // kind-filtered wait.
        inbox.handle_event(&launched(3));

        let batch = inbox
            .wait_for_events(
                EventWaitSpec::new()
                    .kind(EventKind::WindowCreated)
                    .timeout_ms(1_000),
            )
            .await
            .expect("infallible");
        assert!(batch.timed_out);
        assert!(batch.events.is_empty());
        assert_eq!(batch.seq, 3, "the current watermark");
        assert_eq!(batch.elapsed_ms, 1_000, "measured in the monotonic domain");
    }

    #[tokio::test(start_paused = true)]
    async fn a_matching_event_wakes_a_parked_waiter() {
        let inbox = EventInbox::new(16);

        let mut wait = Box::pin(
            inbox.wait_for_events(
                EventWaitSpec::new()
                    .kind(EventKind::WindowCreated)
                    .timeout_ms(60_000),
            ),
        );
        tokio::select! {
            biased;
            batch = &mut wait => panic!("resolved before any event: {batch:?}"),
            () = tokio::task::yield_now() => {}
        }

        // A non-matching event keeps the waiter parked.
        inbox.handle_event(&launched(1));
        tokio::select! {
            biased;
            batch = &mut wait => panic!("a non-matching event resolved the wait: {batch:?}"),
            () = tokio::task::yield_now() => {}
        }

        inbox.handle_event(&created(2, 1));
        let batch = wait.await.expect("infallible");
        assert!(!batch.timed_out);
        assert_eq!(seqs(&batch), vec![2]);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_timeout_returns_history_but_otherwise_expires_at_once() {
        let inbox = EventInbox::new(16);

        let empty = inbox
            .wait_for_events(EventWaitSpec::new().timeout_ms(0))
            .await
            .expect("infallible");
        assert!(empty.timed_out);
        assert_eq!(empty.elapsed_ms, 0);

        inbox.handle_event(&created(1, 1));
        let with_history = inbox
            .wait_for_events(EventWaitSpec::new().since_seq(0).timeout_ms(0))
            .await
            .expect("infallible");
        assert!(!with_history.timed_out);
        assert_eq!(seqs(&with_history), vec![1]);
    }
}
