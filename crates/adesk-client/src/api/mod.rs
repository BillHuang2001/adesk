//! AGP method surface, grouped by `docs/protocol.md` §5 section.
//!
//! Each module owns the typed request/result types of its section **and** the
//! `impl Client` block that marshals them onto the wire. Marshalling is
//! deliberately mechanical: build the params object, call
//! [`Client::request`](crate::Client), return the typed result. All semantics
//! (waiting, filtering, ordering) live in the server; the client never
//! synthesises protocol behaviour.

pub(crate) mod apps;
pub(crate) mod capture;
pub(crate) mod input;
pub(crate) mod inspect;
pub(crate) mod runtime;
pub(crate) mod subscribe;
pub(crate) mod windows;

use serde::Serialize;

/// Params object for methods that take none (`{}` in `docs/protocol.md`).
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct NoParams {}

/// Result object for methods that return `{}`.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub(crate) struct Empty {}

/// Result object for the many methods that return `{"action_id": u64}`.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub(crate) struct ActionIdResult {
    pub(crate) action_id: adesk_core::ActionId,
}
