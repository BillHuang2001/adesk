//! adesk-compositor — the ADesk headless Wayland compositor.
//!
//! This crate owns the single-threaded Smithay core of the ADesk runtime: a
//! `calloop` event loop holding one `Display<State>`, all v1 protocol globals, one
//! virtual output, the seat (keyboard + pointer), the window-management bridge and
//! a headless renderer. It is the only crate that may touch Smithay state.
//!
//! # Threading contract
//!
//! [`spawn`] starts one dedicated thread. Everything Smithay-related (including the
//! renderer) lives on that thread and is never `Send`. The rest of the runtime talks
//! to it through exactly three channels:
//!
//! 1. **Commands** (server → compositor): [`CompositorHandle::send`] delivers a
//!    [`RuntimeCommand`] over a `calloop::channel::Sender`; each result-bearing command
//!    carries its own `tokio::sync::oneshot::Sender`. Commands are served in FIFO order.
//! 2. **Events** (compositor → server): [`CompositorHandle::events`] returns a
//!    `tokio::sync::broadcast::Sender<RuntimeEvent>` (capacity ≥ 4096). Sending never
//!    blocks the loop; a full channel never stalls the compositor.
//! 3. **Lifecycle**: [`CompositorHandle::wait_ready`] resolves when the Wayland socket
//!    is bound and the renderer is chosen.
//!
//! # Scope
//!
//! The compositor has **no agent semantics**: no quiet/observe timers, no image
//! encoding, no Unix socket server, no rendering unless a `RenderWindow` /
//! `RenderOutput` command asks for it. It reports what happened; the server decides
//! what it means.
//!
//! # Example
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use adesk_compositor::{spawn, CompositorConfig, RuntimeCommand};
//!
//! let handle = spawn(CompositorConfig::default())?;
//! let ready = handle.wait_ready().await?;
//! println!("wayland display: {}", ready.display_name);
//!
//! let snapshot = {
//!     let (tx, rx) = tokio::sync::oneshot::channel();
//!     handle.send(RuntimeCommand::QueryState { reply: tx })?;
//!     rx.await?
//! };
//! println!("{} windows", snapshot.windows.len());
//!
//! handle.shutdown().await?;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

pub mod command;
pub mod config;
pub mod error;
pub mod handle;
pub mod snapshot;

mod events;
mod input;
mod protocols;
mod render;
mod run;
mod socket;
mod state;
mod wm;

pub use command::RuntimeCommand;
pub use config::{CompositorConfig, RendererKind, RendererName, XkbSettings};
pub use error::{CompositorError, Result};
pub use handle::{spawn, CompositorHandle, ReadyInfo};
pub use input::{KeyCode, Keysym};
pub use snapshot::{RenderedFrame, StateSnapshot};
