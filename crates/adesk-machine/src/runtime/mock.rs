//! In-memory deterministic container backend for tests and `--runtime mock`.
//!
//! `MockRuntime` is the reference implementation of the machine lifecycle
//! (`docs/machine.md` §2): it models the same state machine a real engine
//! enforces, hands out deterministic ids (`machine-1`, `machine-2`, …) and
//! records every call it received so a test can assert exactly what was asked of
//! the backend.
//!
//! # Lifecycle state machine
//!
//! ```text
//! (unknown) --create--> Created
//! Created | Stopped --start--> Running
//! Running --stop--> Stopped
//! Running --remove--> InvalidState unless force
//! Created | Stopped | Exited | Failed --remove--> (unknown)
//! ```
//!
//! A transition the current state forbids returns
//! [`MachineError::InvalidState`]; unknown ids return [`MachineError::NotFound`]
//! and a duplicate `create` returns [`MachineError::Duplicate`].
//!
//! `created_at_ms` comes from a clock ([`MockRuntime::with_clock`]); the default
//! clock is a monotonic source measured from the moment the runtime was created,
//! so tests that pass a counter get reproducible timestamps.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use crate::error::{MachineError, Result};
use crate::runtime::{ContainerRuntime, RuntimeKind};
use crate::spec::MachineSpec;
use crate::state::{MachineId, MachineName, MachineState, MachineStatus};

/// One recorded backend call, in the order the backend received it.
///
/// The variants carry the salient arguments of each [`ContainerRuntime`] method
/// so a test can assert exactly what the mock was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCall {
    /// [`ContainerRuntime::create`] was called for `name`.
    Create {
        /// The requested machine name.
        name: MachineName,
    },
    /// [`ContainerRuntime::start`] was called for `id`.
    Start {
        /// The machine that was started.
        id: MachineId,
    },
    /// [`ContainerRuntime::stop`] was called for `id`.
    Stop {
        /// The machine that was stopped.
        id: MachineId,
        /// The graceful-shutdown timeout in milliseconds.
        timeout_ms: u64,
    },
    /// [`ContainerRuntime::remove`] was called for `id`.
    Remove {
        /// The machine that was removed.
        id: MachineId,
        /// Whether the container was force-killed.
        force: bool,
    },
    /// [`ContainerRuntime::status`] was called for `id`.
    Status {
        /// The machine that was queried.
        id: MachineId,
    },
    /// [`ContainerRuntime::list`] was called.
    List,
}

/// In-memory, deterministic [`ContainerRuntime`].
///
/// Interior state lives behind a [`Mutex`], so a single `MockRuntime` can be
/// shared across tokio tasks. See the module docs for the state machine it
/// enforces.
pub struct MockRuntime {
    inner: Mutex<MockInner>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// The mutable interior of a [`MockRuntime`].
#[derive(Debug, Default)]
struct MockInner {
    machines: BTreeMap<MachineId, MockMachine>,
    names: BTreeMap<MachineName, MachineId>,
    next_seq: u32,
    calls: Vec<RuntimeCall>,
}

/// One machine the mock knows about.
#[derive(Debug)]
struct MockMachine {
    spec: MachineSpec,
    state: MachineState,
    created_at_ms: u64,
    /// Deterministic fake pid reported while the machine is running.
    pid: u32,
}

impl MockMachine {
    /// Builds the backend's view of this machine.
    fn status(&self, id: &MachineId) -> MachineStatus {
        MachineStatus {
            id: id.clone(),
            name: self.spec.name.clone(),
            image: self.spec.image.clone(),
            state: self.state.clone(),
            pid: self.state.is_running().then_some(self.pid),
            created_at_ms: self.created_at_ms,
            viewer: self.spec.viewer.clone(),
        }
    }
}

impl MockRuntime {
    /// A mock backend whose default clock is a monotonic source measured from
    /// the moment the runtime was created.
    pub fn new() -> MockRuntime {
        let start = std::time::Instant::now();
        MockRuntime::with_clock(Arc::new(move || start.elapsed().as_millis() as u64))
    }

    /// A mock backend that reads `created_at_ms` from `clock`.
    ///
    /// Pass a deterministic counter (for example an `AtomicU64`) so creation
    /// timestamps are reproducible in tests.
    pub fn with_clock(clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> MockRuntime {
        MockRuntime {
            inner: Mutex::new(MockInner::default()),
            clock,
        }
    }

    /// Every call the backend has received, in order.
    pub fn calls(&self) -> Vec<RuntimeCall> {
        self.lock().calls.clone()
    }

    /// Locks the interior, recovering from a poisoned lock so a backend never
    /// panics on an ordinary call path.
    fn lock(&self) -> MutexGuard<'_, MockInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for MockRuntime {
    fn default() -> MockRuntime {
        MockRuntime::new()
    }
}

#[async_trait]
impl ContainerRuntime for MockRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Mock
    }

