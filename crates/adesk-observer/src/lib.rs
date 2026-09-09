//! `adesk-observer` — the temporal observation engine of the ADesk runtime.
//!
//! The observer is the runtime's memory of *what happened when*. It consumes the
//! compositor's [`adesk_core::RuntimeEvent`] stream (fed by the server's event pump),
//! keeps per-window temporal state, correlates agent actions with the event stream
//! through the [`ActionRegistry`], and answers the long waits of
//! `docs/protocol.md` §5.4:
//!
//! - [`ObserverService::wait_for_change`] — first counted commit/lifecycle event.
//! - [`ObserverService::wait_for_quiet`] — no counted commit for `quiet_ms`.
//! - [`ObserverService::observe`] — any [`Condition`] (`change`, `quiet`, `timeout`).
//!
//! It never renders, never touches the compositor and never sleeps in a loop: waits
//! are woken by a `tokio::sync::watch` generation counter and bounded by
//! `tokio::time`, so `tokio::time::pause()` makes them deterministic.
//!
//! # Wiring (server side)
//!
//! ```no_run
//! # use adesk_observer::{ActionKind, ObserverService, QuietSpec, StateSnapshot, WaitSpec};
//! # use adesk_core::{Position, WindowId};
//! # fn spawn_pump(_: ObserverService, mut events: tokio::sync::broadcast::Receiver<adesk_core::RuntimeEvent>) {}
//! # async fn example(mut events: tokio::sync::broadcast::Receiver<adesk_core::RuntimeEvent>) {
//! let observer = ObserverService::new();
//!
//! // 1. One event-pump task feeds every broadcast event into the observer.
//! spawn_pump(observer.clone(), events);
//!
//! // 2. Actions are recorded here; the returned id is the `after_action` handle.
//! let action = observer.record_action(
//!     ActionKind::Click,
//!     Some(WindowId(7)),
//!     Some(Position::Normalized { x: 0.5, y: 0.5 }),
//! );
//!
//! // 3. Waiters are plain async calls on any clone of the service.
//! let spec = QuietSpec::new().window(WindowId(7)).after_action(action);
//! let observation = observer.wait_for_quiet(spec).await.expect("known window");
//!
//! // 4. On `RecvError::Lagged`, resync instead of silently continuing.
//! let snapshot = StateSnapshot { seq: observation.seq, ts_ms: 0, windows: vec![] };
//! let _report = observer.resync(snapshot);
//!
//! // 5. `wait_for_change` covers the remaining AGP wait.
//! let _ = observer.wait_for_change(WaitSpec::new()).await;
//! # }
//! ```
//!
//! The server renders images *after* a wait resolves (`include_image`), so the
//! observer's specs deliberately carry no image parameters.
//!
//! # Layout
//!
//! | Module | Contents |
//! |---|---|
//! | [`actions`] | `ActionKind`, `ActionRecord`, `ActionRegistry` |
//! | [`state`] | per-window temporal state, `ObserverSnapshot`, resync snapshot types |
//! | [`spec`] | `WaitSpec`, `QuietSpec`, `ObserveSpec`, `Condition` |
//! | [`service`] | `ObserverService`, `ObserverConfig` |
//! | [`error`] | crate-local `Error`/`Result` mapped onto `adesk_core::Error` |
//!
//! Internal machinery: `journal` (bounded counted-event journal) and `waiter`
//! (filters, accumulator, condition evaluation) — `pub(crate)`, not part of the
//! public surface.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod actions;
mod clock;
mod error;
mod journal;
mod service;
mod spec;
mod state;
mod waiter;

pub use actions::{ActionKind, ActionRecord, ActionRegistry};
pub use error::{Error, Result};
pub use service::{ObserverConfig, ObserverService};
pub use spec::{Condition, ObserveSpec, QuietSpec, WaitSpec};
pub use state::{
    ObserverSnapshot, PendingObservation, ResyncReport, StateSnapshot, WindowSnapshot,
    WindowTemporalState,
};

/// Default number of counted events retained for waiter seeding.
///
/// Matches the minimum broadcast capacity (`docs/architecture.md` §1, ≥ 4096): a
/// waiter can always be seeded from the journal over the same horizon that the
/// broadcast channel can replay before it reports `Lagged`.
pub const DEFAULT_JOURNAL_CAPACITY: usize = 4096;

/// Default `timeout_ms` for every wait (`docs/protocol.md` §5.4).
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Default quiet threshold (`docs/protocol.md` §5.4) used for the `quiet` evidence
/// flag of observations whose condition is not itself a quiet condition.
pub const DEFAULT_QUIET_MS: u64 = 250;
