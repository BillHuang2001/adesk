//! Machine lifecycle manager over a `ContainerRuntime`.
//!
//! [`MachineManager`] owns the machine lifecycle: it turns a [`MachineSpec`]
//! into a machine and drives it through the state machine, keeping a
//! [`MachineRegistry`] cache of the backend's truth (`docs/machine.md` §3/§4).
//! Names are the manager's unique key, so a duplicate `create` fails rather than
//! silently replacing a machine. The manager never exposes an API for an
//! operation the AI can perform inside its own machine.

use std::sync::{Mutex, MutexGuard};

use crate::error::{MachineError, Result};
use crate::registry::MachineRegistry;
use crate::runtime::ContainerRuntime;
use crate::spec::MachineSpec;
use crate::state::{MachineId, MachineName, MachineStatus};

/// How long [`MachineManager::restart`] waits for a clean shutdown.
///
/// `restart` has no caller-supplied timeout, so it stops the machine with this
/// default; call [`MachineManager::stop`] directly when a specific timeout is
/// needed.
const RESTART_STOP_TIMEOUT_MS: u64 = 10_000;

/// Owns the machine lifecycle over a [`ContainerRuntime`].
///
/// The runtime is the source of truth; the manager caches the last known
/// [`MachineStatus`] per machine in a [`MachineRegistry`] for name lookups and
/// fast status queries, refreshing it after every lifecycle transition. The
/// cache is rebuilt from [`MachineManager::list`], so it can always be recovered
/// from the backend.
pub struct MachineManager<R: ContainerRuntime> {
    runtime: R,
    registry: Mutex<MachineRegistry>,
}

impl<R: ContainerRuntime> MachineManager<R> {
    /// A manager driving `runtime`, with an empty registry.
    pub fn new(runtime: R) -> MachineManager<R> {
        MachineManager {
            runtime,
            registry: Mutex::new(MachineRegistry::new()),
        }
    }

    /// The backend this manager drives.
    pub fn runtime(&self) -> &R {
        &self.runtime
    }

    /// Creates a machine from `spec` and returns its freshly read status.
    ///
    /// Names are unique within a manager: when a machine named `spec.name` is
    /// already registered — or the backend itself reports the name is taken —
    /// this fails with [`MachineError::Duplicate`] and the existing machine is
    /// left untouched. On success the new machine is inserted into the registry.
    pub async fn create(&self, spec: &MachineSpec) -> Result<MachineStatus> {
        if self.lock_registry().contains(&spec.name) {
            return Err(MachineError::Duplicate {
                name: spec.name.to_string(),
            });
        }

        let id = match self.runtime.create(spec).await {
            Ok(id) => id,
            // A backend that already knows the name (for example a second manager
            // sharing the runtime) is reported exactly like a local clash.
            Err(MachineError::Duplicate { .. }) => {
                return Err(MachineError::Duplicate {
                    name: spec.name.to_string(),
                });
            }
            Err(other) => return Err(other),
        };

        let status = self.runtime.status(&id).await?;
        self.lock_registry().insert(status.clone());
        tracing::info!(
            machine = %status.name,
            id = %status.id,
            state = status.state.as_str(),
            "machine created"
        );
        Ok(status)
    }

    /// Starts the machine named `name`, refreshing the cached status.
    ///
    /// Unknown names fail with [`MachineError::NotFound`].
    pub async fn start(&self, name: &MachineName) -> Result<MachineStatus> {
        let id = self.resolve(name)?;
        self.runtime.start(&id).await?;
        let status = self.runtime.status(&id).await?;
        self.lock_registry().update(status.clone());
        tracing::info!(
            machine = %status.name,
            id = %status.id,
            state = status.state.as_str(),
            "machine started"
        );
        Ok(status)
    }