    async fn create(&self, spec: &MachineSpec) -> Result<MachineId> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::Create {
            name: spec.name.clone(),
        });
        if inner.names.contains_key(&spec.name) {
            return Err(MachineError::Duplicate {
                name: spec.name.to_string(),
            });
        }

        inner.next_seq += 1;
        let seq = inner.next_seq;
        let id = MachineId::from(format!("machine-{seq}"));
        let machine = MockMachine {
            spec: spec.clone(),
            state: MachineState::Created,
            created_at_ms: (self.clock)(),
            pid: 1000 + seq,
        };
        inner.machines.insert(id.clone(), machine);
        inner.names.insert(spec.name.clone(), id.clone());
        Ok(id)
    }

    async fn start(&self, id: &MachineId) -> Result<()> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::Start { id: id.clone() });
        let machine = inner.machines.get_mut(id).ok_or_else(|| not_found(id))?;
        if matches!(machine.state, MachineState::Created | MachineState::Stopped) {
            machine.state = MachineState::Running;
            Ok(())
        } else {
            Err(invalid_state(&machine.spec.name, &machine.state, "start"))
        }
    }

    async fn stop(&self, id: &MachineId, timeout_ms: u64) -> Result<()> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::Stop {
            id: id.clone(),
            timeout_ms,
        });
        let machine = inner.machines.get_mut(id).ok_or_else(|| not_found(id))?;
        if matches!(machine.state, MachineState::Running) {
            machine.state = MachineState::Stopped;
            Ok(())
        } else {
            Err(invalid_state(&machine.spec.name, &machine.state, "stop"))
        }
    }

    async fn remove(&self, id: &MachineId, force: bool) -> Result<()> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::Remove {
            id: id.clone(),
            force,
        });
        let machine = inner.machines.get(id).ok_or_else(|| not_found(id))?;
        if machine.state.is_running() && !force {
            return Err(invalid_state(&machine.spec.name, &machine.state, "remove"));
        }
        if let Some(removed) = inner.machines.remove(id) {
            inner.names.remove(&removed.spec.name);
        }
        Ok(())
    }

    async fn status(&self, id: &MachineId) -> Result<MachineStatus> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::Status { id: id.clone() });
        let machine = inner.machines.get(id).ok_or_else(|| not_found(id))?;
        Ok(machine.status(id))
    }

    async fn list(&self) -> Result<Vec<MachineStatus>> {
        let mut inner = self.lock();
        inner.calls.push(RuntimeCall::List);
        Ok(inner
            .machines
            .iter()
            .map(|(id, machine)| machine.status(id))
            .collect())
    }
}

/// The error for an id the mock does not know; only the id is available, so it
/// is reported as the unknown name.
fn not_found(id: &MachineId) -> MachineError {
    MachineError::NotFound {
        name: id.to_string(),
    }
}

