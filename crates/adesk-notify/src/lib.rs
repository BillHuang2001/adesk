//! adesk-notify — the runtime's notification store and agent event inbox.
//!
//! This crate owns the runtime's notification subsystem and the event inbox that
//! backs the agent's reactive `wait_for_events` idle primitive
//! (`docs/protocol.md` §5.9/§5.10, `docs/notifications.md`).
//!
//! The store is mutated synchronously by the §5.9 request handlers; the inbox is
//! fed by the server's event pump and answers `wait_for_events`.
#![forbid(unsafe_code)]
#![deny(missing_docs)]