    /// Stops the machine named `name`, waiting at most `timeout_ms` for a clean
    /// shutdown, and refreshes the cached status.
    ///
    /// Unknown names fail with [`MachineError::NotFound`].
    pub async fn stop(&self, name: &MachineName, timeout_ms: u64) -> Result<MachineStatus> {
        let id = self.resolve(name)?;
        self.runtime.stop(&id, timeout_ms).await?;
        let status = self.runtime.status(&id).await?;
        self.lock_registry().update(status.clone());
        tracing::info!(
            machine = %status.name,
            id = %status.id,
            state = status.state.as_str(),
            "machine stopped"
        );
        Ok(status)
    }

    /// Restarts the machine named `name`: stops it (with the default
    /// `RESTART_STOP_TIMEOUT_MS` timeout) and then starts it again, returning the
    /// status observed after the restart.
    ///
    /// Unknown names fail with [`MachineError::NotFound`].
    pub async fn restart(&self, name: &MachineName) -> Result<MachineStatus> {
        self.stop(name, RESTART_STOP_TIMEOUT_MS).await?;
        self.start(name).await
    }

    /// Removes the machine named `name`, forcing removal of a running container
    /// when `force` is set, and forgets it in the registry.
    ///
    /// Because the machine no longer exists to be queried afterwards, the status
    /// returned is the one observed immediately *before* removal. Unknown names
    /// fail with [`MachineError::NotFound`].
    pub async fn remove(&self, name: &MachineName, force: bool) -> Result<MachineStatus> {
        let id = self.resolve(name)?;
        let last = self.runtime.status(&id).await?;
        self.runtime.remove(&id, force).await?;
        self.lock_registry().remove(name);
        tracing::info!(
            machine = %last.name,
            id = %last.id,
            force,
            "machine removed"
        );
        Ok(last)
    }

    /// Returns the current status of the machine named `name`, refreshing the
    /// cache.
    ///
    /// Unknown names fail with [`MachineError::NotFound`].
    pub async fn status(&self, name: &MachineName) -> Result<MachineStatus> {
        let id = self.resolve(name)?;
        let status = self.runtime.status(&id).await?;
        self.lock_registry().update(status.clone());
        Ok(status)
    }

    /// Lists every machine the backend knows about.
    ///
    /// The backend is the source of truth, so its list is returned and the
    /// registry is rebuilt from it (machines the backend no longer reports are
    /// dropped from the cache).
    pub async fn list(&self) -> Result<Vec<MachineStatus>> {
        let statuses = self.runtime.list().await?;
        let mut registry = self.lock_registry();
        *registry = MachineRegistry::new();
        for status in &statuses {
            registry.insert(status.clone());
        }
        Ok(statuses)
    }

    /// The backend id for `name`, or [`MachineError::NotFound`] if unknown.
    fn resolve(&self, name: &MachineName) -> Result<MachineId> {
        self.lock_registry()
            .id(name)
            .cloned()
            .ok_or_else(|| MachineError::NotFound {
                name: name.to_string(),
            })
    }

    /// Locks the registry, recovering from a poisoned mutex.
    ///
    /// A poisoned lock can only result from a panic while the registry was held
    /// (which none of the paths above do), so the data is still consistent and
    /// is recovered rather than propagating a panic.
    fn lock_registry(&self) -> MutexGuard<'_, MachineRegistry> {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Minimal in-memory [`ContainerRuntime`] shared by the manager's and the host
/// control plane's unit tests.
///
/// The production `MockRuntime` is implemented separately and is not available
/// here, so these tests drive a local double that models the lifecycle state
/// machine (created → running → stopped) and hands out ids `machine-1`,
/// `machine-2`, … in creation order. The full lifecycle integration suite is
/// owned by a later task.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard};

    use async_trait::async_trait;

    use crate::error::{MachineError, Result};
    use crate::runtime::{ContainerRuntime, RuntimeKind};
    use crate::spec::MachineSpec;
    use crate::state::{MachineId, MachineName, MachineState, MachineStatus};

    #[derive(Default)]
    struct Inner {
        next: u64,
        machines: BTreeMap<MachineId, MachineStatus>,
    }

