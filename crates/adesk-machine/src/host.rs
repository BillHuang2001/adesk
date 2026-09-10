//! Host capabilities and the host control plane boundary.
//!
//! The host owns the machine boundary: which host resources a machine may reach
//! and whether a capability that crosses the boundary is approved
//! (`docs/machine.md` §4/§6). [`HostCapabilities`] describes what the host can
//! give a machine; [`HostControlPlane`] exposes the delegating lifecycle
//! operations plus approval routing, and never exposes an API for an operation
//! the AI can perform inside its own machine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::approval::{ApprovalDecision, ApprovalRequest, ApprovalRouter, AutoApprove};
use crate::error::Result;
use crate::manager::MachineManager;
use crate::runtime::ContainerRuntime;
use crate::spec::MachineSpec;
use crate::state::{MachineName, MachineStatus};

/// What the host can give a machine (`docs/machine.md` §4).
///
/// The [`Default`] grants nothing: no GPU passthrough (`gpu`), no KVM
/// (`kvm`), no mountable host paths (`allowed_mounts`) and no published ports
/// (`publish_ports`). A deployment opts in by enabling specific capabilities.
#[derive(Debug, Clone, Default)]
pub struct HostCapabilities {
    /// Whether GPU devices may be passed through to the machine.
    pub gpu: bool,
    /// Whether `/dev/kvm` may be exposed to the machine.
    pub kvm: bool,
    /// Host paths (and their subtrees) the machine may mount.
    pub allowed_mounts: Vec<PathBuf>,
    /// Whether ports may be published to the machine's viewer transport.
    pub publish_ports: bool,
}

impl HostCapabilities {
    /// Whether `path` is equal to or inside one of [`HostCapabilities::allowed_mounts`].
    ///
    /// Matching is component-wise, so allowing `/data` does **not** allow
    /// `/database`; a path must be the allowed root itself or a descendant of it.
    pub fn allows_mount(&self, path: &Path) -> bool {
        self.allowed_mounts
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }

    /// Whether a port may be published.
    ///
    /// Publishing is all-or-nothing per host: any port is allowed when
    /// [`HostCapabilities::publish_ports`] is set, and none otherwise.
    pub fn allows_port(&self, _port: u16) -> bool {
        self.publish_ports
    }
}

/// The boundary the host exposes to the outside world (`docs/machine.md` §4/§6).
///
/// Wraps a [`MachineManager`] with the host's [`HostCapabilities`] and an
/// [`ApprovalRouter`]: lifecycle calls delegate to the manager, and
/// boundary-crossing capability requests are routed through the installed
/// approver (defaulting to [`AutoApprove`]).
pub struct HostControlPlane<R: ContainerRuntime> {
    manager: MachineManager<R>,
    capabilities: HostCapabilities,
    router: ApprovalRouter,
}

impl<R: ContainerRuntime> HostControlPlane<R> {
    /// A control plane over `manager` granting `capabilities`.
    ///
    /// The default approval router wraps [`AutoApprove`]; install a different one
    /// with [`HostControlPlane::with_router`].
    pub fn new(manager: MachineManager<R>, capabilities: HostCapabilities) -> HostControlPlane<R> {
        HostControlPlane {
            manager,
            capabilities,
            router: ApprovalRouter::new(Arc::new(AutoApprove)),
        }
    }

    /// Installs a different [`ApprovalRouter`] (builder style).
    pub fn with_router(mut self, router: ApprovalRouter) -> HostControlPlane<R> {
        self.router = router;
        self
    }

    /// The capabilities this host grants.
    pub fn capabilities(&self) -> &HostCapabilities {
        &self.capabilities
    }

    /// The underlying lifecycle manager.
    pub fn manager(&self) -> &MachineManager<R> {
        &self.manager
    }

    /// Every machine the backend knows about (delegates to
    /// [`MachineManager::list`]).
    pub async fn machines(&self) -> Result<Vec<MachineStatus>> {
        self.manager.list().await
    }

    /// The status of the machine named `name` (delegates to
    /// [`MachineManager::status`]).
    pub async fn status(&self, name: &MachineName) -> Result<MachineStatus> {
        self.manager.status(name).await
    }

    /// Creates a machine from `spec` (delegates to [`MachineManager::create`]).
    pub async fn create(&self, spec: &MachineSpec) -> Result<MachineStatus> {
        self.manager.create(spec).await
    }

    /// Starts the machine named `name` (delegates to [`MachineManager::start`]).
    pub async fn start(&self, name: &MachineName) -> Result<MachineStatus> {
        self.manager.start(name).await
    }

    /// Stops the machine named `name` (delegates to [`MachineManager::stop`]).
    pub async fn stop(&self, name: &MachineName, timeout_ms: u64) -> Result<MachineStatus> {
        self.manager.stop(name, timeout_ms).await
    }

    /// Removes the machine named `name` (delegates to [`MachineManager::remove`]).
    pub async fn remove(&self, name: &MachineName, force: bool) -> Result<MachineStatus> {
        self.manager.remove(name, force).await
    }

    /// Routes an [`ApprovalRequest`] for a boundary-crossing capability through
    /// the installed [`ApprovalRouter`].
    ///
    /// A capability pre-approved by the router's policy returns
    /// [`ApprovalDecision::Allow`] without consulting the approver; everything
    /// else is handed to the approver.
    pub async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision {
        self.router.request(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalPolicy, AutoDeny};
    use crate::manager::testing::name;
    use crate::runtime::MockRuntime;

    fn plane() -> HostControlPlane<MockRuntime> {
        HostControlPlane::new(
            MachineManager::new(MockRuntime::new()),
            HostCapabilities::default(),
        )
    }

    #[tokio::test]
    async fn delegation_propagates_manager_errors() {
        let plane = plane();
        assert!(matches!(
            plane.status(&name("ghost")).await.unwrap_err(),
            crate::error::MachineError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn request_approval_honors_a_pre_approving_policy() {
        let router = ApprovalRouter::new(Arc::new(AutoDeny::new("no")))
            .with_policy(ApprovalPolicy::new().pre_approve("host.mount.allowed"));
        let plane = plane().with_router(router);

        // Pre-approved: allowed without reaching the denying approver.
        assert_eq!(
            plane
                .request_approval(ApprovalRequest::new(
                    3,
                    "adesk",
                    "host.mount.allowed",
                    "mounts an allowed path",
                ))
                .await,
            ApprovalDecision::Allow
        );
        // Anything else still reaches the approver.
        assert_eq!(
            plane
                .request_approval(ApprovalRequest::new(4, "adesk", "host.gpu", "wants a GPU"))
                .await,
            ApprovalDecision::Deny {
                reason: "no".into()
            }
        );
    }
}
