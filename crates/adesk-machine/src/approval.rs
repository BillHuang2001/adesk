//! Boundary-crossing capability approvals.
//!
//! Actions fully contained in the machine belong to the AI; actions that cross
//! the boundary may need mediation (`docs/machine.md` §6). `ApprovalRouter`
//! routes an [`ApprovalRequest`] to an [`Approver`] and awaits a
//! [`ApprovalDecision`]; `AutoApprove`/`AutoDeny` are the built-in approvers for
//! tests and simple deployments, and a host-side [`ApprovalPolicy`] can
//! pre-approve capabilities without asking at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::MachineName;

/// A capability request that crosses the machine boundary.
///
/// `capability` is a short stable key (for example `"host.gpu"` or
/// `"host.port.publish"`); `description` is the human-readable explanation shown
/// to whoever answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Caller-assigned request id, unique within the host control plane.
    pub id: u64,
    /// The machine requesting the capability.
    pub machine: MachineName,
    /// The requested capability key.
    pub capability: String,
    /// Human-readable description of the request.
    pub description: String,
}

impl ApprovalRequest {
    /// A request from `machine` for `capability`, described by `description`.
    pub fn new(
        id: u64,
        machine: impl Into<MachineName>,
        capability: impl Into<String>,
        description: impl Into<String>,
    ) -> ApprovalRequest {
        ApprovalRequest {
            id,
            machine: machine.into(),
            capability: capability.into(),
            description: description.into(),
        }
    }
}

/// The host's answer to an [`ApprovalRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum ApprovalDecision {
    /// The capability is granted.
    Allow,
    /// The capability is refused, with a reason.
    Deny {
        /// Why the request was refused.
        reason: String,
    },
}

impl ApprovalDecision {
    /// Whether the decision grants the capability.
    pub fn is_allowed(&self) -> bool {
        matches!(self, ApprovalDecision::Allow)
    }
}

/// Decides whether a boundary-crossing request is allowed.
///
/// Object-safe: a router holds an `Arc<dyn Approver>`. Implementations must be
/// `Send + Sync` because decisions are awaited inside tokio tasks.
#[async_trait]
pub trait Approver: Send + Sync {
    /// Decide `req`.
    async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision;
}

/// Host-side pre-authorization applied before a request reaches an [`Approver`].
///
/// Some capabilities are granted up front by the host (for example a mount path
/// already on the allow-list); those never reach the approver. Everything else
/// is routed normally. The default policy pre-approves nothing.
#[derive(Debug, Clone, Default)]
pub struct ApprovalPolicy {
    pre_approved: BTreeSet<String>,
}

impl ApprovalPolicy {
    /// A policy that pre-approves nothing: every request reaches the approver.
    pub fn new() -> ApprovalPolicy {
        ApprovalPolicy::default()
    }

    /// Pre-approves `capability` without asking the approver.
    pub fn pre_approve(mut self, capability: impl Into<String>) -> ApprovalPolicy {
        self.pre_approved.insert(capability.into());
        self
    }

    /// Whether `capability` is pre-approved by this policy.
    pub fn is_pre_approved(&self, capability: &str) -> bool {
        self.pre_approved.contains(capability)
    }
}

/// Routes approval requests to an [`Approver`], honoring an [`ApprovalPolicy`].
pub struct ApprovalRouter {
    approver: Arc<dyn Approver>,
    policy: ApprovalPolicy,
}

impl ApprovalRouter {
    /// A router that sends every request to `approver` with the default policy.
    pub fn new(approver: Arc<dyn Approver>) -> ApprovalRouter {
        ApprovalRouter {
            approver,
            policy: ApprovalPolicy::default(),
        }
    }

    /// Installs a host-side [`ApprovalPolicy`].
    pub fn with_policy(mut self, policy: ApprovalPolicy) -> ApprovalRouter {
        self.policy = policy;
        self
    }

    /// The installed policy.
    pub fn policy(&self) -> &ApprovalPolicy {
        &self.policy
    }

