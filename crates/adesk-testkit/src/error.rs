//! The crate error type.
//!
//! Every fallible testkit operation returns [`Result<T>`] and every failure mode a test
//! can hit — a dead runtime, a closed Wayland connection, an expired deadline, a lagging
//! event tap — is a distinct variant so a failing test reports *why* it failed instead of
//! hanging. Assertion helpers ([`crate::ImageAssert`]) deliberately panic instead: they are
//! assertions, not fallible plumbing (see the crate docs).

use std::path::PathBuf;
use std::time::Duration;

/// The testkit error type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestkitError {
    /// `adesk_server::Server::start` failed.
    #[error("runtime startup failed: {0}")]
    Startup(String),
    /// The running server reported an error while shutting down.
    #[error("runtime shutdown failed: {0}")]
    Shutdown(String),
    /// The runtime did not shut down within the configured deadline.
    #[error("runtime shutdown timed out after {timeout:?}")]
    ShutdownTimeout {
        /// The deadline that expired.
        timeout: Duration,
    },
    /// Connecting to the compositor's Wayland socket failed.
    #[error("wayland connection failed: {0}")]
    WaylandConnect(String),
    /// A Wayland protocol or dispatch error.
    #[error("wayland protocol error: {0}")]
    Wayland(String),
    /// The Wayland connection was closed (compositor shut down or client dropped).
    #[error("wayland connection closed")]
    ConnectionClosed,
    /// A bounded wait expired. `what` names the condition that never became true.
    #[error("timed out after {timeout:?} waiting for {what}")]
    Timeout {
        /// Human-readable description of the awaited condition.
        what: &'static str,
        /// The deadline that expired.
        timeout: Duration,
    },
    /// The surface has no pending xdg configure to apply.
    #[error("no pending xdg configure for this surface")]
    NoPendingConfigure,
    /// The surface was already destroyed.
    #[error("surface has been destroyed")]
    SurfaceDestroyed,
    /// The event tap lagged behind the compositor's broadcast channel.
    #[error("event tap lagged; {skipped} events were dropped")]
    Lagged {
        /// Number of events dropped by the broadcast channel.
        skipped: u64,
    },
    /// Fixture setup (writing `.desktop` files, temp dirs) failed.
    #[error("fixture error: {0}")]
    Fixture(String),
    /// An event or condition that the test asserted must not happen, happened.
    #[error("unexpected: {message}")]
    Unexpected {
        /// What was unexpected.
        message: String,
    },
    /// The helper binary could not be located next to the running test executable.
    #[error("helper binary `{name}` not found (searched from {searched_from}); build it with `cargo build -p adesk-testkit`")]
    HelperNotFound {
        /// Binary name, e.g. `adesk-test-app`.
        name: String,
        /// Directory the search started from (`std::env::current_exe`).
        searched_from: PathBuf,
    },
    /// The requested capability is unavailable in this environment (e.g. GL without
    /// `ADESK_TEST_GL=1`).
    #[error("unsupported in this environment: {0}")]
    Unsupported(String),
    /// Filesystem, socket or process I/O failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// An AGP client call failed.
    #[error("agp client error: {0}")]
    Client(#[from] adesk_client::ClientError),
    /// A compositor command failed.
    #[error("compositor error: {0}")]
    Compositor(#[from] adesk_compositor::CompositorError),
    /// A domain error from `adesk-core` (unknown window, invalid request, ...).
    #[error("adesk core error: {0}")]
    Core(#[from] adesk_core::Error),
}

/// Testkit result alias.
pub type Result<T, E = TestkitError> = std::result::Result<T, E>;
