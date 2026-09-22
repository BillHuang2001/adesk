//! `adesk-a11y` — the runtime's accessibility (text) view of a window's UI.
//!
//! ADesk reads a window as TEXT through the Linux accessibility stack (AT-SPI2
//! over D-Bus) instead of pixels: the runtime exposes a toolkit's accessibility
//! tree — roles, names, values, state flags, actions and element bounds — as a
//! first-class, text-only observation alongside the pixel one
//! (`docs/accessibility.md`, `docs/protocol.md` §5.11, `docs/architecture.md`
//! §12). Text costs orders of magnitude fewer tokens than the frame it was
//! rendered into, and it is what lets an agent *address* an element (id, action
//! name, state) instead of guessing a coordinate.
//!
//! This crate is the layer above `adesk-core` and below `adesk-server`:
//! `adesk-core` owns the value vocabulary (`AccessibleId`, `AccessibleState`,
//! `AccessibleNode`, `AccessibleTree`, `AccessibleMatch`), `adesk-proto` owns the
//! §5.11 wire payloads, and this crate turns a backend walk into those values and
//! renders the outline.
//!
//! # Design
//!
//! - **This crate is the only place in the runtime that speaks D-Bus.** The
//!   compositor, the window model, the observer and the event pump are untouched
//!   by accessibility: there is no accessibility event and no accessibility
//!   `EventKind`, so the text view is read strictly on demand by the §5.11
//!   request handlers and never pushed.
//! - **One seam, several backends.** Everything the runtime needs from the
//!   accessibility stack goes through [`AccessibilitySource`], so a test or a
//!   tool can supply a deterministic in-memory tree instead of a real bus, and
//!   the AT-SPI backend stays the only implementation that owns a connection.
//! - **Pure logic stays pure.** [`normalize_role`], the `find_accessible` matcher
//!   (crate-internal) and [`render_text`] are synchronous, D-Bus-free functions
//!   over plain data, so the whole §5.11 surface can be asserted with no session
//!   bus, no toolkit and no display.
//! - **Ids are the runtime's, never the toolkit's.** Backends hand back an opaque
//!   [`ElementHandle`]; `adesk-server` maps handles to `AccessibleId`s through the
//!   registry of the service, so an agent never sees an AT-SPI or D-Bus path.
//!
//! # Layout
//!
//! | Module | Contents |
//! |---|---|
//! | `error` | [`A11yError`] / [`Result`], mapped onto `adesk_core::Error` |
//! | `role` | [`normalize_role`] — the wire vocabulary of `AccessibleNode::role` |
//! | `source` | [`AccessibilitySource`] and the backend snapshot data types |
//! | `find` | the `find_accessible` matcher (crate-internal) |
//! | `text` | [`render_text`] and [`TextOptions`] — the §5.11 outline |

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
// The `find_accessible` matcher is crate-internal: the §5.11 service that calls
// it lands with the backend milestone, so until then it has no in-crate caller
// and `dead_code` would otherwise reject a module this crate ships deliberately.
#[allow(dead_code)]
mod find;
mod role;
mod source;
mod text;

pub use error::{A11yError, Result};
pub use role::normalize_role;
pub use source::{
    AccessibilitySource, ElementHandle, SourceNode, SourceOptions, SourceSnapshot, WindowTarget,
};
pub use text::{render_text, TextOptions};
