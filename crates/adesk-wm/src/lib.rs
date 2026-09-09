//! # adesk-wm — window model and single-visible-toplevel tiling policy
//!
//! `adesk-wm` owns the *policy* of the ADesk compositor: which window is
//! visible, how windows are tiled into the virtual output, which ids windows
//! get, and how window-relative coordinates become output coordinates.
//! `adesk-compositor` owns the Smithay objects; this crate owns the window
//! records and never touches Wayland, the GPU, the clock or the filesystem.
//!
//! ## Properties
//!
//! - **Pure logic.** No Smithay, no async, no I/O, no timers. `tracing` is
//!   used for `trace`-level policy transitions only.
//! - **Decisions are returned, not executed.** Mutating methods return a
//!   [`Vec<WmAction>`](WmAction) that the compositor applies to its Smithay
//!   objects, so the policy is unit-testable without a compositor.
//! - **The policy is isolated.** Every decision lives in the crate-internal
//!   `policy` module as a pure function over the internal window model;
//!   multi-window support is added there without changing this public API or
//!   the compositor's call sites.
//!
//! ## Invariants (binding: `docs/architecture.md` §4)
//!
//! 1. At most one window is [`adesk_core::WindowState::Active`]; every other
//!    mapped window is `Inactive` and stays mapped (invisible, but still
//!    configured to the tiled rect).
//! 2. A mapped window is tiled to [`WindowManager::tiled_rect`] — the whole
//!    virtual output at `(0, 0)`. Window geometry is policy-owned: the client
//!    never decides where a window lives.
//! 3. [`adesk_core::WindowId`]s are assigned on first map, monotonically
//!    increasing, and never reused. Id `0` is never assigned.
//! 4. Mapping a window activates it and deactivates the previously active
//!    window ([`WmAction::ConfigureWindow`] then [`WmAction::Activate`]).
//! 5. Destroying the active window activates the most recently used remaining
//!    window ([`WmAction::ActivatePrevious`]); destroying the last window
//!    leaves no active window.
//! 6. Window-relative [`adesk_core::Position`]s are resolved through the
//!    window's geometry ([`WindowManager::resolve_position`]); the output
//!    origin is never hard-coded.
//!
//! ## Example
//!
//! ```no_run
//! use adesk_core::{Point, Position, Rect, Size, WindowId};
//! use adesk_wm::{MapRequest, PolicyConfig, SurfaceKey, WindowManager, WmAction};
//!
//! let mut wm = WindowManager::new(PolicyConfig::new(Size::new(1280, 800)));
//!
//! // First map: id 1, tiled configure + activation.
//! let (id, actions) = wm.on_map(MapRequest::new(SurfaceKey::new(1)));
//! assert_eq!(id, WindowId(1));
//! assert_eq!(
//!     actions,
//!     vec![
//!         WmAction::ConfigureWindow { id, rect: Rect::new(0, 0, 1280, 800) },
//!         WmAction::Activate { id },
//!     ]
//! );
//!
//! // A second map deactivates the first window but keeps it mapped.
//! let (id2, _) = wm.on_map(MapRequest::new(SurfaceKey::new(2)));
//! assert_eq!(wm.active_window(), Some(id2));
//!
//! // Window-relative input resolves through the window geometry.
//! assert_eq!(
//!     wm.resolve_position(id2, Position::normalized(1.0, 1.0)),
//!     Some(Point::new(1279, 799))
//! );
//! assert_eq!(
//!     wm.resolve_position(id2, Position::pixels(-10, -10)),
//!     Some(Point::new(0, 0))
//! );
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod action;
pub mod config;
pub mod error;
pub mod manager;
pub mod model;

pub(crate) mod policy;

pub use action::WmAction;
pub use config::PolicyConfig;
pub use error::{Error, Result};
pub use manager::WindowManager;
pub use model::{MapRequest, SurfaceKey, WindowRecord};
