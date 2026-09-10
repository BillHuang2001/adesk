//! # adesk-inspector — human inspection projection for the ADesk runtime
//!
//! Human inspection is an **optional projection** of the runtime's internal
//! state, secondary to agent-facing capture: the runtime renders the full
//! virtual output on demand, this crate composes debug overlays
//! (`docs/protocol.md` §5.7 — `inspect_capture` / `inspect_subscribe`,
//! [`OverlayKind`](adesk_core::OverlayKind)) onto that frame, and
//! `adesk-server` encodes the result.
//! Agent-facing `capture_*` / `observe` images never go through this crate.
//!
//! Properties (binding):
//!
//! - Pure and synchronous: no I/O, no async, no Smithay, no tokio.
//! - The only input is an [`InspectionInput`]: a base
//!   [`ImageBuffer`](adesk_core::ImageBuffer) plus the overlay-relevant runtime
//!   state. The runtime side is connected through [`InspectionSource`], which
//!   `adesk-server` implements — this crate never depends on the compositor or
//!   the server.
//! - Overlays are drawn in a fixed canonical order ([`CANONICAL_ORDER`]), never
//!   in caller order, so the same [`InspectionInput`] always produces the same
//!   bytes.
//! - Text is rendered with a built-in 5x7 monospace bitmap font ([`font`]);
//!   there are no font dependencies.
//! - Post-processing of inspection frames (`region`, `max_dimension`) reuses
//!   `adesk-render`'s crop/downscale so `inspect_capture` and agent-facing
//!   `capture_*` scale identically.
//!
//! ```no_run
//! use adesk_core::{ImageBuffer, OverlayKind};
//! use adesk_inspector::{InspectionInput, Inspector};
//!
//! let frame = ImageBuffer::new_rgba(1280, 800);
//! let inspector = Inspector::new(vec![OverlayKind::WindowIds, OverlayKind::Focus]);
//! let input = InspectionInput::new(frame);
//! let composed = inspector.render(&input).expect("overlay composition");
//! assert_eq!(composed.size(), adesk_core::Size::new(1280, 800));
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod canvas;
pub mod color;
pub mod error;
pub mod font;
pub mod input;
pub mod inspector;
pub mod paint;
pub mod source;
pub mod style;
pub mod text;

mod post;

pub use canvas::{blend_over, Canvas};
pub use color::Color;
pub use error::{Error, Result};
pub use input::{ActionKind, ActionMarker, CommitInfo, InspectionInput, InspectionInputBuilder};
pub use inspector::Inspector;
pub use paint::CANONICAL_ORDER;
pub use source::{InspectionRequest, InspectionSource};
pub use style::OverlayStyle;
