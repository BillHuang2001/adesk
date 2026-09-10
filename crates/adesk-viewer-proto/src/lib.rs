//! ADesk Viewer Attachment Protocol (VAP) v1 — wire messages and NDJSON codec.
//!
//! The normative specification is `docs/viewer.md`; this crate implements it
//! exactly and invents no message or field outside it. The crate is pure
//! serialization: no I/O, no async, no transport. `adesk-viewer` (server session
//! + client) and `adesk-server` (the runtime endpoint) build on this surface.
//!
//! Implementation is in progress; the module layout and public surface are
//! documented in `CONTEXT.md`.
#![forbid(unsafe_code)]
#![deny(missing_docs)]
