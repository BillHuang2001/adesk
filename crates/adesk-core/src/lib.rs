//! # adesk-core — shared domain model for the ADesk runtime
//!
//! `adesk-core` is the common ancestor of the workspace: every other crate
//! depends on it. It defines the vocabulary that crosses crate boundaries —
//! identifiers, geometry, window/application descriptions, images, the runtime
//! event vocabulary, and the umbrella error type — and nothing else.
//!
//! Properties (binding, see `docs/core-api.md`):
//!
//! - No I/O, no async, no Smithay, no tokio; dependencies are `serde` and
//!   `thiserror` only.
//! - Wire-facing types derive `Serialize`/`Deserialize` with
//!   `#[serde(rename_all = "snake_case")]` and tagged enums where applicable.
//! - [`ImageBuffer`] is deliberately **not** wire-facing; the protocol carries
//!   base64 image payloads defined in `adesk-proto`.
//! - `todo!()` is never used: this crate *is* the interface and is fully
//!   implemented.
//!
//! ```
//! use adesk_core::{Point, Position, Rect};
//!
//! let window = Rect { x: 0, y: 0, w: 100, h: 50 };
//! // Pixels are clamped into the window.
//! assert_eq!(
//!     Position::Pixels(Point { x: 250, y: -5 }).resolve(window),
//!     Point { x: 99, y: 0 },
//! );
//! // Normalized values map 0.0 to the first and 1.0 to the last pixel.
//! assert_eq!(
//!     Position::Normalized { x: 0.5, y: 0.5 }.resolve(window),
//!     Point { x: 50, y: 25 },
//! );
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod app;
pub mod error;
pub mod event;
pub mod geometry;
pub mod ids;
pub mod image;
pub mod input;
pub mod position;
pub mod window;

pub use app::AppInfo;
pub use error::{Error, ErrorCode, Result};
pub use event::{EventKind, Observation, RuntimeEvent};
pub use geometry::{Point, Rect, Region, Size};
pub use ids::{ActionId, AppId, LaunchId, WindowId};
pub use image::{ImageBuffer, PixelFormat};
pub use input::{Button, ButtonState, KeyState, OverlayKind};
pub use position::Position;
pub use window::{WindowInfo, WindowState};
