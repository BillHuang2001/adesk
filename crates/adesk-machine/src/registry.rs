//! The manager's cache of backend truth: a name ↔ id map plus the last known
//! `MachineStatus` per machine.
//!
//! The manager keeps a `MachineRegistry` so name lookups and status queries
//! never require a backend round-trip on the hot path; the backend remains the
//! source of truth and entries are refreshed with [`MachineRegistry::update`]
//! (see `docs/machine.md` §3).

use std::collections::BTreeMap;

use crate::state::{MachineId, MachineName, MachineStatus};

/// The name ↔ id map plus a last-known `MachineStatus` cache.
///
/// Keyed by `MachineName` (the manager's unique key); the backing maps are
/// `BTreeMap`s, so ordering is deterministic.
#[derive(Debug, Clone, Default)]
pub struct MachineRegistry {
    ids: BTreeMap<MachineName, MachineId>,
    statuses: BTreeMap<MachineName, MachineStatus>,
}

impl MachineRegistry {
    /// An empty registry.
    pub fn new() -> MachineRegistry {
        MachineRegistry::default()
    }

    /// Number of machines currently registered.
    pub fn len(&self) -> usize {
        self.statuses.len()
    }

    /// Whether the registry holds no machines.
    pub fn is_empty(&self) -> bool {
        self.statuses.is_empty()
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &MachineName) -> bool {
        self.statuses.contains_key(name)
    }

    /// The backend id for `name`, if registered.
    pub fn id(&self, name: &MachineName) -> Option<&MachineId> {
        self.ids.get(name)
    }

    /// The last known status of `name`, if registered.
    pub fn get(&self, name: &MachineName) -> Option<&MachineStatus> {
        self.statuses.get(name)
    }

    /// Registers a newly created machine, recording both its id and its status.
    ///
    /// Returns the previously cached status if `name` was already registered, so
    /// a caller can detect an unexpected overwrite.
    pub fn insert(&mut self, status: MachineStatus) -> Option<MachineStatus> {
        self.ids.insert(status.name.clone(), status.id.clone());
        self.statuses.insert(status.name.clone(), status)
    }

    /// Refreshes the cached status of an already-registered machine.
    ///
    /// Unknown names are ignored (returns `None`) rather than inserting: the
    /// name ↔ id association is established by [`MachineRegistry::insert`], and
    /// `update` only refreshes backend truth. Returns the previous status when
    /// the machine was known.
    pub fn update(&mut self, status: MachineStatus) -> Option<MachineStatus> {
        if !self.ids.contains_key(&status.name) {
            return None;
        }
        self.ids.insert(status.name.clone(), status.id.clone());
        self.statuses.insert(status.name.clone(), status)
    }

    /// Forgets `name`, returning its last known status.
    pub fn remove(&mut self, name: &MachineName) -> Option<MachineStatus> {
        self.ids.remove(name);
        self.statuses.remove(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ViewerExposure;
    use crate::state::MachineState;

    fn status(name: &str, id: &str, state: MachineState) -> MachineStatus {
        MachineStatus {
            id: MachineId::from(id),
            name: MachineName::from(name),
            image: "ghcr.io/adesk/machine:latest".into(),
            state,
            pid: None,
            created_at_ms: 0,
            viewer: ViewerExposure::None,
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = MachineRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(!registry.contains(&MachineName::from("adesk")));
        assert_eq!(registry.get(&MachineName::from("adesk")), None);
        assert_eq!(registry.id(&MachineName::from("adesk")), None);
    }

    #[test]
    fn insert_then_get_and_id() {
        let mut registry = MachineRegistry::new();
        let name = MachineName::from("adesk");
        assert_eq!(
            registry.insert(status("adesk", "machine-1", MachineState::Created)),
            None
        );

        assert!(registry.contains(&name));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.id(&name), Some(&MachineId::from("machine-1")));
        assert_eq!(
            registry.get(&name).map(|s| s.state.clone()),
            Some(MachineState::Created)
        );
    }

    #[test]
    fn insert_returns_previous_status_on_overwrite() {
        let mut registry = MachineRegistry::new();
        registry.insert(status("adesk", "machine-1", MachineState::Created));
        let previous = registry.insert(status("adesk", "machine-2", MachineState::Running));

        let previous = previous.expect("previous status");
        assert_eq!(previous.id, MachineId::from("machine-1"));
        assert_eq!(previous.state, MachineState::Created);
        assert_eq!(
            registry.id(&MachineName::from("adesk")),
            Some(&MachineId::from("machine-2"))
        );
    }

    #[test]
    fn update_refreshes_known_and_ignores_unknown() {
        let mut registry = MachineRegistry::new();
        registry.insert(status("adesk", "machine-1", MachineState::Created));

        let previous = registry.update(status("adesk", "machine-1", MachineState::Running));
        assert_eq!(previous.map(|s| s.state), Some(MachineState::Created));
        assert_eq!(
            registry
                .get(&MachineName::from("adesk"))
                .map(|s| s.state.clone()),
            Some(MachineState::Running)
        );

        // Unknown names are not inserted by update.
        assert_eq!(
            registry.update(status("ghost", "machine-9", MachineState::Running)),
            None
        );
        assert_eq!(registry.len(), 1);
        assert!(!registry.contains(&MachineName::from("ghost")));
    }

    #[test]
    fn remove_forgets_the_machine() {
        let mut registry = MachineRegistry::new();
        let name = MachineName::from("adesk");
        registry.insert(status("adesk", "machine-1", MachineState::Running));

        let removed = registry.remove(&name).expect("status");
        assert_eq!(removed.id, MachineId::from("machine-1"));
        assert!(registry.is_empty());
        assert_eq!(registry.id(&name), None);
        assert_eq!(registry.remove(&name), None);
    }

    #[test]
    fn multiple_entries_are_tracked_independently() {
        let mut registry = MachineRegistry::new();
        registry.insert(status("charlie", "machine-3", MachineState::Running));
        registry.insert(status("alpha", "machine-1", MachineState::Running));
        registry.insert(status("bravo", "machine-2", MachineState::Created));

        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
        for (name, id, state) in [
            ("alpha", "machine-1", MachineState::Running),
            ("bravo", "machine-2", MachineState::Created),
            ("charlie", "machine-3", MachineState::Running),
        ] {
            let name = MachineName::from(name);
            assert!(registry.contains(&name));
            assert_eq!(registry.id(&name), Some(&MachineId::from(id)));
            assert_eq!(registry.get(&name).map(|s| s.state.clone()), Some(state));
        }
    }
}
