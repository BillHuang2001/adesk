//! The machine specification: everything needed to create one machine.
//!
//! `MachineSpec::default()` describes the ADesk machine — a container that runs
//! the ADesk runtime binary with a viewer socket bind-mounted into it — and
//! matches `docs/machine.md` §3/§5. Callers start from the default (or
//! `MachineSpec::new`) and adjust with the `with_*` builders.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::MachineName;

/// A bind mount from the host into the container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// Path on the host.
    pub host_path: PathBuf,
    /// Path inside the container.
    pub container_path: PathBuf,
    /// Whether the container sees the mount read-only.
    pub read_only: bool,
}

impl Mount {
    /// A read-only mount: the container can read `host` at `container` but not
    /// modify it.
    pub fn ro(host: impl Into<PathBuf>, container: impl Into<PathBuf>) -> Mount {
        Mount {
            host_path: host.into(),
            container_path: container.into(),
            read_only: true,
        }
    }

    /// A read-write mount.
    pub fn rw(host: impl Into<PathBuf>, container: impl Into<PathBuf>) -> Mount {
        Mount {
            host_path: host.into(),
            container_path: container.into(),
            read_only: false,
        }
    }
}

/// How the machine's network is connected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// No networking (own empty network namespace).
    #[default]
    None,
    /// Share the host network namespace.
    Host,
    /// An isolated private network namespace.
    Private,
}

impl NetworkMode {
    /// The stable snake_case name used on the wire and in CLI arguments.
    pub fn as_str(&self) -> &'static str {
        match self {
            NetworkMode::None => "none",
            NetworkMode::Host => "host",
            NetworkMode::Private => "private",
        }
    }
}

/// How the viewer (outside the machine) reaches ADesk (inside it).
///
/// Both cases are expressible as ordinary container options — a bind mount or a
/// published port — so the container backend stays generic
/// (`docs/machine.md` §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "transport")]
pub enum ViewerExposure {
    /// The local case: a Unix socket bind-mounted into the container.
    UnixSocket {
        /// Socket path on the host.
        host_path: PathBuf,
        /// Socket path inside the container.
        container_path: PathBuf,
    },
    /// The remote case: a published TCP port.
    TcpPort {
        /// Port on the host.
        host_port: u16,
        /// Port inside the container.
        container_port: u16,
    },
    /// The viewer transport is not exposed through the container.
    None,
}

impl ViewerExposure {
    /// The local case: expose the host socket `host` inside the container at
    /// `container`.
    pub fn default_unix(host: impl Into<PathBuf>, container: impl Into<PathBuf>) -> ViewerExposure {
        ViewerExposure::UnixSocket {
            host_path: host.into(),
            container_path: container.into(),
        }
    }
}

/// Everything needed to create one machine.
///
/// The `viewer` field is the single source of truth for how the viewer transport
/// crosses the boundary; the backend translates it into a bind mount (Unix
/// socket) or a published port (`TcpPort`), so it must not also be listed in
/// `mounts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineSpec {
    /// Unique manager-facing name.
    pub name: MachineName,
    /// Container image to run.
    pub image: String,
    /// Command to run in the container (the container's `argv`).
    pub command: Vec<String>,
    /// Environment variables for the container's main process.
    pub env: BTreeMap<String, String>,
    /// Extra bind mounts (the viewer socket is expressed via `viewer`, not here).
    pub mounts: Vec<Mount>,
    /// Memory limit in MiB, when set.
    pub memory_mb: Option<u64>,
    /// CPU limit in cores, when set.
    pub cpus: Option<f64>,
    /// Network connection mode.
    pub network: NetworkMode,
    /// How the viewer reaches ADesk inside the machine.
    pub viewer: ViewerExposure,
    /// Backend/container labels, for identification and cleanup.
    pub labels: BTreeMap<String, String>,
}

