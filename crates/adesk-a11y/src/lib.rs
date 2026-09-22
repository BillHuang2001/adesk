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
//!   accessibility stack goes through [`AccessibilitySource`]: the real
//!   [`AtspiSource`] (the only implementation that owns a D-Bus connection, plus
//!   its [`LazyAtspiSource`] `auto` and [`UnavailableSource`] `off` variants) and
//!   [`FixtureSource`], a deterministic in-memory tree for tests and tools.
//! - **Pure logic stays pure.** [`normalize_role`], the `find_accessible` matcher
//!   (crate-internal) and [`render_text`] are synchronous, D-Bus-free functions
//!   over plain data, so the whole §5.11 surface can be asserted with no session
//!   bus, no toolkit and no display.
//! - **One service, one registry.** [`AccessibilityService`] is runtime-scoped and
//!   cheap to clone: it assigns the `AccessibleId`s an agent sees and bounds every
//!   backend call in time.
//! - **Ids are the runtime's, never the toolkit's.** Backends hand back an opaque
//!   [`ElementHandle`]; the service maps handles to `AccessibleId`s through its
//!   registry, so an agent never sees an AT-SPI or D-Bus path.
//!
//! # Layout
//!
//! | Module | Contents |
//! |---|---|
//! | `error` | [`A11yError`] / [`Result`], mapped onto `adesk_core::Error` |
//! | `role` | [`normalize_role`] — the wire vocabulary of `AccessibleNode::role` |
//! | `source` | [`AccessibilitySource`] and the backend snapshot data types |
//! | `find` | the `find_accessible` matcher (crate-internal) |
//! | `service` | [`AccessibilityService`] — the id registry, the §5.11 queries, the backend time bound |
//! | `fixture` | [`FixtureSource`] — a deterministic in-memory backend |
//! | `atspi` | [`AtspiSource`] / [`LazyAtspiSource`] / [`UnavailableSource`] — the real AT-SPI2 backend over D-Bus, and its selection ([`auto_source`] / [`unavailable_source`]) |
//! | `text` | [`render_text`] and [`TextOptions`] — the §5.11 outline |

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod atspi;
mod error;
mod find;
mod fixture;
mod role;
mod service;
mod source;
mod text;

pub use atspi::{auto_source, unavailable_source, AtspiSource, LazyAtspiSource, UnavailableSource};
pub use error::{A11yError, Result};
pub use fixture::{node, FixtureSource, NodeBuilder};
pub use role::normalize_role;
pub use service::{
    AccessibilityService, FindOutcome, FindQuery, TreeOptions, FIND_MAX_DEPTH, FIND_MAX_NODES,
    MAX_TRACKED_ELEMENTS, SNAPSHOT_TIMEOUT,
};
pub use source::{
    AccessibilitySource, ElementHandle, InvokeOutcome, SourceNode, SourceOptions, SourceSnapshot,
    WindowTarget,
};
pub use text::{render_text, TextOptions};
