//! Per-connection session state and the ordered input queue.

use std::future::Future;
use std::sync::{Arc, Mutex};

use crate::subscriptions::SubscriptionId;

/// Monotonic per-server connection id (starts at 1).
pub type SessionId = u64;

/// Per-connection state: subscription bookkeeping and input ordering.
///
/// Cheap to clone; all clones share one session.
#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    id: SessionId,
    subscriptions: Mutex<Vec<SubscriptionId>>,
    input: InputQueue,
}

impl Session {
    /// A new session for connection `id`.
    pub fn new(id: SessionId) -> Session {
        Session {
            inner: Arc::new(SessionInner {
                id,
                subscriptions: Mutex::new(Vec::new()),
                input: InputQueue::new(),
            }),
        }
    }

    /// The connection id.
    pub fn id(&self) -> SessionId {
        self.inner.id
    }

    /// Records an event or inspector subscription owned by this session.
    pub fn add_subscription(&self, id: SubscriptionId) {
        self.inner
            .subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(id);
    }

    /// Forgets one subscription; returns whether it was owned here.
    pub fn remove_subscription(&self, id: SubscriptionId) -> bool {
        let mut subscriptions = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let before = subscriptions.len();
        subscriptions.retain(|subscription| *subscription != id);
        subscriptions.len() != before
    }

