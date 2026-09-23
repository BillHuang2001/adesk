//! Reply FIFOs for the client's reply-only requests.
//!
//! Several VAP requests are answered by a dedicated reply message rather than by
//! a broadcast stream: `request_state` → `state`, the three recording controls →
//! `recording`, `list_apps` → `apps`, `launch_app` → `launch_result`. None of
//! those replies carries a correlation id the client relies on, so the client
//! resolves them **oldest-first** through one [`ReplyFifo`] per request kind —
//! which also guarantees a reply can never be delivered to a different kind's
//! pending call.
//!
//! Every registration carries a unique token so the awaiting side can remove
//! *its own* entry when a request is answered by a VAP `error` (which travels the
//! shared error broadcast, not the FIFO) or when the request could not be written
//! at all. Without that, the stale entry would stay at the head of the FIFO and
//! swallow the next reply, so a later request on the same connection would never
//! resolve.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use adesk_core::ErrorCode;
use tokio::sync::{broadcast, oneshot};

use crate::error::{Result, ViewerError};

/// One registration in a reply FIFO: the sender the dispatcher resolves on the
/// matching reply, plus a token that identifies the entry.
struct Waiter<T> {
    /// Unique registration token, matched by the awaiting side.
    token: u64,
    /// The sender the dispatcher resolves when the matching reply arrives.
    sender: oneshot::Sender<T>,
}

/// A registered, not-yet-answered request: its FIFO token and its reply receiver.
pub(crate) struct Pending<T> {
    /// Identifies this registration inside its FIFO, for
    /// [`ReplyFifo::remove`].
    pub(crate) token: u64,
    /// Resolves with the reply, or fails if the connection ends first.
    receiver: oneshot::Receiver<T>,
}

/// A FIFO of pending reply waiters for one reply-only request kind.
///
/// The dispatcher [`resolve`](ReplyFifo::resolve)s the oldest entry when the
/// matching reply arrives and drops every entry on
/// [`shutdown`](ReplyFifo::clear); the awaiting side
/// [`remove`](ReplyFifo::remove)s its own entry when its request is answered by a
/// VAP `error`, a write failure or a closed connection.
pub(crate) struct ReplyFifo<T> {
    /// Pending registrations, oldest first.
    waiters: Mutex<VecDeque<Waiter<T>>>,
    /// Source of the unique registration tokens.
    next_token: AtomicU64,
}

impl<T> ReplyFifo<T> {
    /// Creates an empty FIFO.
    pub(crate) fn new() -> ReplyFifo<T> {
        ReplyFifo {
            waiters: Mutex::new(VecDeque::new()),
            next_token: AtomicU64::new(1),
        }
    }

    /// Registers a waiter at the back of the FIFO.
    pub(crate) fn register(&self) -> Pending<T> {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        lock(&self.waiters).push_back(Waiter { token, sender });
        Pending { token, receiver }
    }

    /// Removes a still-pending waiter by its token.
    ///
    /// A no-op once the dispatcher has already resolved (and popped) the entry,
    /// or once the connection cleared the FIFO.
    pub(crate) fn remove(&self, token: u64) {
        lock(&self.waiters).retain(|waiter| waiter.token != token);
    }

    /// Resolves the oldest pending waiter with `value`.
    ///
    /// Returns whether a waiter was pending; an entry whose awaiting side has
    /// already gone away is still consumed. A dropped receiver is not an error:
    /// the caller only logs that nothing was waiting.
    pub(crate) fn resolve(&self, value: T) -> bool {
        match lock(&self.waiters).pop_front() {
            Some(waiter) => {
                let _ = waiter.sender.send(value);
                true
            }
            None => false,
        }
    }

    /// Drops every pending waiter, so no request can park forever after the peer
    /// is gone.
    pub(crate) fn clear(&self) {
        lock(&self.waiters).clear();
    }

