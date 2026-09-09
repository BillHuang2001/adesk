//! Connection internals: NDJSON framing, request ids, response demux, fan-out.
//!
//! One [`Connection`] owns a connected Unix socket and two background tasks:
//!
//! - **writer** — drains an outbound queue of pre-encoded NDJSON lines and
//!   flushes them; it is the only task that writes.
//! - **reader** — reads one line at a time under a hard length cap, decodes it
//!   with [`crate::wire`], and dispatches:
//!   - *response* → the pending-request entry for that id (a `oneshot`),
//!   - *event* → the connection's broadcast channel (all streams filter
//!     locally),
//!   - *malformed frame* → fail every pending request with
//!     [`ClientError::Protocol`] and terminate the connection,
//!   - *EOF* → fail every pending request with [`ClientError::Closed`].
//!
//! Concurrency model (binding design):
//!
//! - Request ids are a per-connection `AtomicU64` starting at 1, never reused.
//! - `pending` is a `std::sync::Mutex<HashMap<u64, oneshot::Sender<..>>>`; the
//!   lock is never held across an `await`.
//! - The outbound queue is bounded; a full queue means the connection is
//!   unhealthy and the request fails fast with [`ClientError::Closed`] rather
//!   than blocking the caller.
//! - The event channel is a `tokio::sync::broadcast` with a fixed capacity; a
//!   lagging subscriber observes [`ClientError::Lagged`] instead of stalling
//!   the reader.
//! - Neither task holds a strong reference to [`Connection`], so dropping every
//!   [`Client`](crate::Client) handle closes the socket (writer sees the queue
//!   closed, reader sees EOF).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::wire::ServerError;
// Named by the intra-doc links below and by the request/reader bodies the
// Manager phase adds; unused until then (skeleton phase).
#[allow(unused_imports)]
use crate::ClientError;
use crate::{AgpEvent, ConnectOptions, Result};

/// Capacity of the per-connection event broadcast channel.
///
/// Large enough to absorb bursts of `surface_commit` events between two polls
/// of a stream; lagging beyond this is reported, never silently ignored.
#[allow(dead_code)] // skeleton phase: used by the reader task once it lands
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Capacity of the outbound request queue.
#[allow(dead_code)] // skeleton phase: used by `Connection::connect` once it lands
pub(crate) const OUTBOUND_QUEUE_CAPACITY: usize = 64;

/// One in-flight request: where to deliver its result.
type PendingSender = oneshot::Sender<Result<Value, ServerError>>;

/// Shared state between the public `Client` handle and the background tasks.
///
/// The field list is the design of the transport; the Manager phase implements
/// the bodies. `#[allow(dead_code)]` keeps the skeleton warning-free.
#[allow(dead_code)]
pub(crate) struct Connection {
    /// Socket path (for diagnostics and `Client::socket_path`).
    path: PathBuf,
    /// Pre-encoded NDJSON lines waiting for the writer task.
    outbound: mpsc::Sender<Vec<u8>>,
    /// In-flight requests keyed by request id.
    pending: Arc<Mutex<HashMap<u64, PendingSender>>>,
    /// Next request id (monotonic per connection, starts at 1).
    next_id: AtomicU64,
    /// Fan-out channel for inbound events.
    events: broadcast::Sender<AgpEvent>,
    /// Set once the connection is unusable (EOF, protocol error, explicit close).
    closed: Arc<AtomicBool>,
    /// Max NDJSON line length accepted from the server (bytes).
    max_frame_len: usize,
    /// Join handles for the reader and writer tasks (used by `close`).
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Connection {
    /// Connect to `options.path`, spawn the reader/writer tasks and return the
    /// shared connection.
    ///
    /// Fails with [`ClientError::Io`] if the socket cannot be reached within
    /// `options.connect_timeout`.
    pub(crate) async fn connect(options: ConnectOptions) -> Result<Arc<Connection>> {
        let _ = options;
        todo!("bind tokio::net::UnixStream, spawn reader + writer tasks")
    }

    /// Send one request and await its typed result.
    ///
    /// `P` is the params object (serialised with `serde_json`); `R` is the
    /// `result` object of the response. Server error frames become
    /// [`ClientError::Server`]; a missing/closed connection becomes
    /// [`ClientError::Closed`]; a response whose JSON does not fit `R` becomes
    /// [`ClientError::InvalidPayload`].
    pub(crate) async fn request<P, R>(&self, method: &str, params: &P) -> Result<R>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let _ = (method, params);
        todo!("allocate id, register oneshot, encode, enqueue, await, decode")
    }

    /// Enqueue a request without awaiting its response.
    ///
    /// Used by the stream `Drop` impls to send `unsubscribe_events`; failures
    /// are ignored (the connection is going away anyway).
    #[allow(dead_code)] // skeleton phase: called by `unsubscribe_fire_and_forget`
    pub(crate) fn fire_and_forget<P>(&self, method: &str, params: &P)
    where
        P: Serialize + ?Sized,
    {
        let _ = (method, params);
        todo!("encode and try_send on the outbound queue")
    }

    /// Cancel a subscription from a `Drop` impl (best-effort).
    pub(crate) fn unsubscribe_fire_and_forget(&self, subscription_id: u64) {
        let _ = subscription_id;
        todo!("fire_and_forget(\"unsubscribe_events\", {subscription_id})")
    }

    /// Subscribe to this connection's event fan-out.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AgpEvent> {
        self.events.subscribe()
    }

    /// Whether the connection has been closed or broken.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The socket path this connection was opened on.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Gracefully close: stop accepting requests, fail in-flight ones with
    /// [`ClientError::Closed`], and shut the socket down.
    pub(crate) async fn close(&self) -> Result<()> {
        todo!("mark closed, drop outbound sender, await tasks")
    }
}

/// Read one NDJSON line with a hard length cap.
///
/// Returns `Ok(None)` on clean EOF. A line exceeding `max_frame_len` (or EOF in
/// the middle of a line) is a [`ClientError::Protocol`] / [`ClientError::Io`]
/// and terminates the connection.
#[allow(dead_code)]
pub(crate) async fn read_line<R>(reader: &mut R, max_frame_len: usize) -> Result<Option<Vec<u8>>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let _ = (reader, max_frame_len);
    todo!("bounded read_until(b'\\n') without unbounded buffering")
}