    /// All subscriptions still owned by this session (test-only accessor).
    #[cfg(test)]
    pub fn subscriptions(&self) -> Vec<SubscriptionId> {
        self.inner
            .subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The ordered input queue.
    pub fn input(&self) -> &InputQueue {
        &self.inner.input
    }
}

/// FIFO gate that serializes input methods per connection.
///
/// Input methods (`docs/protocol.md` §5.5) expand into several compositor
/// commands; running them through this gate guarantees they reach the
/// compositor in submission order even when the connection dispatches
/// concurrently. `tokio::sync::Mutex` is fair, so waiters are served FIFO.
pub struct InputQueue {
    gate: tokio::sync::Mutex<()>,
}

impl InputQueue {
    /// An empty queue.
    pub fn new() -> InputQueue {
        InputQueue {
            gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Runs `future` while holding the queue, preserving submission order.
    pub async fn run<F: Future>(&self, future: F) -> F::Output {
        let _guard = self.gate.lock().await;
        future.await
    }
}

impl Default for InputQueue {
    fn default() -> InputQueue {
        InputQueue::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- Session bookkeeping -------------------------------------------------

    #[test]
    fn session_reports_its_id() {
        assert_eq!(Session::new(1).id(), 1);
        assert_eq!(Session::new(42).id(), 42);
    }

    #[test]
    fn session_tracks_subscriptions_in_insertion_order() {
        let session = Session::new(7);
        assert!(session.subscriptions().is_empty());

        session.add_subscription(10);
        session.add_subscription(11);
        session.add_subscription(12);
        assert_eq!(session.subscriptions(), vec![10, 11, 12]);
    }

    #[test]
    fn session_remove_subscription_only_removes_owned_ids() {
        let session = Session::new(7);
        session.add_subscription(10);
        session.add_subscription(11);
        session.add_subscription(12);

        // An id this session never registered is not removed (and reported false).
        assert!(!session.remove_subscription(99));
        assert_eq!(session.subscriptions(), vec![10, 11, 12]);

        // Exactly the requested id disappears, the others stay.
        assert!(session.remove_subscription(11));
        assert_eq!(session.subscriptions(), vec![10, 12]);

        // Removing the same id twice: the second call has nothing left to drop.
        assert!(!session.remove_subscription(11));
        assert_eq!(session.subscriptions(), vec![10, 12]);
    }

    #[test]
    fn session_remove_subscription_drops_duplicate_ids_together() {
        let session = Session::new(7);
        session.add_subscription(5);
        session.add_subscription(5);
        session.add_subscription(6);
        assert_eq!(session.subscriptions(), vec![5, 5, 6]);

        // `remove_subscription` is set semantics: every copy of the id goes away
        // and the call reports that this session owned it.
        assert!(session.remove_subscription(5));
        assert_eq!(session.subscriptions(), vec![6]);
        assert!(!session.remove_subscription(5));
        assert_eq!(session.subscriptions(), vec![6]);
    }

    #[test]
    fn session_clones_share_one_session() {
        let session = Session::new(3);
        let clone = session.clone();
        assert_eq!(clone.id(), session.id());

        // Bookkeeping written through one clone is visible through the other.
        clone.add_subscription(20);
        assert_eq!(session.subscriptions(), vec![20]);
        assert!(session.remove_subscription(20));
        assert!(clone.subscriptions().is_empty());

        // `input()` hands out the same queue, not a per-clone one.
        assert!(std::ptr::eq(session.input(), clone.input()));
    }

    // --- InputQueue ----------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn input_queue_run_returns_the_future_output() {
        let queue = InputQueue::new();
        assert_eq!(queue.run(async { 6 * 7 }).await, 42);
        assert_eq!(queue.run(async { String::from("ok") }).await, "ok");

        // A guarded future may itself await; its output still comes back.
        let awaited = queue
            .run(async {
                tokio::task::yield_now().await;
                (1, 2)
            })
            .await;
        assert_eq!(awaited, (1, 2));

        // `Default` is the same empty queue as `new`.
        assert_eq!(InputQueue::default().run(async { 1 + 1 }).await, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn input_queue_serializes_sequential_calls_in_order() {
        let queue = InputQueue::new();
        let log = Mutex::new(Vec::new());

        for step in 0..5u32 {
            let returned = queue
                .run(async {
                    log.lock().expect("test lock").push((step, "enter"));
                    tokio::task::yield_now().await;
                    log.lock().expect("test lock").push((step, "leave"));
                    step
                })
                .await;
            assert_eq!(returned, step);
        }

        let expected: Vec<(u32, &str)> = (0..5u32)
            .flat_map(|step| [(step, "enter"), (step, "leave")])
            .collect();
        assert_eq!(*log.lock().expect("test lock"), expected);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn input_queue_serves_concurrent_submissions_in_submission_order() {
        const TASKS: u32 = 8;

        let session = Arc::new(Session::new(1));
        let entered = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(Mutex::new(Vec::new()));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        // Handshake: the test spawns the next task only after the previous one has
        // actually submitted to the gate, so submission order is deterministic
        // (no sleeps, no scheduler assumptions beyond "a task runs when polled").
        let (submitted_tx, mut submitted_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();

        let mut handles = Vec::new();
        for step in 0..TASKS {
            let session = Arc::clone(&session);
            let entered = Arc::clone(&entered);
            let completed = Arc::clone(&completed);
            let in_flight = Arc::clone(&in_flight);
            let max_in_flight = Arc::clone(&max_in_flight);
            let submitted_tx = submitted_tx.clone();

            handles.push(tokio::task::spawn(async move {
                submitted_tx.send(step).expect("test receiver is alive");
                session
                    .input()
                    .run(async move {
                        let running = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_in_flight.fetch_max(running, Ordering::SeqCst);
                        // If the gate let submissions interleave, a second waiter
                        // would run here while this one is still in flight.
                        tokio::task::yield_now().await;
                        entered.lock().expect("test lock").push(step);
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
                completed.lock().expect("test lock").push(step);
            }));

            // Let the spawned task reach the gate before spawning the next one.
            tokio::task::yield_now().await;
            assert_eq!(submitted_rx.recv().await, Some(step));
        }

        for handle in handles {
            handle.await.expect("input task must not panic");
        }

        let expected: Vec<u32> = (0..TASKS).collect();
        assert_eq!(*entered.lock().expect("test lock"), expected);
        assert_eq!(*completed.lock().expect("test lock"), expected);
        assert_eq!(
            max_in_flight.load(Ordering::SeqCst),
            1,
            "concurrent submissions must never run at the same time"
        );
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
    }
}