impl MachineSpec {
    /// A spec for `name` running `image`, with the ADesk defaults for everything
    /// else (a viewer Unix socket, a private network, a 4 GiB / 2-core limit and
    /// the `adesk-machine` label).
    ///
    /// The `command` is left empty here; use `MachineSpec::default()` when the
    /// machine should run ADesk itself.
    pub fn new(name: impl Into<MachineName>, image: impl Into<String>) -> MachineSpec {
        MachineSpec {
            name: name.into(),
            image: image.into(),
            ..MachineSpec::default()
        }
    }

    /// Sets the machine name.
    pub fn with_name(mut self, name: impl Into<MachineName>) -> MachineSpec {
        self.name = name.into();
        self
    }

    /// Sets the container image.
    pub fn with_image(mut self, image: impl Into<String>) -> MachineSpec {
        self.image = image.into();
        self
    }

    /// Replaces the container command (`argv`).
    pub fn with_command(mut self, command: Vec<String>) -> MachineSpec {
        self.command = command;
        self
    }

    /// Adds an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> MachineSpec {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Adds a bind mount.
    pub fn with_mount(mut self, mount: Mount) -> MachineSpec {
        self.mounts.push(mount);
        self
    }

    /// Sets the memory limit in MiB.
    pub fn with_memory_mb(mut self, memory_mb: u64) -> MachineSpec {
        self.memory_mb = Some(memory_mb);
        self
    }

    /// Sets the CPU limit in cores.
    pub fn with_cpus(mut self, cpus: f64) -> MachineSpec {
        self.cpus = Some(cpus);
        self
    }

    /// Sets the network mode.
    pub fn with_network(mut self, network: NetworkMode) -> MachineSpec {
        self.network = network;
        self
    }

    /// Sets how the viewer reaches ADesk.
    pub fn with_viewer(mut self, viewer: ViewerExposure) -> MachineSpec {
        self.viewer = viewer;
        self
    }

    /// Adds a container label.
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> MachineSpec {
        self.labels.insert(key.into(), value.into());
        self
    }
}

