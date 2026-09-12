# adesk-notify — notification store + agent event inbox

## Intent

`adesk-notify` owns the ADesk runtime's notification subsystem and the event inbox that backs the agent's reactive `wait_for_events` idle primitive.
`docs/protocol.md` §5.9 (notifications) and §5.10 (event waits) are the normative wire contract; `docs/notifications.md` is the design record; `docs/architecture.md` §11 is the binding cross-crate contract.

Two cohesive responsibilities, deliberately separate so a store mutation and the event it publishes can never disagree:

- a synchronous notification **store** (id allocation + lifecycle) mutated only by the §5.9 request handlers `post`/`close`/`invoke_action`; and
- an **event inbox** (a bounded journal of `RuntimeEvent`s plus async `wait_for_events` waiters) fed by the server's event pump via `handle_event`.

Like `adesk-observer`: pure consumer, no Smithay, no compositor state, no rendering, no filesystem/socket I/O, cheap-clone `Arc<Inner>` facade, `std::sync::Mutex` state never held across an `.await`, and a `tokio::sync::watch` generation counter for lost-wakeup-free waits.

## API Surface

All public; everything else is `pub(crate)`.

- `NotificationService` (`#[derive(Clone)]`, one `Arc<Inner>`) — the facade:
  - `new()` / `Default`
  - `post(NewNotification, seq: u64, ts_ms: u64) -> Result<Notification>` — allocates the next id, stores the notification, returns it for publication. Empty `title` → `Error::InvalidRequest`.
  - `close(NotificationId, NotificationCloseReason, seq: u64) -> Result<CloseOutcome>` — unknown id → `Error::UnknownNotification`; already-dismissed is a no-op (`newly_dismissed = false`).
  - `invoke_action(NotificationId, &str) -> Result<()>` — unknown id → `UnknownNotification`, unknown key → `InvalidRequest`; never dismisses.
  - `list(include_dismissed: bool) -> Vec<Notification>` — newest first (`posted_seq` descending).
  - `get(NotificationId) -> Option<Notification>`
  - `handle_event(&RuntimeEvent)` — feeds the inbox; **never** mutates the store.
  - `async wait_for_events(EventWaitSpec) -> Result<EventBatch>` — delegates to the inbox.
- `NewNotification` — the §5.9 `post_notification` fields minus ids/seqs (`source`, `title`, `body`, `urgency`, `category`, `actions`, `hints`, `timeout_ms`); `Default` matches the §5.9 defaults; `new(title)` + builder methods.
- `CloseOutcome { notification: Notification, newly_dismissed: bool }` — `newly_dismissed` tells the server whether to publish a `notification_closed` event.
- `EventWaitSpec { kinds: Vec<EventKind>, window_id: Option<WindowId>, timeout_ms: u64, max_events: u32, since_seq: Option<u64> }` — `Default` = empty kinds, `None` window, `5000` ms, `32`, `None`; `new()` + builder methods.
- `EventBatch { events: Vec<RuntimeEvent>, timed_out: bool, elapsed_ms: u64, seq: u64 }`.
- `Error` + `Result<T>` (`unknown_notification`, `invalid_request`, `internal`), mapped onto `adesk_core::Error` (`ErrorCode::UnknownNotification` / `InvalidRequest` / `Internal`).
- Constants: `DEFAULT_INBOX_CAPACITY = 4096`, `DEFAULT_TIMEOUT_MS = 5000`, `DEFAULT_MAX_EVENTS = 32`.

## Design Decisions / Constraints

- `docs/protocol.md` is normative; no invented methods or fields.
- The crate owns **no** `seq` counter and **no** clock: the server reserves the global `seq` (the same counter `AppLaunched` uses) and supplies the monotonic `ts_ms`. A no-op `close` therefore leaves a harmless gap in the published sequence; `newly_dismissed` gates publication.
- `EventBatch.events` is `Vec<RuntimeEvent>`, **not** a JSON-`data` carrier: the crate does not depend on `serde_json`, so `adesk-server`/`adesk-proto` assemble the wire `EventRecord { event, seq, ts_ms, data }`.
- `wait_for_events` filter point is `spec.since_seq` when given, else the watermark captured **once** at wait start (re-reading it each iteration would skip the waking event). Empty `kinds` = every kind; `window_id` excludes window-less events (`app_launched`, notifications). `inspect_frame` is wire-only, not a `RuntimeEvent` variant, so nothing to filter.
- `EventBatch.seq` is the inbox watermark at resolution; when `events.len() == max_events` the caller should resume from the last returned event's `seq` to avoid a truncation gap.
- The inbox records **every** event kind (unlike the observer, which ignores the notification kinds).
- Inbox journal is a bounded `VecDeque` (`DEFAULT_INBOX_CAPACITY`); the watermark is the running max `seq` and only ever rises.
- `elapsed_ms` uses `tokio::time::Instant`, so timeouts are deterministic under `tokio::time::pause()` / `#[tokio::test(start_paused = true)]`.
- Dependencies: `adesk-core`, `tokio` (sync/time), `thiserror`, `tracing`; versions only from the root `[workspace.dependencies]` (dev-dependency adds tokio `test-util` only, for `start_paused`).
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; `tracing` for logging (a `debug!` on post/close/invoke and on waiter resolution; never payloads); no panics on request/event paths; saturating arithmetic for `ts_ms`/`elapsed_ms`.

## Routing Table

| Area | Owner |
|---|---|
| Notification store, id allocation, lifecycle (`NewNotification`, `CloseOutcome`) | `./src/store.rs` |
| Event inbox, bounded journal, `wait_for_events` waiters (`EventWaitSpec`, `EventBatch`) | `./src/inbox.rs` |
| `NotificationService` facade (`Arc<Inner>`) | `./src/service.rs` |
| Crate-local `Error`/`Result` → `adesk_core::Error` | `./src/error.rs` |
| Consumer: §5.9/§5.10 dispatch, event pump feed | `../adesk-server/` (sibling — read-only) |
| `Notification`, `NotificationId`, `NotificationUrgency`, `NotificationAction`, `NotificationCloseReason`, `RuntimeEvent`, `EventKind`, `Error`/`ErrorCode` | `../adesk-core/` (dependency — read-only) |

## Status

Implemented and self-contained: store + inbox + service, 32 unit tests plus 1 `no_run` wiring doctest.
`cargo test -p adesk-notify --all-targets`, `cargo clippy -p adesk-notify --all-targets --no-deps -- -D warnings`, `cargo fmt -p adesk-notify -- --check` and `cargo doc -p adesk-notify --no-deps --document-private-items` are all clean.
Server integration (dispatch of §5.9/§5.10, reserving `seq`, driving `handle_event` from the event pump) is a separate task owned by `adesk-server`.
