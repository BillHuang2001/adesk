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

    /// All subscriptions still owned by this session.
    pub fn subscriptions(&self) -> Vec<SubscriptionId> {
        self.inner
            .subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Drains the subscription set (called on disconnect).
    pub fn take_subscriptions(&self) -> Vec<SubscriptionId> {
        std::mem::take(
            &mut *self
                .inner
                .subscriptions
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
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
        InputQueue { gate: tokio::sync::Mutex::new(()) }
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
