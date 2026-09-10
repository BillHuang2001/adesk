# ADesk — AI Machine runtime and host control plane — normative design

This document defines how an assistant gets a Linux machine it fully owns, and
how the host keeps control of the boundary. `adesk-machine` implements it.

Central abstraction:

> An AI-owned Linux machine running inside a lightweight container, with a
> remotely inspectable ADesk desktop.

## 1. Division of responsibility

```text
Host ─ Host control plane (adesk-machine::HostControlPlane + MachineManager)
        │  create/delete · start/stop/restart · networking · exposed resources
        │  approval routing · machine state
        ▼
AI Machine ─ rootless container (systemd as PID 1, Nix, root for the AI)
        │
        ├── AI agent  (shell · filesystem · systemd · Nix)
        └── ADesk     (compositor · GUI control · rendering · inspection)
                        │
                        │ VAP (docs/viewer.md) over a chosen transport
                        ▼
                      Viewer (outside the machine; a window into the desktop)
```

- The **AI owns the guest**: root inside the machine, normal Linux interfaces,
  Nix for software, systemd for services. The host provides **no** APIs for
  operations the AI can perform inside its own environment.
- The **host owns the boundary**: machine lifecycle, networking, which host
  resources are exposed, and approvals for capabilities that cross the boundary.
- The **runtime is an implementation detail**: the machine, ADesk, the viewer and
  the manager never depend on a particular container engine.

## 2. Container runtime abstraction

`adesk-machine::runtime::ContainerRuntime` is the backend seam. The machine and
the host control plane speak only this trait.

```rust
#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    async fn create(&self, spec: &MachineSpec) -> Result<MachineId>;
    async fn start(&self, id: &MachineId) -> Result<()>;
    async fn stop(&self, id: &MachineId, timeout_ms: u64) -> Result<()>;
    async fn remove(&self, id: &MachineId, force: bool) -> Result<()>;
    async fn status(&self, id: &MachineId) -> Result<MachineStatus>;
    async fn list(&self) -> Result<Vec<MachineStatus>>;
}
```

Backends:

- **`PodmanRuntime`** — the first backend. Rootless Podman driven through the
  `podman` command line (`create`/`start`/`stop`/`rm`/`inspect`). The program
  path is configurable so the backend can be exercised against a stub in tests;
  it is never required for the crate's own unit tests.
- **`MockRuntime`** — an in-memory, deterministic backend used by every unit
  test and by `adesk-machine --runtime mock`. It models the same lifecycle state
  machine and records the calls it received.

A future `systemd-nspawn` backend is added by implementing the same trait; no
other crate changes.

## 3. Machine model

- `MachineId` — stable, opaque id assigned by the backend.
- `MachineName` — the human/agent-facing name (unique within a manager).
- `MachineSpec` — everything needed to create a machine: `name`, `image`,
  `command`, environment, mounts, resource limits, network mode, and the viewer
  exposure (see §5). `MachineSpec::default()` describes a machine that runs ADesk.
- `MachineState` — `Created`, `Running`, `Stopped`, `Exited { code }`,
  `Failed { message }`. `MachineStatus` carries the id, name, image, state, an
  optional container pid, a created timestamp and the viewer endpoint when known.
- `MachineRegistry` — the name ↔ id map plus the last known `MachineStatus` per
  machine; the manager's cache of backend truth.

## 4. Host control plane

`MachineManager` owns the machine lifecycle over a `ContainerRuntime`:

- `create(spec)` → `MachineId`, `start(name)`, `stop(name, timeout)`,
  `restart(name)`, `remove(name, force)`, `status(name)`, `list()`.
- Names are unique; a duplicate `create` fails rather than silently replacing.
- The manager never exposes an API for an operation the AI can perform inside
  its machine.

`HostControlPlane` is the boundary the host exposes:

- `HostCapabilities` — what the host can give a machine: GPU passthrough, KVM,
  which host paths may be mounted, whether published ports are allowed.
- Machine state queries (delegating to the manager).
- Approval routing (see §6).

## 5. Networking and viewer exposure

The viewer runs outside the machine, so ADesk's viewer transport must cross the
boundary. `ViewerExposure` describes how:

- `UnixSocket { host_path, container_path }` — the local case: a socket bind-mount.
- `TcpPort { host_port, container_port }` — the remote case: a published port.

Both are expressible as ordinary container options (mounts / port publishes), so
the container runtime stays generic.

## 6. Approvals

Actions fully contained in the machine belong to the AI; actions that cross the
boundary may need mediation.

```text
AI Machine ── request external capability ──▶ Host control plane
                                                   │ allowed  ─▶ execute
                                                   └ approval ─▶ Viewer (allow/deny)
```

`ApprovalRequest` names the machine, the requested capability and a description;
`ApprovalDecision` is `Allow` or `Deny`. `ApprovalRouter` routes a request to an
`Approver` and awaits a decision. The viewer is a natural place to surface and
answer approval requests; that wiring lives above ADesk and outside the VAP.

## 7. Trust model

The AI is trusted to own its machine, not the host. The machine boundary is
enforced by the container plus the host control plane; the viewer stays outside
the boundary and only receives what ADesk and the host explicitly expose. The
system isolates the AI's working environment from the host; it does not sandbox
deliberately malicious software.

## 8. Out of scope (v1 of the machine runtime)

- Real rootfs/image building (the spec names an image; building it is a Nix /
  container concern outside this crate).
- Multi-host orchestration, scheduling, migration.
- A production approval UI (the router is an interface; the viewer/host UI uses it).
