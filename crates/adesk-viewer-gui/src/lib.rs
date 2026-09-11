//! ADesk viewer GUI — a GTK4/libadwaita desktop front-end for the VAP viewer.
//!
//! This crate is the human-facing projection of an ADesk runtime: it connects to
//! the runtime's viewer endpoint over the Viewer Attachment Protocol (VAP,
//! `docs/viewer.md`), renders the streamed desktop frames, and turns local
//! mouse/keyboard activity into VAP input so the human is placed in the same
//! seat the agent drives — a viewer action is never a special code path.
//!
//! It pairs with the headless `adesk-viewer` binary (frames → PNG, scripted
//! input); this crate owns the interactive GUI instead.
//!
//! The crate is split into GTK-free, unit-testable modules and (later) a GTK
//! layer that builds the widgets on top of them:
//! - [`cli`] — command-line parsing and viewer-endpoint resolution.
//! - [`error`] — the crate error type [`GuiError`] and the [`Result`] alias.
//! - [`image`] — decoding VAP image payloads to tightly packed RGBA8.
//! - [`mapping`] — the pure widget ↔ normalized coordinate letterbox math.
//! - [`taskbar`] — the pure task-bar view model derived from a desktop state.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod cli;
pub mod error;
pub mod image;
pub mod mapping;
pub mod taskbar;

pub use error::{GuiError, Result};
