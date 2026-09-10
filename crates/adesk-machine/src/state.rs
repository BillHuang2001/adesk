//! Machine identity, lifecycle state and status.
//!
//! `MachineId` is assigned by the container backend and opaque to everyone else;
//! `MachineName` is the human/agent-facing key the manager makes unique (see
//! `docs/machine.md` §3). `MachineStatus` is the backend's view of one machine,
//! cached in the registry.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::spec::ViewerExposure;

/// Stable, opaque machine id assigned by the container backend.
///
/// Serializes transparently as the bare string so it can be persisted and fed
/// back to the backend unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineId(pub String);

impl MachineId {
    /// Borrows the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for MachineId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MachineId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<MachineId> for String {
    fn from(value: MachineId) -> Self {
        value.0
    }
}

impl AsRef<str> for MachineId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Human/agent-facing machine name, unique within a manager.
///
/// The registry maps this to the backend-assigned `MachineId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineName(pub String);

impl MachineName {
    /// Borrows the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MachineName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for MachineName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MachineName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<MachineName> for String {
    fn from(value: MachineName) -> Self {
        value.0
    }
}

impl AsRef<str> for MachineName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The lifecycle state of one machine.
///
/// Serialized as an internally tagged object, e.g. `{"state":"running"}` or
/// `{"state":"exited","code":0}`, so the state and its detail always travel
/// together and round-trip through serde unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum MachineState {
    /// The container exists but has never been started.
    Created,
    /// The container is running.
    Running,
    /// The container was stopped cleanly.
    Stopped,
    /// The container's main process exited with `code`.
    Exited {
        /// Exit code of the container's main process.
        code: i32,
    },
    /// The container failed to start or crashed.
    Failed {
        /// Human-readable description of the failure.
        message: String,
    },
}

impl MachineState {
    /// Whether the machine is currently running.
    pub fn is_running(&self) -> bool {
        matches!(self, MachineState::Running)
    }

    /// The stable snake_case name used on the wire and in logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            MachineState::Created => "created",
            MachineState::Running => "running",
            MachineState::Stopped => "stopped",
            MachineState::Exited { .. } => "exited",
            MachineState::Failed { .. } => "failed",
        }
    }
}

impl fmt::Display for MachineState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The backend's view of one machine.
///
/// This is the value `MachineManager` and `MachineRegistry` cache; it carries
/// everything a caller needs to render or correlate a machine without another
/// backend round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineStatus {
    /// Backend-assigned id.
    pub id: MachineId,
    /// Unique manager-facing name.
    pub name: MachineName,
    /// Container image the machine runs.
    pub image: String,
    /// Current lifecycle state.
    pub state: MachineState,
    /// Container pid, when the backend reports one.
    pub pid: Option<u32>,
    /// Monotonic creation timestamp (milliseconds since the manager started).
    pub created_at_ms: u64,
    /// How the viewer reaches ADesk inside the machine.
    pub viewer: ViewerExposure,
}

impl MachineStatus {
    /// Whether the machine is currently running.
    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_display_and_conversions() {
        let id = MachineId::from("machine-1");
        assert_eq!(id.as_str(), "machine-1");
        assert_eq!(id.as_ref(), "machine-1");
        assert_eq!(id.to_string(), "machine-1");
        let owned: String = id.clone().into();
        assert_eq!(owned, "machine-1");
        assert_eq!(MachineId::from(owned), id);
    }

    #[test]
    fn machine_name_display_and_conversions() {
        let name = MachineName::from("adesk");
        assert_eq!(name.as_str(), "adesk");
        assert_eq!(name.as_ref(), "adesk");
        assert_eq!(name.to_string(), "adesk");
        assert_eq!(MachineName::from(String::from("adesk")), name);
        assert_ne!(MachineName::from("a"), MachineName::from("b"));
    }

    #[test]
    fn ids_serde_transparently() {
        assert_eq!(
            serde_json::to_string(&MachineId::from("machine-1")).unwrap(),
            "\"machine-1\""
        );
        assert_eq!(
            serde_json::from_str::<MachineId>("\"machine-1\"").unwrap(),
            MachineId::from("machine-1")
        );
        assert_eq!(
            serde_json::to_string(&MachineName::from("adesk")).unwrap(),
            "\"adesk\""
        );
    }

    #[test]
    fn names_sort_deterministically() {
        let mut names = vec![
            MachineName::from("b"),
            MachineName::from("a"),
            MachineName::from("c"),
        ];
        names.sort();
        assert_eq!(
            names,
            vec![
                MachineName::from("a"),
                MachineName::from("b"),
                MachineName::from("c"),
            ]
        );
    }

    #[test]
    fn state_is_running_and_as_str() {
        assert!(MachineState::Running.is_running());
        assert!(!MachineState::Created.is_running());
        assert!(!MachineState::Stopped.is_running());

        assert_eq!(MachineState::Created.as_str(), "created");
        assert_eq!(MachineState::Running.as_str(), "running");
        assert_eq!(MachineState::Stopped.as_str(), "stopped");
        assert_eq!(MachineState::Exited { code: 3 }.as_str(), "exited");
        assert_eq!(
            MachineState::Failed {
                message: "boom".into()
            }
            .as_str(),
            "failed"
        );
        assert_eq!(MachineState::Running.to_string(), "running");
    }

    #[test]
    fn state_round_trips_through_serde() {
        let states = [
            MachineState::Created,
            MachineState::Running,
            MachineState::Stopped,
            MachineState::Exited { code: 137 },
            MachineState::Failed {
                message: "no kernel".into(),
            },
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let back: MachineState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, state);
        }
        assert_eq!(
            serde_json::to_value(MachineState::Exited { code: 0 }).unwrap(),
            serde_json::json!({ "state": "exited", "code": 0 })
        );
        assert_eq!(
            serde_json::to_value(MachineState::Running).unwrap(),
            serde_json::json!({ "state": "running" })
        );
    }

    #[test]
    fn status_round_trips_and_reports_running() {
        let status = MachineStatus {
            id: MachineId::from("machine-1"),
            name: MachineName::from("adesk"),
            image: "ghcr.io/adesk/machine:latest".into(),
            state: MachineState::Running,
            pid: Some(4242),
            created_at_ms: 17,
            viewer: ViewerExposure::default_unix(
                "/run/adesk/viewer.sock",
                "/run/adesk/viewer.sock",
            ),
        };
        assert!(status.is_running());

        let json = serde_json::to_string(&status).unwrap();
        let back: MachineStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }
}
