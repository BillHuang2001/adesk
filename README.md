# ADesk

An **AI-native headless Wayland runtime**: a Smithay-based compositor whose primary
client is a multimodal GUI agent rather than a human.

Instead of driving a desktop through external screenshots and input automation, agents
talk to the compositor directly over a Unix socket using the **Agent GUI Protocol**
(AGP): discover and launch applications by desktop-file id, manage windows, inject real
Wayland input, and — the point of the design — request *temporal observations* that
describe what happened after a specific action (commits, damage regions, focus changes,
new windows, and one selected rendered frame) without any continuous screenshot loop.

- One virtual output, exactly one visible toplevel tiled to fill it (window-management
  policy, not an architectural limit).
- Application registry over XDG `.desktop` files: `list_apps`, `get_app`, `launch_app`.
- Runtime-native operations (`activate_window`, `close_window`) mutate compositor state
  directly; only real application input goes through the Wayland seat.
- Selective rendering: compositor state is retained; GPU readback happens only when an
  observation or inspection frame is requested. Software (pixman) path for headless CI.
- Human inspection is an optional projection: full-output rendering with debug overlays.

## Layout

See `CONTEXT.md` for the workspace map and `docs/` for the protocol spec
(`docs/protocol.md`) and internal architecture (`docs/architecture.md`).

## Building and testing

All builds and tests must run inside the Nix dev shell, which provides the system
libraries smithay needs (libxkbcommon, pixman, EGL/GLES, libwayland):

```sh
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo run -p adesk-server -- --help
```

## Status

Bootstrapping. Architecture and public API are defined crate-by-crate; implementation
is in progress. Each crate's `CONTEXT.md` states what is complete.