    /// Awaits the reply registered as `pending`, or the first server `error` /
    /// connection close.
    ///
    /// A VAP `error` answers its request through the shared error stream rather
    /// than the FIFO, so this consumes the registration before returning it —
    /// keeping the FIFO aligned for the next request. A success resolves through
    /// the dispatcher, which has already popped the entry.
    ///
    /// # Errors
    ///
    /// [`ViewerError::Backend`] for a server `error`,
    /// [`ViewerError::Closed`] once the connection ends.
    pub(crate) async fn await_reply(
        &self,
        pending: Pending<T>,
        errors: &mut broadcast::Receiver<(ErrorCode, String)>,
    ) -> Result<T> {
        let Pending {
            token,
            mut receiver,
        } = pending;
        loop {
            tokio::select! {
                reply = &mut receiver => return reply.map_err(|_| ViewerError::Closed),
                error = errors.recv() => match error {
                    Ok((code, message)) => {
                        self.remove(token);
                        return Err(ViewerError::Backend { code, message });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        self.remove(token);
                        return Err(ViewerError::Closed);
                    }
                },
            }
        }
    }
}

/// Locks a standard mutex, recovering the guard on poisoning.
///
/// The client never panics while holding these locks, so poisoning is
/// unreachable in practice; recovering keeps every path panic-free.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_reply_resolves_the_oldest_waiter() {
        let fifo: ReplyFifo<u32> = ReplyFifo::new();
        let first = fifo.register();
        let second = fifo.register();
        // The error stream stays open: only a real `error`/close may fail a reply.
        let (_sender, mut errors) = broadcast::channel(1);

        assert!(fifo.resolve(1));
        assert!(fifo.resolve(2));
        assert!(!fifo.resolve(3), "a third reply has no waiter to resolve");
        assert_eq!(fifo.await_reply(first, &mut errors).await.unwrap(), 1);
        assert_eq!(fifo.await_reply(second, &mut errors).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_removed_waiter_does_not_swallow_the_next_reply() {
        let fifo: ReplyFifo<u32> = ReplyFifo::new();
        let abandoned = fifo.register();
        fifo.remove(abandoned.token);
        // Removing an unknown token is a no-op, not a panic.
        fifo.remove(abandoned.token);

        let next = fifo.register();
        assert!(fifo.resolve(9), "the live waiter still gets the reply");
        let (_sender, mut errors) = broadcast::channel(1);
        assert_eq!(fifo.await_reply(next, &mut errors).await.unwrap(), 9);
    }

    #[tokio::test]
    async fn a_server_error_fails_the_waiter_and_alignment_holds() {
        let fifo: ReplyFifo<u32> = ReplyFifo::new();
        let (sender, mut errors) = broadcast::channel(4);
        let rejected = fifo.register();

        sender
            .send((ErrorCode::NotSupported, "nope".to_owned()))
            .expect("a live receiver");

        match fifo.await_reply(rejected, &mut errors).await {
            Err(ViewerError::Backend { code, message }) => {
                assert_eq!(code, ErrorCode::NotSupported);
                assert_eq!(message, "nope");
            }
            other => panic!("expected a backend error, got {other:?}"),
        }

        // The rejected request left no entry behind, so the next reply resolves
        // the next request instead of the stale slot.
        let next = fifo.register();
        assert!(fifo.resolve(5));
        assert_eq!(fifo.await_reply(next, &mut errors).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn a_cleared_fifo_fails_every_waiter_with_closed() {
        let fifo: ReplyFifo<u32> = ReplyFifo::new();
        let (_sender, mut errors) = broadcast::channel(1);
        let pending = fifo.register();
        fifo.clear();
        assert!(matches!(
            fifo.await_reply(pending, &mut errors).await,
            Err(ViewerError::Closed)
        ));
    }

    #[tokio::test]
    async fn a_closed_error_stream_fails_the_waiter_with_closed() {
        let fifo: ReplyFifo<u32> = ReplyFifo::new();
        let (sender, mut errors) = broadcast::channel(1);
        let pending = fifo.register();
        drop(sender);
        assert!(matches!(
            fifo.await_reply(pending, &mut errors).await,
            Err(ViewerError::Closed)
        ));
    }
}