    impl Inner {
        fn create(&mut self, spec: &MachineSpec) -> MachineId {
            self.next += 1;
            let id = MachineId::from(format!("machine-{}", self.next));
            let status = MachineStatus {
                id: id.clone(),
                name: spec.name.clone(),
                image: spec.image.clone(),
                state: MachineState::Created,
                pid: None,
                created_at_ms: 1_000 + self.next,
                viewer: spec.viewer.clone(),
            };
            self.machines.insert(id.clone(), status);
            id
        }
    }

    /// A minimal in-memory [`ContainerRuntime`] for the manager/host unit tests.
    #[derive(Default)]
    pub(crate) struct StubRuntime {
        inner: Mutex<Inner>,
    }

    impl StubRuntime {
        /// An empty runtime.
        pub(crate) fn new() -> StubRuntime {
            StubRuntime::default()
        }

        /// Registers a `Created` machine for `spec` directly, as if `create` had
        /// been called, but without going through a manager's registry — used to
        /// make the backend (not the cache) report a duplicate name.
        pub(crate) fn seed(&self, spec: &MachineSpec) -> MachineId {
            self.lock().create(spec)
        }

        fn lock(&self) -> MutexGuard<'_, Inner> {
            self.inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    #[async_trait]
    impl ContainerRuntime for StubRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Mock
        }

        async fn create(&self, spec: &MachineSpec) -> Result<MachineId> {
            let mut inner = self.lock();
            if inner
                .machines
                .values()
                .any(|status| status.name == spec.name)
            {
                return Err(MachineError::Duplicate {
                    name: spec.name.to_string(),
                });
            }
            Ok(inner.create(spec))
        }

        async fn start(&self, id: &MachineId) -> Result<()> {
            let mut inner = self.lock();
            let status = inner.machines.get_mut(id).ok_or_else(|| not_found(id))?;
            status.state = MachineState::Running;
            status.pid = Some(2_000);
            Ok(())
        }

        async fn stop(&self, id: &MachineId, _timeout_ms: u64) -> Result<()> {
            let mut inner = self.lock();
            let status = inner.machines.get_mut(id).ok_or_else(|| not_found(id))?;
            status.state = MachineState::Stopped;
            status.pid = None;
            Ok(())
        }

        async fn remove(&self, id: &MachineId, _force: bool) -> Result<()> {
            self.lock().machines.remove(id);
            Ok(())
        }

        async fn status(&self, id: &MachineId) -> Result<MachineStatus> {
            self.lock()
                .machines
                .get(id)
                .cloned()
                .ok_or_else(|| not_found(id))
        }

