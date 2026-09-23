# ADesk

An **AI-native headless Wayland runtime**: a Smithay-based compositor whose primary
client is a multimodal GUI agent rather than a human.

ADesk is developed entirely by AI agents orchestrated with
[Genesis](https://github.com/EMI-Group/genesis).

## What it is

ADesk runs a real Wayland display with **no physical screen and no human at the
controls**. Applications think they are talking to an ordinary desktop; in reality the
only client that matters is a GUI agent, which talks to the runtime directly over a Unix
socket using the **Agent GUI Protocol** (AGP). There is no screenshot-and-click loop:
the runtime keeps compositor state and hands the agent *temporal observations* — what
changed after a specific action (commits, damage regions, focus changes, new windows,
and one selected rendered frame).

- Discover and launch applications by desktop-file id; manage windows; inject real
  Wayland input.
- Exactly one visible toplevel is tiled to fill a single virtual output (a
  window-management *policy*, not an architectural limit).
- Runtime-native operations (`activate_window`, `close_window`) mutate compositor state
  directly; only genuine application input goes through the Wayland seat.
- Rendering is on demand: buffers are retained as state, and a readback happens only
  when an observation or inspection frame is requested.
- Beyond pixels, the runtime is a **programmable event source** and a **text sensor**: an
  agent can idle on a notification/event inbox and read a window's toolkit accessibility
  tree, and a human can watch and record the desktop over the VAP viewer protocol.

## What it is for

- Run a complete virtual AI assistant inside a container with full desktop access.
- Automate arbitrary GUI tasks safely, fully isolated from the host desktop.
- Drive ordinary Linux GUI apps on a headless machine (CI, servers, sandboxes).

Because the runtime is self-contained and exposes only a local socket, an agent gets a
real desktop without ever touching the host's display, input devices or session.

## How to run ADesk

Build and run everything through the Nix dev shell wrapper — it provides the system
libraries (libxkbcommon, pixman, EGL/GLES, libwayland) that the compositor links
against:

```sh
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo run -p adesk-server
```

The `adesk-server` binary accepts:

| Flag | Default | Meaning |
|---|---|---|
| `--socket <PATH>` | see below | AGP Unix socket path |
| `--output <WxH>` | `1280x800` | Virtual output size |
| `--renderer auto\|gl\|pixman` | `auto` | `pixman` forces the software path (GPU-less containers) |
| `--accessibility auto\|off` | `auto` | Accessibility backend; `off` never touches D-Bus |
| `--viewer-socket <PATH>` | AGP sibling | Viewer (VAP) Unix socket |
| `--viewer-tcp <HOST:PORT>` | — | Opt-in TCP transport for the viewer |
| `--no-viewer` | — | Disable the viewer (VAP) endpoint |
| `--recordings-dir <DIR>` | `<socket dir>/adesk-recordings` | Directory for default-named recordings |
| `--apps-dir <DIR>` | — | Extra `.desktop` search dirs (repeatable) |
| `--log <FILTER>` | `info` | `tracing-subscriber` env-filter directive |
| `--xkb-layout/-variant/-model/-rules` | — | Keyboard setup |

Every flag has an `ADESK_*` environment fallback (`ADESK_SOCKET`, `ADESK_OUTPUT`,
`ADESK_RENDERER`, `ADESK_ACCESSIBILITY`, `ADESK_VIEWER_SOCKET`, `ADESK_VIEWER_TCP`,
`ADESK_RECORDINGS_DIR`, `ADESK_APPS_DIR`, `ADESK_LOG`, `ADESK_XKB_*`), so the binary
is service/container friendly. Example invocations:

```sh
# Software-only, custom output and socket.
./scripts/dev.sh cargo run -p adesk-server -- \
    --renderer pixman --output 1600x1000 --socket /tmp/adesk.sock

# Equivalent via the environment.
ADESK_RENDERER=pixman ADESK_OUTPUT=1600x1000 ./scripts/dev.sh cargo run -p adesk-server
```

The default AGP socket path is resolved identically by server and client:

1. `$ADESK_SOCKET`, else
2. `$XDG_RUNTIME_DIR/adesk.sock`, else
3. `<system temp dir>/adesk.sock`.

## Plugging in an agent

**Transport.** AGP is plain NDJSON over a local Unix socket — no TLS and no
authentication; isolation comes from filesystem permissions on the socket path. The
client speaks first. `ping` returns the runtime and protocol version, the renderer in
use, and the output size. The server dispatcher is total over the 37 methods of
`docs/protocol.md`; clients must not invent methods or fields outside it.

**Rust client SDK** (`adesk-client`, async/tokio). Connect with
`Client::connect_default().await?` or `Client::connect("/tmp/adesk.sock").await?`, then
call typed async methods such as `list_apps`, `launch_app`, `list_windows`,
`activate_window`, `observe`, `wait_for_quiet`, `click` and `type_text`, the notification
methods (`post_notification`, `list_notifications`, `close_notification`,
`invoke_notification_action`), the idle primitive `wait_for_events`, and the
accessibility view (`accessibility_tree`, `find_accessible`, `invoke_accessible_action`).
For a stream of typed `RuntimeEvent`s use `subscribe_events(EventFilter)`, which returns
an `EventStream`.

```rust
use adesk_client::{ClickRequest, Client, EventFilter, ObserveRequest};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), adesk_client::ClientError> {
// $ADESK_SOCKET, else $XDG_RUNTIME_DIR/adesk.sock, else <temp dir>/adesk.sock
let client = Client::connect_default().await?;

let info = client.ping().await?; // runtime/protocol version, renderer, output size

let apps = client.list_apps(None, false).await?;   // discover installed apps
let launched = client.launch_app(&apps[0].id, &[]).await?; // spawn by desktop-file id

let windows = client.list_windows().await?;
if let Some(window) = windows.windows.first() {
    let action_id = client.activate_window(window.id).await?;

    // Observations describe causal history *after* an action — never sleeps.
    let obs = client
        .observe(ObserveRequest::quiet(250).window(window.id).after_action(action_id))
        .await?;
    println!("{} commit(s), quiet={}", obs.observation.commits, obs.observation.quiet);

    let _ = client.click(ClickRequest::window(window.id)).await?;
    let _ = client.type_text("hello", Some(window.id)).await?;
}

// Stream typed events (unsubscribed when the stream is dropped).
let mut events = client.subscribe_events(EventFilter::all()).await?;
if let Some(event) = events.next().await {
    println!("{event:?}");
}
let _ = launched;
Ok(())
}
```

**Built-in agent prototype** (`adesk-agent`): a provider-agnostic multimodal LLM agent
that connects to a running server over the socket and runs a plan/act/observe loop.

```sh
# Network-free dry run against a running server.
./scripts/dev.sh cargo run -p adesk-agent -- --provider mock --scenario click

# Drive a real OpenAI-compatible endpoint.
./scripts/dev.sh cargo run -p adesk-agent -- \
    --provider openai --task "Open the settings dialog and enable dark mode" \
    --max-steps 30 --report runs/dark-mode.json
```

CLI flags: `--socket`, `--provider <mock|dummy|openai>`, `--task "<goal>"` **or**
`--scenario <id>` (built-ins: `launch`, `activate`, `click`, `type`, `scroll`, `dialog`,
`navigation`, `error-recovery`), `--max-steps`, `--report <PATH>`, `--model`,
`--base-url`, `--api-key`, `--max-dimension`, `--quiet-ms`, `--include-accessibility`
(let the agent read a window's UI as a text outline via `accessibility_tree` instead of a
screenshot), plus `--watch` (documented below). Provider configuration also reads
`ADESK_AGENT_PROVIDER`, `ADESK_AGENT_MODEL`, `ADESK_AGENT_BASE_URL` and
`ADESK_AGENT_API_KEY` (falling back to `OPENAI_API_KEY`). The `openai` provider speaks
any OpenAI-compatible `/chat/completions` endpoint; `dummy` is a synthetic no-I/O VLM
configured by `--dummy-mode <fixed|random>`, `--dummy-seed`,
`--dummy-finish-probability` and `--dummy-step-budget`; `mock` is the network-free
default.

**Watch mode.** `--watch` turns the agent into an idle listener: it validates the runtime
once, then loops `wait_for_events` → feeds the wake events into the LLM context → runs the
standing `--task` body → back to idle, so it parks on the runtime's event inbox instead of
polling or sleeping. It is bounded by `--watch-timeout-ms` (a single idle wait),
`--watch-max-wakeups` and `--watch-max-idle-waits` (`0` = unbounded).

```sh
# Stay idle and handle the notification each time one arrives.
./scripts/dev.sh cargo run -p adesk-agent -- --provider mock \
    --watch --task "Handle the notification" --watch-max-wakeups 5
```

## Notifications and reactive events

The runtime is also a **programmable event source**, so an agent can stay idle by default
and be woken when a notification or a runtime event (a user message, a task handoff)
arrives. `adesk-notify` holds a synchronous notification store plus a reactive event
inbox fed by the one event pump.

- Four AGP §5.9 methods expose the store: `post_notification`, `list_notifications`,
  `close_notification` and `invoke_notification_action`. Each mutation publishes a
  `notification` / `notification_closed` / `notification_action` `RuntimeEvent` on the
  compositor's broadcast, so the existing `subscribe_events` fan-out covers it.
- `wait_for_events` (AGP §5.10) is the pull counterpart of a push subscription — one
  request that answers once — so an idle agent parks on the inbox instead of polling or
  sleeping. `adesk-agent --watch` (above) is the built-in consumer.

Design record: `docs/notifications.md`.

## Accessibility (text view)

`adesk-a11y` gives the agent a **text** observation of a window alongside the pixel one.
It is an AT-SPI2 client (the workspace's only D-Bus speaker) that reads a window's toolkit
accessibility tree, correlates windows to accessible applications, and resolves element
handles to `AccessibleId`s, so an agent can read what a window *contains* — labels, buttons,
text fields, lists, menus — without looking at a pixel.

- Three AGP §5.11 methods: `accessibility_tree`, `find_accessible` and
  `invoke_accessible_action` (which actuates an element through the toolkit —
  runtime-native actuation, not synthesized input).
- `--accessibility auto|off` (env `ADESK_ACCESSIBILITY`) selects the backend: `auto`
  (default) connects lazily on first use and degrades to `not_supported`; `off` never
  touches D-Bus. With no backend, every §5.11 method answers `not_supported`.
- The view is read strictly on request — there is no accessibility event stream — and the
  one service lives off the compositor thread, owning no compositor state.

Design record: `docs/accessibility.md`.

## Running in a container

The repository does **not** ship a container image, Dockerfile or CI today, but the
runtime is explicitly designed for headless, GPU-less containers. To containerize it:

- Force the software renderer with `--renderer pixman` (or rely on `auto`'s EGL→pixman
  fallback).
- Provide a writable `XDG_RUNTIME_DIR` (the Wayland socket lives there).
- Install the system libraries: libxkbcommon + xkeyboard-config, pixman, wayland +
  wayland-protocols, libdrm/libgbm, libinput and libudev — plus libEGL/libGLESv2/Mesa
  `llvmpipe` **only** if you use the GL path.
- Set `XKB_CONFIG_ROOT` to the xkeyboard-config data dir; keyboard setup fails without it.

The dev shell (`flake.nix`) is the reference for the exact package set. An illustrative
sketch (not shipped, not complete):

```dockerfile
FROM debian:stable-slim
RUN apt-get update && apt-get install -y \
    libxkbcommon0 xkb-data libpixman-1-0 libwayland-client0 \
    libwayland-server0 libdrm2 libgbm1 libinput10 libudev1
COPY target/release/adesk-server /usr/local/bin/
ENV XDG_RUNTIME_DIR=/run/adesk XKB_CONFIG_ROOT=/usr/share/X11/xkb
RUN mkdir -p /run/adesk
CMD ["adesk-server", "--renderer", "pixman", "--socket", "/run/adesk/adesk.sock"]
```

While no container image or CI ships, Nix packaging does: `flake.nix` exposes
`devShells.default` (the dev shell), `packages.<system>.{default,adesk}` (a
`rustPlatform.buildRustPackage` that installs the `adesk-server`, `adesk-viewer`,
`adesk-viewer-gui`, `adesk-machine` and `adesk-agent` binaries) and
`nixosModules.{default,adesk}`, a systemd service module (`nix/adesk-module.nix`) that runs
`adesk-server` with an optional companion agent.

## Inspecting as a human

Two independent paths let a human observe the desktop.

**Viewer (VAP).** The runtime serves a purpose-built viewer protocol (VAP v1,
implemented by `adesk-viewer-proto`) on a second listener. Two clients ship in the
workspace; both are pure VAP clients, so the human ends up in the **same seat the agent
drives** (a viewer action is never a special code path):

- `adesk-viewer` — headless: it captures frames to PNG, replays a scripted input stream,
  and records the desktop (`--record <FILE>`).
- `adesk-viewer-gui` — the interactive GTK4/libadwaita front-end (the workspace's only GTK
  crate): it streams the desktop into a window, adds a window task bar whose clicks switch
  windows through the runtime-native VAP `activate_window` message, routes
  pointer/key/scroll/text through the same seat path, and carries a record toggle in its
  header.

```sh
# One-shot capture of the current desktop to a PNG.
./scripts/dev.sh cargo run -p adesk-viewer -- --capture desktop.png

# Follow the desktop, writing every frame, and replay a small input script.
./scripts/dev.sh cargo run -p adesk-viewer -- --follow --out-dir shots/ --input script.txt

# Watch the desktop interactively (needs a real display).
./scripts/dev.sh cargo run -p adesk-viewer-gui -- --unix /run/adesk/adesk-viewer.sock
```

The viewer listener is a Unix socket by default at the AGP socket's sibling
(`$XDG_RUNTIME_DIR/adesk-viewer.sock`); `--viewer-tcp <HOST:PORT>` exposes a TCP
transport for a remote viewer and `--no-viewer` disables it. Viewer input (pointer,
click, scroll, key, text) is applied through the **same seat path** as agent input and
returns a normal AGP `action_id`. VAP is specified in `docs/viewer.md`.

**Screen recording.** The same VAP connection can start and stop a recording of the
desktop: the runtime adds the `start_recording` / `stop_recording` / `request_recording`
client messages and the `recording` server message, backed by `adesk-recorder`. It
always has a pure-Rust Motion-JPEG/AVI software backend that works headless, plus an
optional GPU-accelerated H.264 backend that shells out to `ffmpeg` with a hardware
encoder (selected only when one is actually available). `adesk-viewer --record <FILE>`
records from the CLI and the GUI has a record toggle; a recording started without an
explicit path lands in `--recordings-dir` / `ADESK_RECORDINGS_DIR`. Recording is
runtime-scoped and captures on demand, so the on-demand-rendering invariant holds.

**Inspector overlays (AGP).** Debug overlays remain AGP-only:

- `inspect_capture` returns **one** PNG of the composed output with debug overlays.
- `inspect_subscribe` pushes a throttled stream of `inspect_frame` PNG events.

Overlays are selectable: `window_ids`, `app_ids`, `focus`, `damage`, `surface_bounds`,
`cursor`, `actions`, `commit_timing` (default `window_ids` + `focus` + `damage`). The SDK
mirrors these as `Client::inspect_capture(...)` and `Client::inspect_subscribe(...)`.
Inspection overlays are human-only and never appear in the agent's own capture images.

## Assistant runtime (AI Machine)

`adesk-machine` provides the host-side runtime that gives each assistant a Linux machine
it fully owns — root inside, systemd as PID 1, Nix for software — inside a **rootless
container**, while the host keeps control of the boundary (lifecycle, networking,
exposed resources, approvals). ADesk runs *inside* the machine; the viewer runs
*outside* it. The container engine is an abstraction: rootless **Podman** is the first
backend, with an in-memory `mock` backend for tests and development.

```sh
# Create/start an ADesk machine over rootless Podman, then inspect it.
./scripts/dev.sh cargo run -p adesk-machine -- --runtime podman create \
    --name assistant --image registry.local/adesk:latest --viewer-unix /run/adesk/adesk-viewer.sock:/run/adesk/adesk-viewer.sock
./scripts/dev.sh cargo run -p adesk-machine -- --runtime podman start assistant
./scripts/dev.sh cargo run -p adesk-machine -- --runtime podman list

# Everything works without a container engine via the mock runtime.
./scripts/dev.sh cargo run -p adesk-machine -- --runtime mock list
```

The design (division of responsibility, the backend seam, viewer exposure over
mounts/ports, approvals, trust model) is `docs/machine.md`.

## Layout and docs

- `CONTEXT.md` — workspace map and cross-crate contracts.
- `docs/protocol.md` — the normative AGP specification.
- `docs/architecture.md` — threading, channels, render pipeline, observation semantics.
- `docs/core-api.md` — the domain model (`adesk-core`).
- `docs/viewer.md` — the normative Viewer Attachment Protocol (VAP v1), including recording.
- `docs/machine.md` — the AI Machine runtime and host control plane.
- `docs/notifications.md` — the notification subsystem and the agent event inbox.
- `docs/accessibility.md` — accessibility, the agent's text view of a window's UI.

## Status

Implementation-complete. All 19 crates are implemented and independently audited, with
no executable `todo!()`/`unimplemented!()` in the workspace and no crate-level `allow`
attributes. The workspace builds cleanly and passes `clippy` (warnings denied), `fmt`
and rustdoc (zero warnings); `cargo test --workspace --no-fail-fast` reports 1713 passed,
0 failed, 5 ignored (the 5 ignored are doc-code fences only). The feature-gated agent
suite (`cargo test -p adesk-agent --features test-support,e2e`) is 146 passed, and
`adesk-testkit`'s suite is 77 passed / 3 ignored doc-fences.
