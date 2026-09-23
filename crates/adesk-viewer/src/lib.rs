//! ADesk viewer — desktop projection for a human outside the AI machine.
//!
//! The crate has two halves plus a binary:
//! - a **server session** ([`ViewerServer`]) that streams rendered desktop frames
//!   and desktop metadata to a connected viewer and applies the viewer's input
//!   through a [`ViewerBackend`] trait the runtime implements;
//! - an **async client SDK** ([`ViewerClient`]) that connects over a Unix or TCP
//!   transport, yields desktop frames as a stream and sends human input;
//! - a headless `adesk-viewer` binary that connects, captures frames and can
//!   drive input from a script.
//!
//! The wire is `adesk-viewer-proto` (VAP v1); the normative specification is
//! `docs/viewer.md`. The crate is transport-agnostic: [`ViewerServer::serve`]
//! takes an already-connected stream and [`ViewerClient`] dials one; binding a
//! listener and implementing [`ViewerBackend`] are `adesk-server`'s job.
//!
//! Which Unix socket the viewer endpoint lives on is decided once, in
//! [`resolve_socket_path`] — the single source of truth every VAP client
//! (this binary, and a GUI front-end) shares, mirroring the server's bind
//! logic so a default-configured client and server always meet.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod backend;
mod capture;
mod client;
mod error;
mod reply;
mod script;
mod server;
mod session;
mod socket;
#[cfg(test)]
mod test_support;
mod transport;

pub use backend::{ChangeSignal, RecordRequest, ViewerBackend, ViewerInput};
pub use capture::{save_frame_png, write_rgba8, FrameWriter};
pub use client::{
    ConnectOptions, ViewerClient, ViewerTarget, DEFAULT_CONNECT_TIMEOUT, DEFAULT_HANDSHAKE_TIMEOUT,
    DEFAULT_MAX_FRAME_LEN,
};
pub use error::{Result, ViewerError};
pub use script::{parse_script, ScriptCommand, ScriptError};
pub use server::{PeerInfo, ViewerServer, ViewerServerConfig};
pub use socket::{resolve_socket_path, viewer_socket_sibling, VIEWER_SOCKET_FILE_NAME};

// The NDJSON framing helpers are `pub` so the crate's integration tests (which
// live outside the crate and cannot see a private module) can frame a duplex
// stream exactly the way the session and the client do. Hidden from the docs
// because they are not part of the supported public surface.
#[doc(hidden)]
pub use transport::{read_line, read_line_into, write_line};