        async fn list(&self) -> Result<Vec<MachineStatus>> {
            Ok(self.lock().machines.values().cloned().collect())
        }
    }

    fn not_found(id: &MachineId) -> MachineError {
        MachineError::NotFound {
            name: id.to_string(),
        }
    }

    /// The name a [`StubRuntime`] will assign first, handy for assertions.
    pub(crate) fn first_id() -> MachineId {
        MachineId::from("machine-1")
    }

    /// The [`MachineName`] helper used across the manager/host tests.
    pub(crate) fn name(value: &str) -> MachineName {
        MachineName::from(value)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{first_id, name, StubRuntime};
    use super::*;
    use crate::spec::ViewerExposure;
    use crate::state::MachineState;

    fn spec(value: &str) -> MachineSpec {
        MachineSpec::new(value, "ghcr.io/adesk/machine:latest")
    }

    fn manager() -> MachineManager<StubRuntime> {
        MachineManager::new(StubRuntime::new())
    }

    fn bogus_status(value: &str) -> MachineStatus {
        MachineStatus {
            id: MachineId::from("machine-9"),
            name: name(value),
            image: "ghcr.io/adesk/machine:latest".into(),
            state: MachineState::Running,
            pid: None,
            created_at_ms: 0,
            viewer: ViewerExposure::None,
        }
    }

    #[tokio::test]
    async fn create_registers_and_returns_the_status() {
        let manager = manager();
        let status = manager.create(&spec("adesk")).await.unwrap();

        assert_eq!(status.name, name("adesk"));
        assert_eq!(status.id, first_id());
        assert_eq!(status.state, MachineState::Created);
        assert!(manager.lock_registry().contains(&name("adesk")));
        assert_eq!(
            manager
                .lock_registry()
                .get(&name("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Created)
        );
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_name_via_the_registry() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();

        let err = manager.create(&spec("adesk")).await.unwrap_err();
        assert!(matches!(err, MachineError::Duplicate { name } if name == "adesk"));
        // The original machine is untouched.
        assert_eq!(manager.lock_registry().len(), 1);
    }

    #[tokio::test]
    async fn create_maps_a_backend_duplicate() {
        let runtime = StubRuntime::new();
        runtime.seed(&spec("adesk"));
        let manager = MachineManager::new(runtime);

        let err = manager.create(&spec("adesk")).await.unwrap_err();
        assert!(matches!(err, MachineError::Duplicate { name } if name == "adesk"));
        assert!(manager.lock_registry().is_empty());
    }

    #[tokio::test]
    async fn unknown_names_are_not_found() {
        let manager = manager();
        let ghost = name("ghost");

        assert!(matches!(
            manager.start(&ghost).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
        assert!(matches!(
            manager.stop(&ghost, 0).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
        assert!(matches!(
            manager.status(&ghost).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
        assert!(matches!(
            manager.remove(&ghost, false).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn start_refreshes_the_cache() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();

        let status = manager.start(&name("adesk")).await.unwrap();
        assert_eq!(status.state, MachineState::Running);
        assert!(status.is_running());
        assert_eq!(status.pid, Some(2_000));
        assert_eq!(
            manager
                .lock_registry()
                .get(&name("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Running)
        );
    }

    #[tokio::test]
    async fn stop_refreshes_the_cache() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();
        manager.start(&name("adesk")).await.unwrap();

        let status = manager.stop(&name("adesk"), 1_500).await.unwrap();
        assert_eq!(status.state, MachineState::Stopped);
        assert_eq!(
            manager
                .lock_registry()
                .get(&name("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Stopped)
        );
    }

    #[tokio::test]
    async fn restart_stops_then_starts() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();
        manager.start(&name("adesk")).await.unwrap();

        let status = manager.restart(&name("adesk")).await.unwrap();
        assert_eq!(status.state, MachineState::Running);
        assert_eq!(
            manager
                .lock_registry()
                .get(&name("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Running)
        );
    }

    #[tokio::test]
    async fn remove_returns_the_pre_removal_status_and_forgets_it() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();
        manager.start(&name("adesk")).await.unwrap();

        let removed = manager.remove(&name("adesk"), true).await.unwrap();
        assert_eq!(removed.state, MachineState::Running);
        assert!(!manager.lock_registry().contains(&name("adesk")));
        assert!(matches!(
            manager.status(&name("adesk")).await.unwrap_err(),
            MachineError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn status_refreshes_the_cache() {
        let manager = manager();
        manager.create(&spec("adesk")).await.unwrap();
        // Start directly through the backend so the cache is temporarily stale.
        let id = manager.lock_registry().id(&name("adesk")).cloned().unwrap();
        manager.runtime().start(&id).await.unwrap();

        let status = manager.status(&name("adesk")).await.unwrap();
        assert_eq!(status.state, MachineState::Running);
        assert_eq!(
            manager
                .lock_registry()
                .get(&name("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Running)
        );
    }

    #[tokio::test]
    async fn list_rebuilds_the_registry_from_the_backend() {
        let manager = manager();
        manager.create(&spec("alpha")).await.unwrap();
        manager.create(&spec("bravo")).await.unwrap();

        // Corrupt the cache: add a bogus entry and forget a real one.
        {
            let mut registry = manager.lock_registry();
            registry.insert(bogus_status("stale"));
            registry.remove(&name("bravo"));
        }
        assert!(manager.lock_registry().contains(&name("stale")));
        assert!(!manager.lock_registry().contains(&name("bravo")));

        let mut listed: Vec<MachineName> = manager
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        listed.sort();

        assert_eq!(listed, vec![name("alpha"), name("bravo")]);
        assert!(!manager.lock_registry().contains(&name("stale")));
        assert!(manager.lock_registry().contains(&name("bravo")));
    }
}
