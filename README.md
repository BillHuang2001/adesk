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
| `--apps-dir <DIR>` | — | Extra `.desktop` search dirs (repeatable) |
| `--log <FILTER>` | `info` | `tracing-subscriber` env-filter directive |
| `--xkb-layout/-variant/-model/-rules` | — | Keyboard setup |

Every flag has an `ADESK_*` environment fallback (`ADESK_SOCKET`, `ADESK_OUTPUT`,
`ADESK_RENDERER`, `ADESK_APPS_DIR`, `ADESK_LOG`, `ADESK_XKB_*`), so the binary is
service/container friendly. Example invocations:

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
use, and the output size.

**Rust client SDK** (`adesk-client`, async/tokio). Connect with
`Client::connect_default().await?` or `Client::connect("/tmp/adesk.sock").await?`, then
call typed async methods such as `list_apps`, `launch_app`, `list_windows`,
`activate_window`, `observe`, `wait_for_quiet`, `click` and `type_text`. For a stream of
typed `RuntimeEvent`s use `subscribe_events(EventFilter)`, which returns an `EventStream`.

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

CLI flags: `--socket`, `--provider <mock|openai>`, `--task "<goal>"` **or**
`--scenario <id>` (built-ins: `launch`, `activate`, `click`, `type`, `scroll`, `dialog`,
`navigation`, `error-recovery`), `--max-steps`, `--report <PATH>`, `--model`,
`--base-url`, `--api-key`, `--max-dimension`, `--quiet-ms`. Provider configuration also
reads `ADESK_AGENT_PROVIDER`, `ADESK_AGENT_MODEL`, `ADESK_AGENT_BASE_URL` and
`ADESK_AGENT_API_KEY` (falling back to `OPENAI_API_KEY`). The `openai` provider speaks
any OpenAI-compatible `/chat/completions` endpoint; `mock` is the network-free default.

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

## Inspecting as a human

There is no viewer binary or UI. A human uses an AGP client over the same socket:

- `inspect_capture` returns **one** PNG of the composed output with debug overlays.
- `inspect_subscribe` pushes a throttled stream of `inspect_frame` PNG events.

Overlays are selectable: `window_ids`, `app_ids`, `focus`, `damage`, `surface_bounds`,
`cursor`, `actions`, `commit_timing` (the default set is `window_ids` + `focus` +
`damage`). The SDK mirrors these as `Client::inspect_capture(...)` and
`Client::inspect_subscribe(...)`. The image comes back as a base64 PNG (`ImagePayload`)
over the socket, so the human must decode and save it to view it. Inspection overlays are
human-only and never appear in the agent's own capture/observation images.

## Layout and docs

- `CONTEXT.md` — workspace map and cross-crate contracts.
- `docs/protocol.md` — the normative AGP specification.
- `docs/architecture.md` — threading, channels, render pipeline, observation semantics.
- `docs/core-api.md` — the domain model (`adesk-core`).

## Status

Implementation-complete. All 12 crates are implemented and independently audited, with
no executable `todo!()`/`unimplemented!()` in the workspace and no crate-level `allow`
attributes. The workspace builds cleanly and passes `clippy` (warnings denied), `fmt`
and rustdoc (zero warnings); `cargo test --workspace` reports 1230 passed, 0 failed.
