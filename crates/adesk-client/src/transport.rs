//! Connection internals: NDJSON framing, request ids, response demux, fan-out.
//!
//! One [`Connection`] owns a connected Unix socket and two background tasks:
//!
//! - **writer** — drains an outbound queue of pre-encoded NDJSON lines and
//!   flushes them; it is the only task that writes.
//! - **reader** — reads one line at a time under a hard length cap, decodes it
//!   with [`crate::wire`], and dispatches:
//!   - *response* → the pending-request entry for that id (a `oneshot`),
//!   - *event* → the connection's event fan-out (all streams filter locally),
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
//! - The event fan-out gives every subscription its own bounded queue; a
//!   lagging subscriber observes [`ClientError::Lagged`] instead of stalling
//!   the reader.
//! - Neither task holds a strong reference to [`Connection`], so dropping every
//!   [`Client`](crate::Client) handle closes the socket (the writer's queue
//!   closes, the reader's shutdown signal drops).
//!
//! ## Why not a `broadcast` channel?
//!
//! `tokio::sync::broadcast::Receiver` cannot be polled from a
//! `futures::Stream::poll_next` (it exposes no `poll_recv`), so the fan-out is
//! built from per-subscriber bounded `mpsc` queues plus an explicit skipped
//! counter ([`EventFanout`]). The observable contract is the same as a
//! broadcast channel: bounded buffering, [`ClientError::Lagged`] on overflow,
//! [`ClientError::Closed`] when the connection ends.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use crate::events::agp_event_from_raw;
use crate::wire::{self, Inbound, ServerError};
use crate::{AgpEvent, ClientError, ConnectOptions, Result};

/// Capacity of one subscriber's event queue.
///
/// Large enough to absorb bursts of `surface_commit` events between two polls
/// of a stream; lagging beyond this is reported, never silently ignored.
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Capacity of the outbound request queue.
pub(crate) const OUTBOUND_QUEUE_CAPACITY: usize = 64;

/// One in-flight request: where to deliver its result.
type PendingSender = oneshot::Sender<Result<Value, ServerError>>;

/// Why the connection ended.
///
/// [`PendingSender`] can only carry the server's answer, so every other failure
/// is stored once in the connection (see [`Shared::close_reason`]) and
/// handed to each caller that is left hanging. Unlike [`ClientError::Io`] this
/// type is `Clone`, so one stored reason can explain any number of cancelled
/// requests.
#[derive(Debug, Clone)]
enum CloseReason {
    /// The peer violated the wire contract.
    Protocol {
        /// Human-readable description of the violation.
        message: String,
    },
    /// The connection ended (EOF or explicit `close`).
    Closed,
    /// A read or write failed.
    Io {
        /// Kind of the underlying I/O error.
        kind: io::ErrorKind,
        /// Message of the underlying I/O error.
        message: String,
    },
}

impl CloseReason {
    /// Materialise a fresh [`ClientError`] for one caller.
    fn to_error(&self) -> ClientError {
        match self {
            CloseReason::Protocol { message } => ClientError::Protocol {
                message: message.clone(),
            },
            CloseReason::Closed => ClientError::Closed,
            CloseReason::Io { kind, message } => {
                ClientError::Io(io::Error::new(*kind, message.clone()))
            }
        }
    }

    /// Capture an I/O error (`std::io::Error` is not `Clone`).
    fn io(error: &io::Error) -> CloseReason {
        CloseReason::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl From<ClientError> for CloseReason {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Protocol { message } => CloseReason::Protocol { message },
            ClientError::Closed => CloseReason::Closed,
            ClientError::Io(error) => CloseReason::io(&error),
            other => CloseReason::Protocol {
                message: other.to_string(),
            },
        }
    }
}

/// One subscriber of the connection's event fan-out.
struct Subscriber {
    /// Bounded queue towards one stream.
    sender: mpsc::Sender<AgpEvent>,
    /// Events dropped because `sender` was full since the last poll.
    skipped: Arc<AtomicU64>,
}

/// Per-connection event fan-out: one bounded queue per subscription.
#[derive(Default)]
struct EventFanout {
    /// Live subscriptions; senders are dropped when a stream goes away.
    subscribers: Vec<Subscriber>,
    /// Set once the connection ends; new subscribers are born closed.
    closed: bool,
}

impl EventFanout {
    /// Register a new subscription.
    fn subscribe(&mut self) -> EventReceiver {
        let skipped = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        if !self.closed {
            self.subscribers.push(Subscriber {
                sender,
                skipped: skipped.clone(),
            });
        }
        // When already closed the sender is dropped here, so `receiver` is
        // immediately at its end (the stream reports `Closed`).
        EventReceiver {
            receiver,
            skipped,
            closed_reported: false,
        }
    }