/// The error for a lifecycle action the machine's current state forbids.
fn invalid_state(name: &MachineName, state: &MachineState, action: &str) -> MachineError {
    MachineError::InvalidState {
        name: name.to_string(),
        state: state.as_str().to_owned(),
        action: action.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ViewerExposure;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A minimal spec whose viewer is not exposed (so status assertions are
    /// unambiguous).
    fn spec(name: &str) -> MachineSpec {
        MachineSpec::new(name, "image:1").with_viewer(ViewerExposure::None)
    }

    #[tokio::test]
    async fn kind_is_mock() {
        assert_eq!(MockRuntime::new().kind(), RuntimeKind::Mock);
        assert_eq!(MockRuntime::default().kind(), RuntimeKind::Mock);
    }

    #[tokio::test]
    async fn create_assigns_sequential_ids_and_initial_state() {
        let runtime = MockRuntime::new();
        let first = runtime.create(&spec("alpha")).await.unwrap();
        let second = runtime.create(&spec("bravo")).await.unwrap();
        assert_eq!(first, MachineId::from("machine-1"));
        assert_eq!(second, MachineId::from("machine-2"));

        let status = runtime.status(&first).await.unwrap();
        assert_eq!(status.id, first);
        assert_eq!(status.name, MachineName::from("alpha"));
        assert_eq!(status.image, "image:1");
        assert_eq!(status.state, MachineState::Created);
        assert_eq!(status.pid, None);
        assert_eq!(status.viewer, ViewerExposure::None);
    }

    #[tokio::test]
    async fn lifecycle_create_start_stop_start_remove() {
        let runtime = MockRuntime::new();
        let id = runtime.create(&spec("adesk")).await.unwrap();

        runtime.start(&id).await.unwrap();
        let running = runtime.status(&id).await.unwrap();
        assert_eq!(running.state, MachineState::Running);
        assert_eq!(running.pid, Some(1001));

        runtime.stop(&id, 5_000).await.unwrap();
        let stopped = runtime.status(&id).await.unwrap();
        assert_eq!(stopped.state, MachineState::Stopped);
        assert_eq!(stopped.pid, None);

        runtime.start(&id).await.unwrap();
        assert_eq!(
            runtime.status(&id).await.unwrap().state,
            MachineState::Running
        );

        runtime.remove(&id, true).await.unwrap();
        assert!(matches!(
            runtime.status(&id).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn illegal_transitions_are_reported() {
        let runtime = MockRuntime::new();
        let id = runtime.create(&spec("adesk")).await.unwrap();

        runtime.start(&id).await.unwrap();
        let err = runtime.start(&id).await.unwrap_err();
        assert!(
            matches!(err, MachineError::InvalidState { ref state, .. } if state == "running"),
            "unexpected error: {err:?}"
        );

        let other = runtime.create(&spec("other")).await.unwrap();
        let err = runtime.stop(&other, 0).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "machine `other` in state `created` cannot stop"
        );
    }

    #[tokio::test]
    async fn remove_running_requires_force() {
        let runtime = MockRuntime::new();
        let id = runtime.create(&spec("adesk")).await.unwrap();
        runtime.start(&id).await.unwrap();

        let err = runtime.remove(&id, false).await.unwrap_err();
        assert!(
            matches!(err, MachineError::InvalidState { ref action, .. } if action == "remove"),
            "unexpected error: {err:?}"
        );
        // The machine survives the rejected removal.
        assert!(runtime.status(&id).await.is_ok());

        runtime.remove(&id, true).await.unwrap();
        assert!(runtime.status(&id).await.is_err());
    }

    #[tokio::test]
    async fn duplicate_name_is_rejected() {
        let runtime = MockRuntime::new();
        runtime.create(&spec("adesk")).await.unwrap();
        let err = runtime.create(&spec("adesk")).await.unwrap_err();
        assert_eq!(err.to_string(), "machine already exists: adesk");
        assert_eq!(runtime.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_ids_are_not_found() {
        let runtime = MockRuntime::new();
        let ghost = MachineId::from("machine-99");
        let errors = [
            runtime.start(&ghost).await.unwrap_err(),
            runtime.stop(&ghost, 0).await.unwrap_err(),
            runtime.remove(&ghost, true).await.unwrap_err(),
            runtime.status(&ghost).await.unwrap_err(),
        ];
        for err in errors {
            assert_eq!(err.to_string(), "machine not found: machine-99");
        }
    }

    #[tokio::test]
    async fn list_reports_every_machine() {
        let runtime = MockRuntime::new();
        let alpha = runtime.create(&spec("alpha")).await.unwrap();
        runtime.create(&spec("bravo")).await.unwrap();
        runtime.start(&alpha).await.unwrap();

        let names: Vec<MachineName> = runtime
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|status| status.name)
            .collect();
        assert_eq!(
            names,
            vec![MachineName::from("alpha"), MachineName::from("bravo")]
        );
    }

    #[tokio::test]
    async fn every_call_is_recorded_in_order() {
        let runtime = MockRuntime::new();
        let id = runtime.create(&spec("adesk")).await.unwrap();
        runtime.start(&id).await.unwrap();
        runtime.status(&id).await.unwrap();
        runtime.stop(&id, 2_500).await.unwrap();
        runtime.list().await.unwrap();
        runtime.remove(&id, true).await.unwrap();

        assert_eq!(
            runtime.calls(),
            vec![
                RuntimeCall::Create {
                    name: MachineName::from("adesk"),
                },
                RuntimeCall::Start { id: id.clone() },
                RuntimeCall::Status { id: id.clone() },
                RuntimeCall::Stop {
                    id: id.clone(),
                    timeout_ms: 2_500,
                },
                RuntimeCall::List,
                RuntimeCall::Remove {
                    id: id.clone(),
                    force: true,
                },
            ]
        );
    }

    #[tokio::test]
    async fn custom_clock_sets_created_at_ms() {
        let counter = Arc::new(AtomicU64::new(0));
        let ticks = Arc::clone(&counter);
        let runtime =
            MockRuntime::with_clock(Arc::new(move || ticks.fetch_add(1, Ordering::SeqCst) + 10));

        let first = runtime.create(&spec("alpha")).await.unwrap();
        let second = runtime.create(&spec("bravo")).await.unwrap();
        assert_eq!(runtime.status(&first).await.unwrap().created_at_ms, 10);
        assert_eq!(runtime.status(&second).await.unwrap().created_at_ms, 11);
    }
}
