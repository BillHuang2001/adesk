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
#![forbid(unsafe_code)]
#![deny(missing_docs)]
