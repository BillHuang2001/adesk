//! adesk-render — the offscreen render pipeline of the ADesk runtime.
//!
//! This crate owns everything between "a window's surface tree exists" and
//! "here is an [`adesk_core::ImageBuffer`] with damage evidence": the scene
//! description, the offscreen target, the read-back, crop/downscale and damage
//! coalescing. It implements `docs/architecture.md` §5.
//!
//! # Division of responsibility
//!
//! The concrete renderer is **created and owned by `adesk-compositor`**:
//! `GlesRenderer` over surfaceless EGL, or `PixmanRenderer` for the software
//! path. This crate is generic over Smithay's renderer traits
//! (`Renderer`/`Bind`/`Offscreen`/`ExportMem`/`ImportAll`) and never names a
//! backend, never depends on `adesk-compositor`, and never contains unsafe
//! code.
//!
//! ```text
//! adesk-compositor                      adesk-render
//! ─────────────────                     ────────────
//! renderer (GL / pixman) ──&mut──▶ create_target ──▶ OffscreenTarget<T>
//! build elements (ImportAll) ─────▶ Scene<E> + SceneNode<E> (locations, damage)
//! window geometry / region ───────▶ RenderConfig
//!                                render_scene(...) ──▶ RenderedFrame
//!                                     { image: ImageBuffer, commit_seq, damage }
//! ```
//!
//! # Invariants
//!
//! - **On demand only.** Nothing here runs per frame; a render happens because
//!   a `RenderWindow`/`RenderOutput` command asked for one.
//! - **Coordinates.** Scene coordinates are window-relative pixels;
//!   [`RenderConfig::source`] is mapped to target pixel `(0, 0)`. Damage in
//!   [`RenderedFrame::damage`] stays in scene coordinates, independent of crop
//!   and downscale.
//! - **Never panic.** Renderer, import and encoding failures are structured
//!   [`RenderError`]s that map to AGP `ErrorCode`s; the software path reports
//!   unsupported DMA-BUF formats instead of crashing.
//! - **Headless.** [`crop`], [`downscale`], [`encode_png`], [`fit_dimensions`],
//!   [`image_from_readback`] and the damage helpers are pure and need no GPU;
//!   only the renderer-backed entry points do.
//!
//! See `CONTEXT.md` for the exact Smithay 0.7 API this crate builds on and the
//! test strategy.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod config;
pub mod damage;
pub mod error;
pub mod image;
pub mod output;
pub mod pipeline;
pub mod pool;
pub mod scene;

pub use config::{RenderConfig, DEFAULT_CLEAR_COLOR, READBACK_FORMAT, TARGET_FORMAT};
pub use damage::{coalesce_damage, DamageAccumulator};
pub use error::{RenderError, Result};
pub use image::{crop, downscale, encode_png, fit_dimensions, image_from_readback};
pub use output::{create_target, OffscreenTarget, RenderedFrame};
pub use pipeline::render_scene;
pub use pool::TargetPool;
pub use scene::{Scene, SceneNode};
