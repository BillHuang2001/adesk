# adesk-machine — AI Machine runtime + host control plane
## Intent
`adesk-machine` gives an agent a Linux machine it fully owns — root inside, systemd as PID 1, Nix for software — inside a rootless container, while the host keeps control of the boundary (`docs/machine.md` is normative).
It is the host-side subsystem the ADesk GUI runtime does not cover: machine lifecycle, the container-backend seam, networking/viewer exposure, host capabilities and approval routing.
It is a different domain from the GUI runtime, so it carries its own error type and does **not** depend on `adesk-core`.
The container implementation is an abstraction: rootless Podman is the first backend, `systemd-nspawn` can be added without touching the manager.
## API Surface
Crate root (`src/lib.rs`) re-exports every public item below (`adesk_machine::<Name>`).
### Errors (`src/error.rs`)
- `MachineError` (`thiserror`): `Runtime { message }` (backend failure), `Backend { program, status, stderr }` (a CLI backend exited non-zero), `NotFound { name }`, `Duplicate { name }`, `InvalidSpec { message }`, `InvalidState { name, state, action }`, `Io(std::io::Error)`, `Spawn(std::io::Error)`.
- `Result<T, E = MachineError>`.
### Spec + state (`src/spec.rs`, `src/state.rs`)
- `MachineId(pub String)`, `MachineName(pub String)` — `Display`, `From<&str>`, serde transparent.
- `MachineSpec { name: MachineName, image: String, command: Vec<String>, env: BTreeMap<String, String>, mounts: Vec<Mount>, memory_mb: Option<u64>, cpus: Option<f64>, network: NetworkMode, viewer: ViewerExposure, labels: BTreeMap<String, String> }` with `Default` (a machine that runs ADesk) + builders.
- `Mount { host_path: PathBuf, container_path: PathBuf, read_only: bool }` + `ro`/`rw`.
- `NetworkMode { None, Host, Private }` (serde snake_case).
- `ViewerExposure { UnixSocket { host_path, container_path }, TcpPort { host_port, container_port }, None }` + `default_unix(host, container)`.
- `MachineState { Created, Running, Stopped, Exited { code: i32 }, Failed { message: String } }` + `is_running()`, `as_str()`.
- `MachineStatus { id, name, image, state, pid: Option<u32>, created_at_ms: u64, viewer: ViewerExposure }` + `is_running()`.
### Backend seam (`src/runtime.rs`, `src/runtime/`)
- `RuntimeKind { Podman, Mock }` + `as_str()`.
- `#[async_trait] trait ContainerRuntime: Send + Sync + 'static` — `kind()`, `create(&MachineSpec) -> Result<MachineId>`, `start(&MachineId)`, `stop(&MachineId, timeout_ms)`, `remove(&MachineId, force)`, `status(&MachineId) -> Result<MachineStatus>`, `list() -> Result<Vec<MachineStatus>>`.
- `PodmanRuntime` (`src/runtime/podman.rs`) — rootless Podman via CLI: `new()`, `with_program(path)` (testable against a stub), `with_binary_env`; maps `podman create/start/stop/rm/inspect` onto the trait; `create` translates a `MachineSpec` into the argv (mounts, env, memory/cpus, network, viewer exposure).
- `MockRuntime` (`src/runtime/mock.rs`) — in-memory, deterministic; models the lifecycle state machine, hands out ids `machine-1`, and records every call (`calls() -> Vec<RuntimeCall>`) for assertions; `with_clock(Arc<dyn Fn() -> u64 + Send + Sync>)` for deterministic timestamps.
### Manager + registry (`src/manager.rs`, `src/registry.rs`)
- `MachineManager<R: ContainerRuntime>` — `new(runtime)`, `runtime()`; `async create(&MachineSpec) -> Result<MachineStatus>` (unique names), `start(&MachineName)`, `stop(&MachineName, timeout_ms)`, `restart(&MachineName)`, `remove(&MachineName, force)`, `status(&MachineName) -> Result<MachineStatus>`, `list() -> Result<Vec<MachineStatus>>`.
- `MachineRegistry` — the name ↔ id map + last known `MachineStatus` cache; `insert`, `get`, `remove`, `list`, `update`.
### Host control plane (`src/host.rs`, `src/approval.rs`)
- `HostCapabilities { gpu: bool, kvm: bool, allowed_mounts: Vec<PathBuf>, publish_ports: bool }` + `Default`, `allows_mount`, `allows_port`.
- `HostControlPlane<R>` — `new(manager, capabilities)`; `capabilities()`, `machines()`/`status()`/`create()`/`start()`/`stop()`/`remove()` (delegating to the manager), `request_approval(ApprovalRequest)`.
- `ApprovalRequest { id: u64, machine: MachineName, capability: String, description: String }`, `ApprovalDecision { Allow, Deny { reason } }`, `#[async_trait] trait Approver: Send + Sync { async fn decide(&self, &ApprovalRequest) -> ApprovalDecision; }`, `ApprovalRouter` (`new(Arc<dyn Approver>)`, `request(req)`, `with_policy`); `AutoApprove`/`AutoDeny` approvers.
### Binary (`src/main.rs`)
- `adesk-machine` (clap): `--runtime podman|mock`, `--log <FILTER>`; subcommands `create --name --image [--command ...] [--viewer-unix HOST:CONTAINER] [--viewer-tcp HOST:CONTAINER] [--mount HOST:CONTAINER[:ro]] [--memory-mb N] [--cpus N] [--network none|host|private]`, `start <NAME>`, `stop <NAME> [--timeout-ms]`, `restart <NAME>`, `remove <NAME> [--force]`, `list`, `status <NAME>`, `capabilities`.
- Exit codes: `0` success, `1` runtime error, `2` config/CLI error.
## Constraints
- `docs/machine.md` is normative; the container engine never leaks past `ContainerRuntime`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay well under the ~1000-line threshold; split along module boundaries.
- No test may require an installed container engine, a display, GPU or network: unit tests use `MockRuntime`; a CLI backend may optionally be exercised against a stub program via `PodmanRuntime::with_program`, never against a real `podman`.
- No panics on request/command paths: every failure returns `MachineError`.
- `tracing` for lifecycle events (create/start/stop/remove); never log environment secrets.
- Dependencies come only from root `[workspace.dependencies]` (`async-trait`, `clap`, `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`); never inline versions.
- The host exposes no API for an operation the AI can perform inside its own machine (no file edit, no package install, no service control).
## Routing Table
| Area | Owner |
|---|---|
| Error type + `Result` | `./src/error.rs` |
| `MachineSpec`, `Mount`, `NetworkMode`, `ViewerExposure` | `./src/spec.rs` |
| `MachineId`, `MachineName`, `MachineState`, `MachineStatus` | `./src/state.rs` |
| `ContainerRuntime` trait + `RuntimeKind` | `./src/runtime.rs` |
| Rootless Podman CLI backend | `./src/runtime/podman.rs` |
| In-memory deterministic backend (tests + dev) | `./src/runtime/mock.rs` |
| Lifecycle manager | `./src/manager.rs` |
| Name ↔ id map + status cache | `./src/registry.rs` |
| Host capabilities + control plane | `./src/host.rs` |
| Approval request/decision/router/approvers | `./src/approval.rs` |
| CLI wiring | `./src/main.rs` |
| Spec translation + mock-runtime tests | `./tests/` |
| Stub-program CLI backend tests | `./tests/podman_stub.rs` |
## Design Decisions
- **The backend seam is the whole point.** `MachineManager` and `HostControlPlane` speak only `ContainerRuntime`, so a new engine is one trait impl; the manager never sees argv.
- **`MockRuntime` is first-class.** Because CI has no container engine, the mock is the reference implementation of the lifecycle state machine and every unit test drives it; `PodmanRuntime` is verified against a stub program for argv/Spec translation and error mapping.
- **Names are the manager's key.** The registry maps unique `MachineName` → `MachineId`; a duplicate `create` fails (`Duplicate`) rather than silently replacing — the agent's machine has a stable identity across restarts of the manager process (the registry is rebuilt from `list()`).
- **Viewer exposure is expressed as container options.** `ViewerExposure` translates to a bind-mount or a published port, so the container runtime stays generic and ADesk never learns about the engine.
- **Approvals are an interface, not a policy.** `ApprovalRouter` + `Approver` let the host (and the viewer UI) decide; the crate ships `AutoApprove`/`AutoDeny` for tests and simple deployments.
## Test Strategy
Tests in `./tests/`, no container engine/display/GPU/network:
- Lifecycle over `MockRuntime`: create/start/stop/restart/remove, duplicate names, unknown names, state transitions, `list()` rebuild.
- Spec translation: `MachineSpec` → `PodmanRuntime` argv (via a capturing stub program or a pure `build_argv` helper), viewer exposure → mounts/ports.
- Approvals: router with `AutoApprove`/`AutoDeny`, request routing.
- `./tests/podman_stub.rs` — a temp executable shell stub invoked through `PodmanRuntime::with_program` to pin the argv and the non-zero-exit → `Backend` error mapping.
- Run with `./scripts/dev.sh cargo test -p adesk-machine`.
## Notes for Agents
- `adesk-machine` does not depend on `adesk-core` or any GUI crate; the ADesk kernel binary name is data (`MachineSpec::command`), not a dependency.
- The default `MachineSpec` describes the ADesk machine (image + command running `adesk-server` + a viewer socket mount); keep that default coherent with `docs/machine.md` §3/§5.
- This crate is an interface + a reference backend, not an orchestrator: multi-host scheduling and image building are out of scope (`docs/machine.md` §8).
## Status
Skeleton only: `src/lib.rs` carries the crate attributes and docs; every module above is **not yet implemented**. Implement the surface exactly as documented here.
