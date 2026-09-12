# adesk-notify — notification store + agent event inbox
## Intent
`adesk-notify` owns the ADesk runtime's notification subsystem and the event inbox that backs the agent's reactive `wait_for_events` idle primitive.
`docs/protocol.md` §5.9 (notifications) and §5.10 (event waits) are the normative wire contract; `docs/notifications.md` is the design record; `docs/architecture.md` §11 is the binding cross-crate contract.
Two cohesive responsibilities: a synchronous notification **store** (id allocation + lifecycle) mutated only by §5.9 request handlers, and an **event inbox** (a bounded journal of `RuntimeEvent`s plus async `wait_for_events` waiters) fed by the server's event pump.
They are deliberately separate so a store mutation and the event it publishes can never disagree.
Like `adesk-observer`: pure consumer, no Smithay, no compositor state, no rendering, no filesystem/socket I/O, cheap-clone `Arc<Inner>` facade.
## API Surface (planned)
- `NotificationService` — `new()`, `post(NewNotification) -> Notification`, `close(NotificationId, NotificationCloseReason) -> Result<Notification>`, `invoke_action(NotificationId, &str) -> Result<()>`, `list(include_dismissed: bool) -> Vec<Notification>`, `handle_event(&RuntimeEvent)`, `wait_for_events(EventWaitSpec) -> Result<EventBatch>`.
- `NewNotification` — the accepted fields of §5.9 `post_notification` minus ids/seqs.
- `EventWaitSpec { kinds, window_id, timeout_ms, max_events, since_seq }`, `EventBatch { events, timed_out, elapsed_ms, seq }`, `EventRecord { kind, seq, ts_ms, data }`.
- `Error` + `Result<T>` (`unknown_notification`, `invalid_request`).
## Constraints
- `docs/protocol.md` is normative; never invent methods or fields.
- Dependencies: `adesk-core`, `tokio` (sync/time), `thiserror`, `tracing`; versions only from root `[workspace.dependencies]`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; no `todo!()`; no panics on request/event paths.
- Never hold the state mutex across an `.await`; the service never spawns background tasks (the server drives it, like the observer).
- Keep files well under the ~1000-line concern threshold.
## Routing Table
| Area | Owner |
|---|---|
| Notification store, id allocation, lifecycle | `./src/` (to be implemented) |
| Event inbox, `wait_for_events` waiters | `./src/` (to be implemented) |
| Consumer: §5.9/§5.10 dispatch, event pump feed | `../adesk-server/` (sibling — read-only) |
| `Notification`, `NotificationId`, `RuntimeEvent`, `EventKind`, `Error` | `../adesk-core/` (dependency — read-only) |
## Status
Scaffold only (crate manifest + empty lib). The store and inbox are implemented by the notifications feature work.