impl Default for MachineSpec {
    /// The ADesk machine: an image that ships the ADesk runtime, the
    /// `adesk-server` command, a viewer Unix socket at `/run/adesk/viewer.sock`
    /// bind-mounted at the same path, a private network and a 4 GiB / 2-core
    /// limit.
    fn default() -> MachineSpec {
        let mut labels = BTreeMap::new();
        labels.insert("adesk.io/role".to_owned(), "machine".to_owned());
        MachineSpec {
            name: MachineName::from("adesk"),
            image: "ghcr.io/adesk/machine:latest".to_owned(),
            command: vec!["adesk-server".to_owned()],
            env: BTreeMap::new(),
            mounts: Vec::new(),
            memory_mb: Some(4096),
            cpus: Some(2.0),
            network: NetworkMode::Private,
            viewer: ViewerExposure::default_unix(
                "/run/adesk/viewer.sock",
                "/run/adesk/viewer.sock",
            ),
            labels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_describes_the_adesk_machine() {
        let spec = MachineSpec::default();
        assert_eq!(spec.name, MachineName::from("adesk"));
        assert!(spec.image.contains("adesk"));
        assert_eq!(spec.command, vec!["adesk-server".to_owned()]);
        assert!(spec.env.is_empty());
        assert!(spec.mounts.is_empty());
        assert_eq!(spec.memory_mb, Some(4096));
        assert_eq!(spec.cpus, Some(2.0));
        assert_eq!(spec.network, NetworkMode::Private);
        assert_eq!(
            spec.viewer,
            ViewerExposure::default_unix("/run/adesk/viewer.sock", "/run/adesk/viewer.sock")
        );
        assert_eq!(
            spec.labels.get("adesk.io/role").map(String::as_str),
            Some("machine")
        );
    }

    #[test]
    fn new_overrides_name_and_image_and_keeps_defaults() {
        let spec = MachineSpec::new("work", "debian:trixie");
        assert_eq!(spec.name, MachineName::from("work"));
        assert_eq!(spec.image, "debian:trixie");
        // Command is inherited from Default (runs adesk-server).
        assert_eq!(spec.command, vec!["adesk-server".to_owned()]);
        assert_eq!(spec.network, NetworkMode::Private);
    }

    #[test]
    fn builders_mutate_the_spec() {
        let spec = MachineSpec::new("work", "debian:trixie")
            .with_name("renamed")
            .with_image("ubuntu:noble")
            .with_command(vec!["bash".to_owned(), "-l".to_owned()])
            .with_env("LANG", "C.UTF-8")
            .with_env("TZ", "UTC")
            .with_mount(Mount::ro("/etc/localtime", "/etc/localtime"))
            .with_memory_mb(2048)
            .with_cpus(1.5)
            .with_network(NetworkMode::Host)
            .with_viewer(ViewerExposure::TcpPort {
                host_port: 9000,
                container_port: 9000,
            })
            .with_label("team", "agent");

        assert_eq!(spec.name, MachineName::from("renamed"));
        assert_eq!(spec.image, "ubuntu:noble");
        assert_eq!(spec.command, vec!["bash".to_owned(), "-l".to_owned()]);
        assert_eq!(spec.env.get("LANG").map(String::as_str), Some("C.UTF-8"));
        assert_eq!(spec.env.get("TZ").map(String::as_str), Some("UTC"));
        assert_eq!(spec.mounts.len(), 1);
        assert_eq!(spec.memory_mb, Some(2048));
        assert_eq!(spec.cpus, Some(1.5));
        assert_eq!(spec.network, NetworkMode::Host);
        assert_eq!(
            spec.viewer,
            ViewerExposure::TcpPort {
                host_port: 9000,
                container_port: 9000,
            }
        );
        assert_eq!(spec.labels.get("team").map(String::as_str), Some("agent"));
    }

    #[test]
    fn mount_ro_and_rw_set_read_only() {
        let ro = Mount::ro("/host/a", "/container/a");
        assert_eq!(ro.host_path, PathBuf::from("/host/a"));
        assert_eq!(ro.container_path, PathBuf::from("/container/a"));
        assert!(ro.read_only);

        let rw = Mount::rw("/host/b", "/container/b");
        assert!(!rw.read_only);
    }

    #[test]
    fn network_mode_as_str_matches_serde() {
        let cases = [
            (NetworkMode::None, "none"),
            (NetworkMode::Host, "host"),
            (NetworkMode::Private, "private"),
        ];
        for (mode, name) in cases {
            assert_eq!(mode.as_str(), name);
            assert_eq!(serde_json::to_value(mode).unwrap(), serde_json::json!(name));
            assert_eq!(
                serde_json::from_value::<NetworkMode>(serde_json::json!(name)).unwrap(),
                mode
            );
        }
        assert_eq!(NetworkMode::default(), NetworkMode::None);
    }

    #[test]
    fn viewer_exposure_round_trips_through_serde() {
        let cases = [
            ViewerExposure::default_unix("/run/adesk/viewer.sock", "/run/adesk/viewer.sock"),
            ViewerExposure::TcpPort {
                host_port: 7100,
                container_port: 8100,
            },
            ViewerExposure::None,
        ];
        for exposure in cases {
            let json = serde_json::to_string(&exposure).unwrap();
            let back: ViewerExposure = serde_json::from_str(&json).unwrap();
            assert_eq!(back, exposure);
        }

        assert_eq!(
            serde_json::to_value(ViewerExposure::TcpPort {
                host_port: 1,
                container_port: 2,
            })
            .unwrap(),
            serde_json::json!({ "transport": "tcp_port", "host_port": 1, "container_port": 2 })
        );
        assert_eq!(
            serde_json::to_value(ViewerExposure::None).unwrap(),
            serde_json::json!({ "transport": "none" })
        );
    }

    #[test]
    fn spec_round_trips_through_serde() {
        let spec = MachineSpec::default()
            .with_env("ADESK_RENDERER", "pixman")
            .with_mount(Mount::rw("/srv/share", "/mnt/share"));
        let json = serde_json::to_string(&spec).unwrap();
        let back: MachineSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }
}