    /// Deliver one event to every live subscriber.
    ///
    /// A full queue drops the event and counts it; a dropped receiver retires
    /// the subscriber.
    fn send(&mut self, event: AgpEvent) {
        if self.closed {
            return;
        }
        self.subscribers.retain_mut(
            |subscriber| match subscriber.sender.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    subscriber.skipped.fetch_add(1, Ordering::SeqCst);
                    true
                }
                Err(TrySendError::Closed(_)) => false,
            },
        );
    }

    /// End every subscription (dropping the senders wakes the streams).
    fn close(&mut self) {
        self.closed = true;
        self.subscribers.clear();
    }
}

/// Receiving end of one event subscription (crate-internal; the public surface
/// is the `futures::Stream` implemented by the stream types in `events.rs`).
pub(crate) struct EventReceiver {
    /// Bounded queue fed by the reader task.
    receiver: mpsc::Receiver<AgpEvent>,
    /// Events dropped since the last poll (see [`EventFanout::send`]).
    skipped: Arc<AtomicU64>,
    /// Whether the single [`ClientError::Closed`] item was already produced.
    closed_reported: bool,
}

impl EventReceiver {
    /// Poll the next event.
    ///
    /// Overflow is reported once as [`ClientError::Lagged`] (with the number of
    /// dropped events), the end of the connection once as
    /// [`ClientError::Closed`], and only then does the stream end with `None`.
    pub(crate) fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<AgpEvent>>> {
        let skipped = self.skipped.swap(0, Ordering::SeqCst);
        if skipped > 0 {
            return Poll::Ready(Some(Err(ClientError::Lagged { skipped })));
        }
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event))),
            Poll::Ready(None) => {
                if self.closed_reported {
                    Poll::Ready(None)
                } else {
                    self.closed_reported = true;
                    Poll::Ready(Some(Err(ClientError::Closed)))
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// State shared between [`Connection`], the reader task and the writer task.
#[derive(Clone)]
struct Shared {
    /// In-flight requests keyed by request id.
    pending: Arc<Mutex<HashMap<u64, PendingSender>>>,
    /// Fan-out to every event subscription.
    events: Arc<Mutex<EventFanout>>,
    /// Set once the connection is unusable (EOF, protocol error, explicit close).
    closed: Arc<AtomicBool>,
    /// Why the connection ended, set by whichever task (or `close`) noticed
    /// first; every cancelled request reports this.
    close_reason: Arc<Mutex<Option<CloseReason>>>,
}

impl Shared {
    /// Whether the connection has been closed or broken.
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Record why the connection ended (first reason wins).
    fn set_reason(&self, reason: CloseReason) {
        let mut stored = self.close_reason.lock().unwrap();
        if stored.is_none() {
            *stored = Some(reason);
        }
    }

    /// The error every cancelled request reports.
    fn close_error(&self) -> ClientError {
        match self.close_reason.lock().unwrap().as_ref() {
            Some(reason) => reason.to_error(),
            None => ClientError::Closed,
        }
    }

    /// Mark the connection unusable and fail everything still in flight.
    fn close_with(&self, reason: CloseReason) {
        self.set_reason(reason);
        // Mark closed before draining so a racing `request` fails fast.
        self.closed.store(true, Ordering::SeqCst);
        fail_inflight(self);
    }
}

/// Shared state between the public `Client` handle and the background tasks.
pub(crate) struct Connection {
    /// Socket path (for diagnostics and `Client::socket_path`).
    path: PathBuf,
    /// Pre-encoded NDJSON lines waiting for the writer task.
    ///
    /// `None` once the connection is closed; taking it stops the writer.
    outbound: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// Next request id (monotonic per connection, starts at 1).
    next_id: AtomicU64,
    /// Demux/fan-out state, shared with the background tasks.
    shared: Shared,
    /// Join handles for the reader and writer tasks (used by `close`).
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Signals the reader task to stop (dropped when the connection is).
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl Connection {
    /// Connect to `options.path`, spawn the reader/writer tasks and return the
    /// shared connection.
    ///
    /// Fails with [`ClientError::Io`] if the socket cannot be reached within
    /// `options.connect_timeout`.
    pub(crate) async fn connect(options: ConnectOptions) -> Result<Arc<Connection>> {
        let path = options.path.clone();
        let connect = UnixStream::connect(&path);
        let stream = match options.connect_timeout {
            Some(timeout) => tokio::time::timeout(timeout, connect).await.map_err(|_| {
                ClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out connecting to {}", path.display()),
                ))
            })??,
            None => connect.await?,
        };

        let (read_half, write_half) = stream.into_split();
        let (outbound, inbound) = mpsc::channel::<Vec<u8>>(OUTBOUND_QUEUE_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let pending: Arc<Mutex<HashMap<u64, PendingSender>>> = Arc::new(Mutex::new(HashMap::new()));
        let events = Arc::new(Mutex::new(EventFanout::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let close_reason: Arc<Mutex<Option<CloseReason>>> = Arc::new(Mutex::new(None));
        let shared = Shared {
            pending,
            events,
            closed,
            close_reason,
        };

        let reader = tokio::spawn(reader_task(
            BufReader::new(read_half),
            shutdown_rx,
            shared.clone(),
            options.max_frame_len,
        ));
        let writer = tokio::spawn(writer_task(write_half, inbound, shared.clone()));

        tracing::debug!(path = %path.display(), "connected to the ADesk runtime");
        Ok(Arc::new(Connection {
            path,
            outbound: Mutex::new(Some(outbound)),
            next_id: AtomicU64::new(1),
            shared,
            tasks: Mutex::new(vec![reader, writer]),
            shutdown: Mutex::new(Some(shutdown_tx)),
        }))
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
        if self.is_closed() {
            return Err(self.close_error());
        }
        let params = serde_json::to_value(params).map_err(|error| ClientError::Protocol {
            message: format!("failed to encode the params of `{method}`: {error}"),
        })?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = wire::encode_request(id, method, params)?;

        // Register before enqueueing: the reader may answer as soon as the line
        // is on the wire, and an unregistered response would be dropped.
        let (sender, receiver) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(id, sender);

        let enqueued = match self.outbound.lock().unwrap().as_ref() {
            Some(outbound) => outbound.try_send(line),
            // `close` took the sender: the writer is gone.
            None => Err(TrySendError::Closed(Vec::new())),
        };
        match enqueued {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.pending.lock().unwrap().remove(&id);
                tracing::warn!(id, method, "outbound queue is full; failing the request");
                return Err(ClientError::Closed);
            }
            Err(TrySendError::Closed(_)) => {
                self.shared.pending.lock().unwrap().remove(&id);
                return Err(self.close_error());
            }
        }

        if self.is_closed() && self.shared.pending.lock().unwrap().remove(&id).is_some() {
            // The connection ended after the request was registered but nobody
            // answered it yet: report why instead of waiting forever.
            return Err(self.close_error());
        }

        match receiver.await {
            Ok(Ok(value)) => {
                serde_json::from_value(value).map_err(|error| ClientError::InvalidPayload {
                    message: format!(
                        "result of `{method}` does not match the expected type: {error}"
                    ),
                })
            }
            Ok(Err(error)) => Err(ClientError::Server {
                code: error.code,
                message: error.message,
            }),
            // The sender was dropped: the connection ended or `close` failed it.
            Err(_cancelled) => Err(self.close_error()),
        }
    }

    /// Enqueue a request without awaiting its response.
    ///
    /// Used by the stream `Drop` impls to send `unsubscribe_events`; failures
    /// are ignored (the connection is going away anyway).
    pub(crate) fn fire_and_forget<P>(&self, method: &str, params: &P)
    where
        P: Serialize + ?Sized,
    {
        if self.is_closed() {
            return;
        }
        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(error) => {
                tracing::debug!(method, %error, "dropping fire-and-forget request");
                return;
            }
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = match wire::encode_request(id, method, params) {
            Ok(line) => line,
            Err(error) => {
                tracing::debug!(method, %error, "dropping fire-and-forget request");
                return;
            }
        };
        // A full queue is ignored: nobody is waiting for this request.
        if let Some(outbound) = self.outbound.lock().unwrap().as_ref() {
            let _ = outbound.try_send(line);
        }
    }

    /// Cancel a subscription from a `Drop` impl (best-effort).
    pub(crate) fn unsubscribe_fire_and_forget(&self, subscription_id: u64) {
        #[derive(Serialize)]
        struct UnsubscribeParams {
            subscription_id: u64,
        }
        self.fire_and_forget("unsubscribe_events", &UnsubscribeParams { subscription_id });
    }

    /// Subscribe to this connection's event fan-out.
    pub(crate) fn subscribe(&self) -> EventReceiver {
        self.shared.events.lock().unwrap().subscribe()
    }

    /// Whether the connection has been closed or broken.
    pub(crate) fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    /// The socket path this connection was opened on.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The error every cancelled request reports.
    fn close_error(&self) -> ClientError {
        self.shared.close_error()
    }

    /// Gracefully close: stop accepting requests, fail in-flight ones with
    /// [`ClientError::Closed`], and shut the socket down.
    pub(crate) async fn close(&self) -> Result<()> {
        // Taking the outbound sender stops the writer and makes a concurrent
        // `request` fail fast instead of waiting for an answer that can no
        // longer arrive (it is taken *before* the pending map is drained).
        drop(self.outbound.lock().unwrap().take());
        self.shared.close_with(CloseReason::Closed);
        // Wake the reader, which is parked on a socket read.
        drop(self.shutdown.lock().unwrap().take());

        // Drain the task handles before awaiting: a std lock must not be held
        // across an await.
        let tasks: Vec<tokio::task::JoinHandle<()>> = {
            let mut tasks = self.tasks.lock().unwrap();
            std::mem::take(&mut *tasks)
        };
        for task in tasks {
            let _ = task.await;
        }
        Ok(())
    }
}

/// Fail every in-flight request and end every event subscription.
///
/// The close reason and the `closed` flag must already be set: a caller that
/// registers a request after this returns observes `closed` and fails fast.
fn fail_inflight(shared: &Shared) {
    let senders: Vec<PendingSender> = {
        let mut map = shared.pending.lock().unwrap();
        map.drain().map(|(_, sender)| sender).collect()
    };
    drop(senders);
    shared.events.lock().unwrap().close();
}

/// Reader task: decode inbound frames and dispatch them.
async fn reader_task<R>(
    mut reader: R,
    mut shutdown: oneshot::Receiver<()>,
    shared: Shared,
    max_frame_len: usize,
) where
    R: AsyncBufRead + Unpin,
{
    let reason = loop {
        // `read_line` is not cancel-safe, but the only cancellation is shutdown.
        tokio::select! {
            biased;
            _ = &mut shutdown => break CloseReason::Closed,
            line = read_line(&mut reader, max_frame_len) => match line {
                Ok(Some(line)) => match wire::decode_line(&line) {
                    Ok(Inbound::Response { id, result }) => {
                        match shared.pending.lock().unwrap().remove(&id) {
                            Some(sender) => {
                                // The caller may have gone away; that is fine.
                                let _ = sender.send(result);
                            }
                            None => {
                                tracing::debug!(id, "ignoring a response for an unknown request id");
                            }
                        }
                    }
                    Ok(Inbound::Event(raw)) => {
                        let event = agp_event_from_raw(raw);
                        shared.events.lock().unwrap().send(event);
                    }
                    Err(error) => break CloseReason::from(error),
                },
                Ok(None) => break CloseReason::Closed,
                Err(error) => break CloseReason::from(error),
            },
        }
    };

    tracing::debug!(?reason, "AGP reader task finished");
    shared.close_with(reason);
}

/// Writer task: the only task that writes to the socket.
async fn writer_task(
    mut writer: OwnedWriteHalf,
    mut outbound: mpsc::Receiver<Vec<u8>>,
    shared: Shared,
) {
    while let Some(line) = outbound.recv().await {
        if let Err(error) = writer.write_all(&line).await {
            tracing::debug!(%error, "AGP write failed");
            shared.close_with(CloseReason::io(&error));
            return;
        }
    }
    // Queue closed: the connection is going away, drop the write half.
}

/// Read one NDJSON line with a hard length cap.
///
/// Returns `Ok(None)` on clean EOF. A line exceeding `max_frame_len` (or EOF in
/// the middle of a line) is a [`ClientError::Protocol`] / [`ClientError::Io`]
/// and terminates the connection. An oversized line is rejected as soon as the
/// cap is crossed, so it is never buffered in full.
pub(crate) async fn read_line<R>(reader: &mut R, max_frame_len: usize) -> Result<Option<Vec<u8>>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // Chunk length and the offset of the terminator, so the borrow of
        // `reader` ends before `consume`.
        let step = {
            let available = reader.fill_buf().await.map_err(ClientError::Io)?;
            if available.is_empty() {
                Step::Eof
            } else if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
                if buf.len() + newline > max_frame_len {
                    return Err(too_long(max_frame_len));
                }
                buf.extend_from_slice(&available[..newline]);
                Step::Line(newline + 1)
            } else {
                if buf.len() + available.len() > max_frame_len {
                    return Err(too_long(max_frame_len));
                }
                buf.extend_from_slice(available);
                Step::More(available.len())
            }
        };
        match step {
            Step::Line(consumed) => {
                reader.consume(consumed);
                return Ok(Some(buf));
            }
            Step::More(consumed) => reader.consume(consumed),
            Step::Eof => {
                if buf.is_empty() {
                    return Ok(None);
                }
                return Err(ClientError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed in the middle of a frame",
                )));
            }
        }
    }
}

/// What one `fill_buf` round trip told us.
enum Step {
    /// A complete line: the terminator was at this offset (inclusive).
    Line(usize),
    /// No terminator yet: consume this many bytes and keep reading.
    More(usize),
    /// The reader reported EOF.
    Eof,
}

/// The error for a frame above the configured cap.
fn too_long(max_frame_len: usize) -> ClientError {
    ClientError::Protocol {
        message: format!(
            "inbound frame exceeds the {max_frame_len}-byte frame limit (max_frame_len)"
        ),
    }
}
