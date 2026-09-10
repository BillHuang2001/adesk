//! ADesk AI Machine runtime — a lightweight Linux machine per assistant.
//!
//! This crate gives an agent a Linux environment it fully owns (root inside the
//! machine, systemd as PID 1, Nix for software) inside a rootless container,
//! while the host keeps control of the boundary. ADesk runs *inside* the
//! machine; a viewer runs *outside* it (`adesk-viewer`). The container
//! implementation is abstracted behind a backend trait so the runtime choice
//! (rootless Podman first, `systemd-nspawn` later) never reaches the machine or
//! the viewer.
//!
//! The normative design is `docs/machine.md`. The crate is layered as follows:
//!
//! - [`error`] — the crate-wide `MachineError` and `Result`.
//! - [`spec`] / [`state`] — the machine model: `MachineSpec`, `MachineId`,
//!   `MachineName`, `MachineState`, `MachineStatus` and the viewer exposure.
//! - [`runtime`] — the `ContainerRuntime` backend seam (`podman`, `mock`).
//! - [`registry`] — the manager's name ↔ id map and status cache.
//! - [`manager`] / [`host`] — the lifecycle manager and the host control plane.
//! - [`approval`] — boundary-crossing capability approvals.
//!
//! Every public item is re-exported at the crate root, so callers use
//! `adesk_machine::MachineError`, `adesk_machine::MachineSpec`, and so on.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod approval;
pub mod error;
pub mod host;
pub mod manager;
pub mod registry;
pub mod runtime;
pub mod spec;
pub mod state;

mod sync;

pub use approval::*;
pub use error::*;
pub use host::*;
pub use manager::*;
pub use registry::*;
pub use runtime::*;
pub use spec::*;
pub use state::*;