    /// Decides `req`.
    ///
    /// A pre-approved capability returns [`ApprovalDecision::Allow`] without
    /// consulting the approver; everything else is delegated.
    pub async fn request(&self, req: ApprovalRequest) -> ApprovalDecision {
        if self.policy.is_pre_approved(&req.capability) {
            return ApprovalDecision::Allow;
        }
        self.approver.decide(&req).await
    }
}

/// An [`Approver`] that allows every request.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoApprove;

#[async_trait]
impl Approver for AutoApprove {
    async fn decide(&self, _req: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

/// An [`Approver`] that denies every request with a fixed reason.
#[derive(Debug, Clone)]
pub struct AutoDeny {
    reason: String,
}

impl AutoDeny {
    /// Denies every request with `reason`.
    pub fn new(reason: impl Into<String>) -> AutoDeny {
        AutoDeny {
            reason: reason.into(),
        }
    }

    /// The reason this approver reports.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl Default for AutoDeny {
    fn default() -> AutoDeny {
        AutoDeny::new("denied by policy")
    }
}

#[async_trait]
impl Approver for AutoDeny {
    async fn decide(&self, _req: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny {
            reason: self.reason.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn request(capability: &str) -> ApprovalRequest {
        ApprovalRequest::new(1, "adesk", capability, "wants a thing")
    }

    #[tokio::test]
    async fn new_builds_a_request() {
        let req = request("host.gpu");
        assert_eq!(req.id, 1);
        assert_eq!(req.machine, MachineName::from("adesk"));
        assert_eq!(req.capability, "host.gpu");
        assert_eq!(req.description, "wants a thing");
    }

    #[tokio::test]
    async fn auto_approve_allows() {
        let router = ApprovalRouter::new(Arc::new(AutoApprove));
        let decision = router.request(request("host.gpu")).await;
        assert_eq!(decision, ApprovalDecision::Allow);
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn auto_deny_refuses_with_default_reason() {
        let router = ApprovalRouter::new(Arc::new(AutoDeny::default()));
        assert_eq!(
            router.request(request("host.gpu")).await,
            ApprovalDecision::Deny {
                reason: "denied by policy".into(),
            }
        );

        let router = ApprovalRouter::new(Arc::new(AutoDeny::new("nope")));
        assert_eq!(
            router.request(request("host.gpu")).await,
            ApprovalDecision::Deny {
                reason: "nope".into(),
            }
        );
    }

    #[tokio::test]
    async fn policy_pre_approves_without_asking() {
        // The approver would deny, but the policy short-circuits.
        let router = ApprovalRouter::new(Arc::new(AutoDeny::new("nope")))
            .with_policy(ApprovalPolicy::new().pre_approve("host.mount.allowed"));

        assert!(router.policy().is_pre_approved("host.mount.allowed"));
        assert_eq!(
            router.request(request("host.mount.allowed")).await,
            ApprovalDecision::Allow
        );
        // Other capabilities still reach the denying approver.
        assert_eq!(
            router.request(request("host.gpu")).await,
            ApprovalDecision::Deny {
                reason: "nope".into(),
            }
        );
    }

    /// Records the requests it saw so the test can assert delegation happened.
    struct Recording {
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Approver for Recording {
        async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
            self.seen.lock().unwrap().push(req.capability.clone());
            ApprovalDecision::Allow
        }
    }

    #[tokio::test]
    async fn router_delegates_to_custom_approver() {
        let approver = Arc::new(Recording {
            seen: Mutex::new(Vec::new()),
        });
        let router = ApprovalRouter::new(approver.clone());
        router.request(request("host.kvm")).await;
        assert_eq!(approver.seen.lock().unwrap().as_slice(), ["host.kvm"]);
    }

    #[test]
    fn decisions_round_trip_through_serde() {
        for decision in [
            ApprovalDecision::Allow,
            ApprovalDecision::Deny {
                reason: "no".into(),
            },
        ] {
            let json = serde_json::to_string(&decision).unwrap();
            let back: ApprovalDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(back, decision);
        }
    }
}
