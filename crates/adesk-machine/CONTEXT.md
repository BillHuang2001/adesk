# adesk-machine — AI Machine runtime + host control plane
## Intent
`adesk-machine` gives an agent a Linux machine it fully owns — root inside, systemd as PID 1, Nix for software — inside a rootless container, while the host keeps control of the boundary (`docs/machine.md` is normative).
It is the host-side subsystem the ADesk GUI runtime does not cover: machine lifecycle, the container-backend seam, networking/viewer exposure, host capabilities and approval routing.
It is a different domain from the GUI runtime, so it carries its own error type and does **not** depend on `adesk-core`.
The container implementation is an abstraction: rootless Podman is the first backend, `systemd-nspawn` can be added without touching the manager.
## API Surface
Crate root (`src/lib.rs`) glob-re-exports every public item of every module (`adesk_machine::<Name>`).
### Errors (`src/error.rs`)
- `MachineError` (`thiserror`): `Runtime { message }` (backend failure), `Backend { program, status, stderr }` (a CLI backend exited non-zero), `NotFound { name }`, `Duplicate { name }`, `InvalidSpec { message }`, `InvalidState { name, state, action }`, `Io(std::io::Error)`, `Spawn(std::io::Error)`.
- `Result<T, E = MachineError>`.
### Spec + state (`src/spec.rs`, `src/state.rs`)
- `MachineId(pub String)`, `MachineName(pub String)` — `Display`, `From<String>`/`From<&str>`, serde transparent.
- `MachineSpec { name: MachineName, image: String, command: Vec<String>, env: BTreeMap<String, String>, mounts: Vec<Mount>, memory_mb: Option<u64>, cpus: Option<f64>, network: NetworkMode, viewer: ViewerExposure, labels: BTreeMap<String, String> }` with `Default` (a machine that runs ADesk: image + `adesk-server` command + a viewer unix-socket mount, `NetworkMode::Private`) + builders (`new`, `with_name/image/command/env/mount/memory_mb/cpus/network/viewer/label`).
- `Mount { host_path: PathBuf, container_path: PathBuf, read_only: bool }` + `ro`/`rw`.
- `NetworkMode { None, Host, Private }` (serde snake_case) + `as_str`.
- `ViewerExposure { UnixSocket { host_path, container_path }, TcpPort { host_port, container_port }, None }` + `default_unix(host, container)` (serde tagged with the `transport` key).
- `MachineState { Created, Running, Stopped, Exited { code: i32 }, Failed { message: String } }` + `is_running()`, `as_str()`.
- `MachineStatus { id, name, image, state, pid: Option<u32>, created_at_ms: u64, viewer: ViewerExposure }` + `is_running()`.
### Backend seam (`src/runtime.rs`, `src/runtime/`)
- `RuntimeKind { Podman, Mock }` + `as_str()`.
- `#[async_trait] trait ContainerRuntime: Send + Sync + 'static` — `kind()`, `create(&MachineSpec) -> Result<MachineId>`, `start(&MachineId)`, `stop(&MachineId, timeout_ms)`, `remove(&MachineId, force)`, `status(&MachineId) -> Result<MachineStatus>`, `list() -> Result<Vec<MachineStatus>>`.
- `PodmanRuntime` (`src/runtime/podman.rs`) — rootless Podman via `tokio::process::Command`: `new()` (uses `podman` from PATH), `with_program(path)` (drives a stub in tests), `with_binary_env(key, value)`; maps `podman create/start/stop/rm/inspect` onto the trait; pure `build_create_argv(&MachineSpec) -> Vec<String>` translates the whole spec (mounts, env, labels, memory/cpus, network, viewer exposure, image, command).
- `MockRuntime` (`src/runtime/mock.rs`) — in-memory, deterministic; models the lifecycle state machine (ids `machine-1`, `machine-2`, …), records every call (`calls() -> Vec<RuntimeCall>`), `with_clock(Arc<dyn Fn() -> u64 + Send + Sync>)` for deterministic timestamps. `RuntimeCall` is one variant per trait method (`Create { name }`, `Start { id }`, `Stop { id, timeout_ms }`, `Remove { id, force }`, `Status { id }`, `List`).
### Manager + registry (`src/manager.rs`, `src/registry.rs`)
- `MachineManager<R: ContainerRuntime>` — `new(runtime)`, `runtime()`; `async create(&MachineSpec) -> Result<MachineStatus>` (unique names), `start(&MachineName)`, `stop(&MachineName, timeout_ms)`, `restart(&MachineName)`, `remove(&MachineName, force)`, `status(&MachineName) -> Result<MachineStatus>`, `list() -> Result<Vec<MachineStatus>>`.
- `MachineRegistry` — the name ↔ id map + last known `MachineStatus` cache; `new`, `len`, `is_empty`, `contains`, `id`, `get`, `insert`, `update`, `remove`, `list`, `names`.
### Host control plane (`src/host.rs`, `src/approval.rs`)
- `HostCapabilities { gpu: bool, kvm: bool, allowed_mounts: Vec<PathBuf>, publish_ports: bool }` + `Default` (grants nothing), `allows_mount(&Path)` (component-wise prefix, so `/data` does not allow `/database`), `allows_port(u16)` (all-or-nothing via `publish_ports`).
- `HostControlPlane<R>` — `new(manager, capabilities)`; `with_router(router)`, `capabilities()`, `manager()`, `machines()`/`status()`/`create()`/`start()`/`stop()`/`remove()` (delegating to the manager), `request_approval(ApprovalRequest) -> ApprovalDecision`.
- `ApprovalRequest { id: u64, machine: MachineName, capability: String, description: String }` (`new`), `ApprovalDecision { Allow, Deny { reason } }` (`is_allowed`), `#[async_trait] trait Approver: Send + Sync { async fn decide(&self, &ApprovalRequest) -> ApprovalDecision; }`, `ApprovalPolicy` (`new`, `pre_approve`, `is_pre_approved` — host-side pre-authorization that short-circuits the approver), `ApprovalRouter` (`new(Arc<dyn Approver>)`, `with_policy`, `policy`, `request(req)`); `AutoApprove`/`AutoDeny` approvers.
### Binary (`src/main.rs`, ~555 lines)
- `adesk-machine` (clap): `--runtime podman|mock` (default `podman`), `--log <FILTER>` (default `info`, `ADESK_LOG` env fallback; installs a `tracing-subscriber` env-filter); subcommands `create --name --image [--command <ARG>...] [--viewer-unix HOST:CONTAINER] [--viewer-tcp HOST:PORT] [--mount HOST:CONTAINER[:ro]]... [--memory-mb N] [--cpus N] [--network none|host|private]`, `start <NAME>`, `stop <NAME> [--timeout-ms]`, `restart <NAME>`, `remove <NAME> [--force]`, `list`, `status <NAME>`, `capabilities`.
- All subcommands drive the `HostControlPlane` (`restart` goes through `manager()`); `create` builds the spec from flags via pure `parse_mount`/`parse_viewer_unix`/`parse_viewer_tcp`/`parse_network`/`build_spec` helpers.
- Exit codes: `0` success, `1` runtime error, `2` config/CLI error.
## Constraints
- `docs/machine.md` is normative; the container engine never leaks past `ContainerRuntime`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay well under the ~1000-line threshold; split along module boundaries.
- No test may require an installed container engine, a display, GPU or network: unit tests use `MockRuntime`; the CLI backend is exercised only against a stub program via `PodmanRuntime::with_program`, never a real `podman`.
- No panics on request/command paths: every failure returns `MachineError`.
- `tracing` for lifecycle events (create/start/stop/remove); never log environment secrets or the full argv.
- Dependencies come only from root `[workspace.dependencies]` (`async-trait`, `clap`, `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`, and `tracing-subscriber` for the CLI `--log`); never inline versions.
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
| CLI wiring, flag parsers | `./src/main.rs` |
| Lifecycle-over-`MockRuntime` suite | `./tests/lifecycle.rs` |
| Spec → argv / viewer-exposure suite | `./tests/spec_translation.rs` |
| Host boundary / approval suite | `./tests/approvals.rs` |
| Stub-program CLI backend suite | `./tests/podman_stub.rs` |
## Design Decisions
- **The backend seam is the whole point.** `MachineManager` and `HostControlPlane` speak only `ContainerRuntime`, so a new engine is one trait impl; the manager never sees argv.
- **`MockRuntime` is first-class.** Because CI has no container engine, the mock is the reference implementation of the lifecycle state machine and every unit test drives it; `PodmanRuntime` is verified against a stub program for argv/spec translation and error mapping.
- **Names are the manager's key.** The registry maps unique `MachineName` → `MachineId`; a duplicate `create` fails (`Duplicate`) rather than silently replacing. `list()` returns backend truth and rebuilds the registry cache from it.
- **`MachineManager::remove` returns the status observed just before removal** (the machine is gone afterwards); unknown names are `NotFound` on every lifecycle call.
- **Viewer exposure is expressed as container options.** `ViewerExposure` translates to a bind-mount or a published port, so the container runtime stays generic and ADesk never learns about the engine.
- **`podman inspect` is the Podman backend's only status source.** It does not carry the viewer exposure, so `PodmanRuntime::status`/`list` report `ViewerExposure::None`; the manager's cached spec is authoritative for the viewer endpoint.
- **Approvals are an interface, not a policy.** `ApprovalRouter` + `Approver` let the host (and the viewer UI) decide; the crate ships `AutoApprove`/`AutoDeny` for tests and simple deployments, and `ApprovalPolicy` for host-side pre-authorization.
## Test Strategy
Runs with `./scripts/dev.sh cargo test -p adesk-machine` → 114 passed, 0 failed (74 lib unit + 15 bin unit + 25 integration), no container engine/display/GPU/network.
- In-module unit tests: model/registry/approval, `MockRuntime` lifecycle + recorded calls, `PodmanRuntime` argv/inspect parsing + error mapping, CLI flag parsers.
- `./tests/lifecycle.rs` — create/start/stop/restart/remove over `MockRuntime`, duplicate names, unknown names, state transitions, `list()` cache rebuild, recorded call sequence.
- `./tests/spec_translation.rs` — `build_create_argv` for the default and a fully-populated spec; each `NetworkMode`; viewer exposure → `--volume`/`--publish`.
- `./tests/approvals.rs` — `AutoApprove`/`AutoDeny`, pre-approval short-circuit, `HostCapabilities` mount/port matching, `HostControlPlane::request_approval`.
- `./tests/podman_stub.rs` (`#[cfg(unix)]`) — a temp `#!/bin/sh` stub through `PodmanRuntime::with_program` pins the argv and the non-zero-exit → `MachineError::Backend` mapping.
### Test-suite duplication (audit finding, current state)
- ~23 of the 74 lib unittests have a near-equivalent integration test: `src/manager.rs` tests (7/10) vs `tests/lifecycle.rs`; `src/host.rs` tests (7/9) vs `tests/approvals.rs`; `src/runtime/podman.rs` (4/13) vs `tests/spec_translation.rs` (byte-identical `build_create_argv` expectations) and (2/13) vs `tests/podman_stub.rs`; `src/approval.rs` (3/6) vs `tests/approvals.rs`.
- Two in-memory doubles model the same created→running→stopped lifecycle: production `MockRuntime` (`src/runtime/mock.rs`, used by the integration tests) and the `#[cfg(test)]`-only `StubRuntime` (`src/manager.rs` `mod testing`, used by the manager/host unit tests). `StubRuntime::seed` (register a backend-side name without the manager cache) is the one thing the lib double does that `MockRuntime` does not expose directly.
- Process-spawning stub scaffolding is duplicated: `SPAWN_LOCK` + a temp `#!/bin/sh` writer exist both in `tests/podman_stub.rs` and inline in `src/runtime/podman.rs`; the `Recording`/`RecordingDeny` approver is likewise defined in both `src/approval.rs` and `tests/approvals.rs`.
- No `#[ignore]`d tests; no wall-clock sleeps, real `podman` invocation or network in any test (all stub tests use `with_program`, never `PodmanRuntime::new`).
## Notes for Agents
- `adesk-machine` does not depend on `adesk-core` or any GUI crate; the ADesk kernel binary name is data (`MachineSpec::command`), not a dependency.
- The default `MachineSpec` describes the ADesk machine (image + command running `adesk-server` + a viewer socket mount); keep that default coherent with `docs/machine.md` §3/§5.
- This crate is an interface + a reference backend, not an orchestrator: multi-host scheduling and image building are out of scope (`docs/machine.md` §8).
- The `podman inspect` JSON the backend parses is an array of `{ "Id", "Name", "Image", "Created", "State": { "Status", "Pid", "ExitCode" } }`; `State.Status` (`created`/`running`/`stopped`/`exited`/`dead`) maps onto `MachineState`. `tests/podman_stub.rs` emits exactly this shape — keep the two in sync.
- `MachineStatus::created_at_ms` is monotonic ms from `MockRuntime`'s injectable clock; for `PodmanRuntime` it is a best-effort epoch-derived value parsed from `Created`.
- Process-spawning tests must fully write+close a stub script before spawning and serialize spawning (a process-wide mutex): writing then immediately exec'ing a just-written script can hit `ETXTBSY` ("Text file busy").
## Status
Implementation-complete and green: `error`, `spec`, `state`, `runtime` (trait + `RuntimeKind`), `runtime::mock`, `runtime::podman`, `registry`, `manager`, `host`, `approval` and the `adesk-machine` CLI are all implemented, documented and tested; `src/lib.rs` re-exports the whole surface at the crate root.
`cargo test -p adesk-machine` = 114 passed / 0 failed; `cargo clippy -p adesk-machine --all-targets --no-deps -- -D warnings`, `cargo fmt -p adesk-machine --check` and `cargo doc -p adesk-machine --no-deps --document-private-items` are all clean.
No `todo!()`/`unimplemented!()`, no crate-level `allow`, no behavioural test skips.
