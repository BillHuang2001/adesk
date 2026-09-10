//! ADesk viewer — desktop projection for a human outside the AI machine.
//!
//! The crate has two halves plus a binary:
//! - a **server session** (`ViewerServer`) that streams rendered desktop frames
//!   and desktop metadata to a connected viewer and applies the viewer's input
//!   through a [`backend`] trait the runtime implements;
//! - an **async client SDK** (`ViewerClient`) that connects over a Unix or TCP
//!   transport, yields desktop frames as a stream and sends human input;
//! - a headless `adesk-viewer` binary that connects, captures frames and can
//!   drive input from a script.
//!
//! The wire is `adesk-viewer-proto` (VAP v1); the normative specification is
//! `docs/viewer.md`. Implementation is in progress; the module layout and public
//! surface are documented in `CONTEXT.md`.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod backend;
pub mod capture;
pub mod client;
pub mod error;
pub mod script;
pub mod server;
pub mod session;
pub mod transport;
