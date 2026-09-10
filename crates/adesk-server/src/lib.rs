//! ADesk runtime server — the AGP endpoint that composes the whole runtime.
//!
//! `adesk-server` is both the runtime binary (`adesk-server`) and a library.
//! [`Server::start`] spawns the compositor thread, waits for it to become ready,
//! binds the AGP Unix socket, starts the event pump that feeds the observer and
//! the subscription fan-out, and serves every AGP v1 method
//! (`docs/protocol.md` §5).
//!
//! Threading (docs/architecture.md §1): the compositor owns its own calloop
//! thread; this crate is tokio. The only cross-thread traffic is the
//! `RuntimeCommand` channel and the `RuntimeEvent` broadcast, both owned by
//! `adesk-compositor`.
//!
//! [`crate::dispatch`] routes one arm per AGP method and answers every request
//! with exactly one response; [`crate::event_pump`] feeds the observer, the
//! subscription fan-out and the inspection cache from the compositor's event
//! broadcast.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Server configuration and CLI value parsing.
pub mod config;
/// One AGP client connection: NDJSON framing and the request loop.
pub mod connection;
/// Shared runtime context handed to every connection and dispatch arm.
pub mod context;
/// AGP §5 dispatch: one arm per method, one response per request.
pub mod dispatch;
/// Server error type and AGP error mapping.
pub mod error;
/// Compositor `RuntimeEvent` broadcast → observer, subscribers and caches.
pub mod event_pump;
/// `adesk_core::ImageBuffer` → AGP `ImagePayload` encoding.
pub mod images;
/// Server-side `InspectionSource` (cached inspection state + async refresh).
pub mod inspection;
/// `Server::start` and the `RunningServer` handle.
pub mod server;
/// Per-connection session state and the ordered input queue.
pub mod session;
/// Shutdown coordination and signal handling.
pub mod shutdown;
/// Binding and accepting the AGP Unix socket.
pub mod socket;
/// Event subscriptions (§5.6) and inspector streams (§5.7).
pub mod subscriptions;
/// Bridges between sibling crates' overlapping types.
pub mod translate;
/// Viewer-facing VAP v1 endpoint (backend, transport binding, accept loops).
pub mod viewer;

pub use config::{default_socket_path, ServerConfig, ViewerConfig};
pub use context::ServerContext;
pub use error::{Result, ServerError};
pub use server::{RunningServer, Server};

/// AGP protocol version this runtime serves (`docs/protocol.md` §1).
pub const PROTOCOL_VERSION: u32 = adesk_proto::PROTOCOL_VERSION;

/// Runtime version string reported by `ping.runtime_version`.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");
