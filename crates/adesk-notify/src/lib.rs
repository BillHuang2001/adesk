//! `adesk-notify` — the runtime's notification store and its agent event inbox.
//!
//! The crate owns two cohesive, deliberately separate concerns
//! (`docs/notifications.md`, `docs/architecture.md` §11):
//!
//! - a synchronous notification **store** (identifier allocation + lifecycle),
//!   mutated only by the `docs/protocol.md` §5.9 request handlers
//!   ([`NotificationService::post`], [`NotificationService::close`],
//!   [`NotificationService::invoke_action`]); and
//! - an **event inbox** — a bounded journal of [`adesk_core::RuntimeEvent`]s plus
//!   the `wait_for_events` waiters of §5.10 — fed by the server's event-pump task
//!   ([`NotificationService::handle_event`], one event per call in `seq` order).
//!
//! Keeping them apart means a store mutation and the event it publishes can never
//! disagree. Like the observer, the service never renders, never touches the
//! compositor and never sleeps in a loop: waits are woken by a
//! `tokio::sync::watch` generation counter and bounded by `tokio::time`, so
//! `tokio::time::pause()` makes them deterministic.
//!
//! # Wiring (server side)
//!
//! ```no_run
//! # use adesk_notify::{EventWaitSpec, NewNotification, NotificationService};
//! # use adesk_core::{EventKind, NotificationCloseReason};
//! # fn spawn_pump(_: NotificationService, mut events: tokio::sync::broadcast::Receiver<adesk_core::RuntimeEvent>) {}
//! # async fn example(mut events: tokio::sync::broadcast::Receiver<adesk_core::RuntimeEvent>) {
//! let notify = NotificationService::new();
//!
//! // 1. One event-pump task feeds every broadcast event into the inbox.
//! spawn_pump(notify.clone(), events);
//!
//! // 2. The server reserves a `seq` from the compositor's single counter and
//! //    passes the monotonic `ts_ms`, then publishes the returned notification.
//! let notification = notify
//!     .post(NewNotification::new("Build finished").source("user"), 42, 1_000)
//!     .expect("non-empty title");
//!
//! // 3. Dismissal is a synchronous store mutation; `newly_dismissed` tells the
//! //    server whether to publish a `notification_closed` event.
//! let _ = notify.close(notification.id, NotificationCloseReason::Dismissed, 43);
//!
//! // 4. The idle primitive: wait on any clone; the wait is cancellable.
//! let batch = notify
//!     .wait_for_events(EventWaitSpec::new().kind(EventKind::Notification))
//!     .await
//!     .expect("unit error is reserved for requests");
//! # }
//! ```
//!
//! # Notes on the wire boundary
//!
//! - This crate owns **no** `seq` counter and **no** clock: the server reserves
//!   the global `seq` (the same counter `AppLaunched` uses) and supplies the
//!   monotonic `ts_ms`, so both come from one source of truth. A `close` that is a
//!   no-op therefore leaves a harmless gap in the published sequence — the server
//!   reserved a `seq` it does not publish, and [`CloseOutcome::newly_dismissed`]
//!   tells it whether to publish at all.
//! - [`EventBatch::events`] is a `Vec<RuntimeEvent>`, **not** a type carrying a
//!   JSON `data` object: this crate must not depend on `serde_json`
//!   (dependencies are fixed), so `adesk-server`/`adesk-proto` assemble the wire
//!   `EventRecord { event, seq, ts_ms, data }` from each event.
//!
//! # Layout
//!
//! | Module | Contents |
//! |---|---|
//! | `store` | `NewNotification`, `CloseOutcome`, the internal `NotificationStore` |
//! | `inbox` | `EventWaitSpec`, `EventBatch`, the internal `EventInbox` + `wait_for_events` |
//! | `service` | `NotificationService` |
//! | `error` | crate-local `Error`/`Result` mapped onto `adesk_core::Error` |

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod inbox;
mod service;
mod store;

pub use error::{Error, Result};
pub use inbox::{EventBatch, EventWaitSpec};
pub use service::NotificationService;
pub use store::{CloseOutcome, NewNotification};

/// Default number of events retained by the inbox journal.
///
/// Matches the observer's journal capacity and the minimum broadcast capacity
/// (`docs/architecture.md` §1, ≥ 4096): a waiter can always be seeded from the
/// journal over the same horizon that the broadcast channel can replay before it
/// reports `Lagged`.
pub const DEFAULT_INBOX_CAPACITY: usize = 4096;

/// Default `timeout_ms` for `wait_for_events` (`docs/protocol.md` §5.10).
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Default `max_events` for `wait_for_events` (`docs/protocol.md` §5.10).
pub const DEFAULT_MAX_EVENTS: u32 = 32;
