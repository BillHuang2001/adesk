//! Host control-plane boundary tests: approval routing, host capability checks
//! and lifecycle delegation over the in-memory backend.
//!
//! Actions fully contained in a machine belong to the AI; actions that cross the
//! boundary are mediated here. These tests cover the [`ApprovalRouter`]/
//! [`Approver`] seam, the host-side [`ApprovalPolicy`], [`HostCapabilities`] and
//! the [`HostControlPlane`] that ties them to a [`MachineManager`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use adesk_machine::{
    ApprovalDecision, ApprovalPolicy, ApprovalRequest, ApprovalRouter, Approver, AutoApprove,
    AutoDeny, HostCapabilities, HostControlPlane, MachineManager, MachineName, MachineSpec,
    MachineState, MockRuntime,
};

/// An approver that records every request it sees and denies it, so a test can
/// prove whether the approver was consulted at all.
#[derive(Default)]
struct RecordingDeny {
    seen: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Approver for RecordingDeny {
    async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
        self.seen.lock().expect("lock").push(req.capability.clone());
        ApprovalDecision::Deny {
            reason: "recorded".to_owned(),
        }
    }
}

fn spec(name: &str) -> MachineSpec {
    MachineSpec::new(name, "ghcr.io/adesk/machine:latest")
}

fn name(value: &str) -> MachineName {
    MachineName::from(value)
}

fn request(capability: &str) -> ApprovalRequest {
    ApprovalRequest::new(1, "adesk", capability, "wants a boundary capability")
}

fn plane(capabilities: HostCapabilities) -> HostControlPlane<MockRuntime> {
    HostControlPlane::new(MachineManager::new(MockRuntime::new()), capabilities)
}

#[tokio::test]
async fn auto_approvers_decide() {
    let allow = ApprovalRouter::new(Arc::new(AutoApprove));
    assert_eq!(
        allow.request(request("host.gpu")).await,
        ApprovalDecision::Allow
    );

    let deny = ApprovalRouter::new(Arc::new(AutoDeny::new("no")));
    assert_eq!(
        deny.request(request("host.kvm")).await,
        ApprovalDecision::Deny {
            reason: "no".to_owned(),
        }
    );
}

#[tokio::test]
async fn policy_pre_approval_short_circuits_the_approver() {
    let approver = Arc::new(RecordingDeny::default());
    let router = ApprovalRouter::new(approver.clone())
        .with_policy(ApprovalPolicy::new().pre_approve("host.mount.allowed"));

    // Pre-approved: allowed without the approver ever being consulted.
    assert_eq!(
        router.request(request("host.mount.allowed")).await,
        ApprovalDecision::Allow
    );
    assert!(approver.seen.lock().expect("lock").is_empty());

    // Anything else reaches the approver, which records and denies the request.
    assert_eq!(
        router.request(request("host.gpu")).await,
        ApprovalDecision::Deny {
            reason: "recorded".to_owned(),
        }
    );
    let seen = approver.seen.lock().expect("lock").clone();
    assert_eq!(seen, ["host.gpu"]);
}

#[test]
fn policy_reports_pre_approved_capabilities() {
    let policy = ApprovalPolicy::new().pre_approve("host.gpu");
    assert!(policy.is_pre_approved("host.gpu"));
    assert!(!policy.is_pre_approved("host.kvm"));
}

#[test]
fn default_capabilities_grant_nothing() {
    let caps = HostCapabilities::default();
    assert!(!caps.gpu);
    assert!(!caps.kvm);
    assert!(!caps.publish_ports);
    assert!(caps.allowed_mounts.is_empty());
    assert!(!caps.allows_mount(Path::new("/data")));
    assert!(!caps.allows_mount(Path::new("/")));
    assert!(!caps.allows_port(8080));
}

#[test]
fn allowed_mounts_match_component_wise_and_ports_are_all_or_nothing() {
    let caps = HostCapabilities {
        allowed_mounts: vec![PathBuf::from("/data"), PathBuf::from("/srv/share")],
        publish_ports: true,
        ..HostCapabilities::default()
    };

    // The allowed root and its descendants are permitted...
    assert!(caps.allows_mount(Path::new("/data")));
    assert!(caps.allows_mount(Path::new("/data/nested/file")));
    assert!(caps.allows_mount(Path::new("/srv/share")));

    // ...but a shared string prefix is not a component prefix: allowing `/data`
    // must not allow `/database`.
    assert!(!caps.allows_mount(Path::new("/database")));
    assert!(!caps.allows_mount(Path::new("/srv/shared")));
    assert!(!caps.allows_mount(Path::new("/other")));

    // Publishing is all-or-nothing per host.
    assert!(caps.allows_port(1));
    assert!(caps.allows_port(65_535));
}

#[tokio::test]
async fn control_plane_defaults_to_auto_approve() {
    let plane = plane(HostCapabilities::default());
    assert_eq!(
        plane.request_approval(request("host.gpu")).await,
        ApprovalDecision::Allow
    );
}

#[tokio::test]
async fn control_plane_delegates_lifecycle_and_routes_approvals() {
    let caps = HostCapabilities {
        gpu: true,
        kvm: true,
        ..HostCapabilities::default()
    };
    let plane = plane(caps).with_router(ApprovalRouter::new(Arc::new(AutoDeny::new("no"))));

    // Capabilities are exposed as configured.
    assert!(plane.capabilities().gpu);
    assert!(plane.capabilities().kvm);

    // Lifecycle calls delegate to the underlying manager.
    assert_eq!(
        plane.create(&spec("adesk")).await.unwrap().state,
        MachineState::Created
    );
    assert_eq!(
        plane.status(&name("adesk")).await.unwrap().state,
        MachineState::Created
    );
    assert_eq!(
        plane.start(&name("adesk")).await.unwrap().state,
        MachineState::Running
    );
    assert_eq!(plane.machines().await.unwrap().len(), 1);
    assert_eq!(
        plane.stop(&name("adesk"), 0).await.unwrap().state,
        MachineState::Stopped
    );
    assert_eq!(
        plane.remove(&name("adesk"), false).await.unwrap().state,
        MachineState::Stopped
    );
    assert!(plane.machines().await.unwrap().is_empty());

    // Approval requests are routed through the installed (denying) router.
    assert_eq!(
        plane.request_approval(request("host.gpu")).await,
        ApprovalDecision::Deny {
            reason: "no".to_owned(),
        }
    );
}
