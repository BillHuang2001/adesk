//! The container backend seam.
//!
//! `ContainerRuntime` is the only interface the machine manager and the host
//! control plane speak; the container engine never leaks past it
//! (`docs/machine.md` §2). Backends live in the [`mock`] and [`podman`]
//! submodules.

pub mod mock;
pub mod podman;

use async_trait::async_trait;

use crate::error::Result;
use crate::spec::MachineSpec;
use crate::state::{MachineId, MachineStatus};

/// Which container backend is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    /// Rootless Podman driven through the `podman` command line.
    Podman,
    /// In-memory deterministic backend for tests and `--runtime mock`.
    Mock,
}

impl RuntimeKind {
    /// The stable lowercase name, matching the `--runtime` CLI values.
    pub const fn as_str(&self) -> &'static str {
        match self {
            RuntimeKind::Podman => "podman",
            RuntimeKind::Mock => "mock",
        }
    }
}

impl std::fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The backend seam: create, drive and inspect machines.
///
/// Implementations are `Send + Sync + 'static` so a single backend can be shared
/// across tokio tasks. Every method returns a `MachineError` on failure; none
/// panics on ordinary request/command paths.
#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    /// Which backend this is.
    fn kind(&self) -> RuntimeKind;

    /// Creates a container from `spec` and returns its backend-assigned id.
    async fn create(&self, spec: &MachineSpec) -> Result<MachineId>;

    /// Starts the machine identified by `id`.
    async fn start(&self, id: &MachineId) -> Result<()>;

    /// Stops the machine, waiting at most `timeout_ms` for a clean shutdown.
    async fn stop(&self, id: &MachineId, timeout_ms: u64) -> Result<()>;

    /// Removes the machine's container; `force` kills a running container first.
    async fn remove(&self, id: &MachineId, force: bool) -> Result<()>;

    /// Returns the current status of the machine identified by `id`.
    async fn status(&self, id: &MachineId) -> Result<MachineStatus>;

    /// Lists every machine the backend knows about.
    async fn list(&self) -> Result<Vec<MachineStatus>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_kind_as_str_and_display() {
        assert_eq!(RuntimeKind::Podman.as_str(), "podman");
        assert_eq!(RuntimeKind::Mock.as_str(), "mock");
        assert_eq!(RuntimeKind::Podman.to_string(), "podman");
        assert_eq!(RuntimeKind::Mock.to_string(), "mock");
    }
}
