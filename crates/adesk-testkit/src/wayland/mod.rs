//! The Wayland test client: real protocol path, known SHM fills, bounded pumps.
//!
//! PLACEHOLDER — replaced by the full `state`/`shm`/`window`/`protocol` split
//! during the same architecture phase. It only exists so the crate compiles
//! while the real modules are written in a parallel worktree; it must keep
//! [`WaylandTestClient::connect_in`] working because
//! [`crate::TestRuntime::wayland_client`] calls it.

use std::path::Path;

use crate::error::Result;

/// A client speaking the real Wayland protocol to a test runtime (placeholder).
pub struct WaylandTestClient;

impl WaylandTestClient {
    /// Connects to `display_name` inside `runtime_dir` (placeholder).
    pub fn connect_in(runtime_dir: &Path, display_name: &str) -> Result<WaylandTestClient> {
        let _ = (runtime_dir, display_name);
        todo!("stub: replaced by the real wayland module")
    }
}

/// Description of a toplevel to create (placeholder).
pub struct ToplevelSpec;

/// An xdg configure the compositor sent (placeholder).
pub struct ConfiguredSize;

/// Counters from a bounded event pump (placeholder).
pub struct PumpStats;

/// A toplevel created by the test client (placeholder).
pub struct TestWindow;

/// Description of a popup to create (placeholder).
pub struct PopupSpec;

/// A popup created by the test client (placeholder).
pub struct TestPopup;
