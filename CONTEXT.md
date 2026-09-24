# ADesk — AI-native headless Wayland runtime

## Intent

ADesk is a headless Wayland compositor built with Smithay whose *primary* client is a
multimodal GUI agent, not a human. It exposes application discovery/launch, window
management, input injection, and — most importantly — a native **temporal observation**
interface over a Unix socket (the Agent GUI Protocol, AGP). A human-visible desktop is
only an optional projection of the runtime's internal state.

Central principle: the runtime is the agent's GUI sensor, actuator, window manager and
temporal observer at once. It must never need an external screenshot loop.

Design invariants:

1. **One visible toplevel.** Exactly one normal `xdg_toplevel` is visible at a time and
   is tiled to fill the whole virtual output. This is a *window-management policy*
   (`adesk-wm`), not an architectural limit — multi-window support must be addable
   without restructuring the compositor.
2. **Coordinates are window-relative** (pixels or normalized `0..1`); the window model,
   never hard-coded constants, converts them to output coordinates.
3. **Runtime-native operations are never synthesized input.** `activate_window` changes
   compositor state directly; only `click`/`keypress`/... go through the Wayland seat.
4. **Surface activity is known, semantic completion is inferred.** Commits/damage are
   ground truth; "quiet" is evidence the agent must reason about, never a promise.
5. **Rendering is on demand.** Buffers are retained as compositor state; a GPU
   readback happens only when an observation or an inspection frame needs pixels.
6. **Every action has an ID.** Observations describe causal history relative to an
   `action_id`, not wall-clock sleeps.

## System overview

```text
 multimodal agent (adesk-agent)          tests / tools
            │  AGP over Unix socket           │
            ▼                                 ▼
 ┌──────────────────────────────────────────────────────┐
 │ adesk-server  (tokio)                                │
 │  socket listener · request dispatch · action registry │
 │  adesk-observer (temporal state, wait_for_*)          │
 │  adesk-app-registry (XDG .desktop → launch)           │
 └───────▲──────────────────────────────┬───────────────┘
         │ RuntimeEvent (broadcast)     │ RuntimeCommand (channel)
 ┌───────┴──────────────────────────────▼───────────────┐
 │ adesk-compositor thread (calloop, single-threaded)   │
 │  wayland protocols · seat/input · virtual output     │
 │  adesk-wm (window model, tiling policy)              │
 │  adesk-render (offscreen render, damage, readback)   │
 └──────────────────────────────────────────────────────┘
         │ Wayland (socket)
   ordinary Linux GUI applications (SHM / DMA-BUF)
```

## Workspace layout

| Crate | Responsibility |
|---|---|
| `crates/adesk-core/` | Domain model: ids, geometry, window/app info, `RuntimeEvent`, umbrella error. No I/O. |
| `crates/adesk-proto/` | AGP wire protocol: frame/request/response/event types, JSON codec. Implements `docs/protocol.md`. |
| `crates/adesk-app-registry/` | XDG `.desktop` discovery, parsing, `Exec` field-code expansion, process launch, app↔window correlation. |
| `crates/adesk-wm/` | Window model + single-visible-toplevel tiling policy + focus state machine. Pure logic, no Smithay. |
| `crates/adesk-observer/` | Temporal observation engine: per-window commit/damage history, quiet detection, action correlation, waiters. |
| `crates/adesk-notify/` | Notification store (post/close/invoke lifecycle) + reactive event inbox: bounded journal, `wait_for_events` waiters, the idle agent's wake source. No Smithay/IO. |
| `crates/adesk-render/` | Offscreen render pipeline: render target, damage regions, crop/downsample, readback, image encoding, software path. |
| `crates/adesk-compositor/` | Smithay integration: protocol handlers, event loop, virtual output, seat/input injection, event emission. |
| `crates/adesk-inspector/` | Human inspection: full-output composition, debug overlays, inspection frames. |
| `crates/adesk-server/` | Binary + lib: composes everything, owns the AGP Unix socket and the VAP viewer listener, and dispatching every AGP method. |
| `crates/adesk-client/` | Async Rust client SDK for AGP (typed methods + event stream). |
| `crates/adesk-agent/` | Multimodal agent prototype: provider-agnostic LLM interface, context assembly, metrics, task scenarios. |
| `crates/adesk-testkit/` | Dev-only test harness: in-process runtime, Wayland test client, fixtures, image assertions. |
| `crates/adesk-viewer-proto/` | Viewer Attachment Protocol (VAP) v1: viewer↔runtime wire messages, codec. No I/O. |
| `crates/adesk-viewer/` | Viewer: desktop-streaming server session + async client SDK + headless `adesk-viewer` binary. |
| `crates/adesk-viewer-gui/` | GTK4/libadwaita desktop viewer front-end: frame view, remote cursor, app launcher, window task bar, human input over VAP. The only GTK crate. |
| `crates/adesk-machine/` | AI Machine runtime: rootless-container backend seam, machine lifecycle manager, host control plane. |
| `crates/adesk-recorder/` | Screen recording: rendered desktop frames → encoded video file. Pure-Rust Motion-JPEG/AVI software backend (works with no GPU/display/external tool) + optional GPU-accelerated H.264 backend (external `ffmpeg` with a hardware encoder). No compositor/protocol/async coupling — `adesk-server` drives it. |
| `crates/adesk-a11y/` | Accessibility subsystem: AT-SPI2 client over D-Bus (no compositor/protocol coupling), window↔accessible correlation, an element-handle → `AccessibleId` registry, and the text renderer. |

End-to-end tests live in `crates/adesk-server/tests/`; protocol-level compositor tests in
`crates/adesk-compositor/tests/`. Both build on `adesk-testkit`.

## Assistant runtime (AI Machine + Viewer)

Beyond the GUI runtime, the workspace grows toward the assistant-runtime design in
`docs/machine.md` and `docs/viewer.md`: an AI-owned Linux machine inside a rootless
container (`adesk-machine`) with ADesk running inside it, and a Viewer outside it that
observes the desktop and provides limited human input over a purpose-built protocol
(VAP, `adesk-viewer-proto` / `adesk-viewer`). ADesk's viewer endpoint lives in
`adesk-server` (`src/viewer/`) and reuses the same seat/input path as agent input; the
`adesk-viewer` binary is a headless VAP client (frames → PNG, scripted input), while
`adesk-viewer-gui` is the interactive GTK4/libadwaita front-end (a desktop window with a
drawn remote cursor plus focus/help affordances, a window task bar, and human
pointer/key/text routed through the same VAP seat path).

VAP also carries application and window management, so a remote human can operate the
desktop rather than just watch it: `list_apps` → `apps`, `launch_app` → `launch_result`,
`close_window`, alongside the runtime-native `activate_window`. The server implements
these by calling the same AGP dispatch functions the agent's calls use
(`dispatch/apps.rs`, `dispatch/windows.rs`), so a viewer gains no authority beyond the
pointer/keyboard control it already holds; `control` ownership stays advisory.

Screen recording rides the same VAP connection: the runtime can start/stop a recording
of the desktop and write it to a video file, driven by `adesk-recorder` — a pure-Rust
Motion-JPEG/AVI software backend that always works headless, plus an optional
GPU-accelerated H.264 backend that shells out to `ffmpeg` with a hardware encoder
(VA-API → NVENC → V4L2, `libx264` fallback). VAP gains the `start_recording` /
`stop_recording` / `request_recording` client messages and the `recording` server
message; the headless `adesk-viewer` binary has a `--record <FILE>` mode and
`adesk-viewer-gui` a record toggle. Recording is runtime-scoped (not connection-scoped)
and captures on demand, so the on-demand-rendering invariant holds.

## Notifications & reactive events (idle-agent wakeups)

The runtime is also a **programmable event source** for the agent: an agent can stay
idle by default and be awakened when an application notification or a runtime event
(a user message, a task handoff) arrives. `docs/notifications.md` is the design record
and `docs/architecture.md` §11 the subsystem contract.

- **One store, one stream.** `adesk-notify` owns a synchronous notification store
  (mutated only by the §5.9 handlers) and an event inbox (fed by the one event pump).
  A §5.9 mutation reserves a `seq` (`RuntimeCommand::ReserveSeq`) and publishes a
  `notification` / `notification_closed` / `notification_action` `RuntimeEvent` on the
  compositor's broadcast, so the single global `seq` domain and the existing
  `subscribe_events` fan-out cover it with no special-casing; the observer counts none
  of them (they advance its watermark only).
- **AGP §5.9/§5.10.** Four methods (`post_notification`, `list_notifications`,
  `close_notification`, `invoke_notification_action`) expose the store;
  `wait_for_events` is the pull counterpart of a push subscription — one request that
  answers once, so an idle agent parks on the inbox generation counter instead of
  polling or sleeping. The dispatcher is total over 37 `docs/protocol.md` methods.
- **`adesk-agent` watch mode.** `AgentLoop::run_watch` (CLI `--watch`) validates the
  runtime once, then loops `wait_for_events` → feed wake events into the LLM context →
  run the one-shot body → back to idle, bounded by explicit wakeup/idle budgets (no
  busy-polling, no wall-clock sleeps).

## Accessibility (text-only desktop view)

The runtime is also the agent's **text sensor**: a window's toolkit accessibility tree
(AT-SPI2 over D-Bus) is a first-class, text-only observation alongside the pixel one, so
the agent can read what a window *contains* — labels, buttons, text fields, lists, menus —
and act on elements by `AccessibleId` without looking at a pixel. `docs/accessibility.md`
is the design record and `docs/architecture.md` §12 the subsystem contract.

- **AGP §5.11.** Three methods (`accessibility_tree`, `find_accessible`,
  `invoke_accessible_action`) return the window-scoped tree and a filtered search, and
  invoke an element's action through the toolkit (runtime-native actuation, not synthesized
  input).
- **Backend model.** `adesk-a11y` owns the AT-SPI2 client and is the only crate that
  speaks D-Bus; `auto` (default) connects lazily on first use and degrades to
  `not_supported`, `off` never touches D-Bus (`--accessibility auto|off`,
  `ADESK_ACCESSIBILITY`), and a deterministic fixture backend is injected for tests. With no
  backend, *every* §5.11 method — `invoke_accessible_action` included — answers
  `not_supported`, because availability is probed before a node id is resolved.
- **On demand, no coupling.** The one `AccessibilityService` lives in `ServerContext`, off
  the compositor thread and owning no compositor state; the event pump, the observer and
  the window model are untouched, and there are no accessibility event kinds — the view is
  read strictly on request.

## Cross-crate contracts

- **Protocol:** `docs/protocol.md` is normative. `adesk-proto` implements it exactly;
  server and client must not invent methods or fields outside it.
- **Viewer protocol:** `docs/viewer.md` is normative for VAP (v1, additive evolution).
- **System architecture & data flow:** `docs/architecture.md` (threading, channels,
  render pipeline, observation semantics, renderer selection).
- **Errors:** every crate exposes a `thiserror` enum and `pub type Result<T>`.
  `adesk_core::Error` is the umbrella used at crate boundaries and maps to the AGP
  `ErrorCode` values. No panics on request/event paths; `todo!()` may exist only in
  freshly designed API stubs and must be gone before a crate is "implemented".
- **Dependencies:** all third-party versions are declared once in the root
  `Cargo.toml` `[workspace.dependencies]`; members use `<dep>.workspace = true`.
  Adding a dependency means editing the root manifest — do not add inline versions.
- **Logging:** `tracing` everywhere. Servers install `tracing-subscriber` with
  `ADESK_LOG` (env-filter). One span per request (`request{id method}`), one per
  window lifecycle, never log pixel payloads.
- **Timestamps:** monotonic milliseconds since runtime start (`ts_ms`); no wall clock
  inside observations (reproducibility).

## Conventions

- Rust 2021, `rust-version = 1.80`, workspace version `0.1.0`.
- The whole workspace is `cargo fmt --all --check` clean under rustfmt 1.9.0 defaults
  (there is no `rustfmt.toml`); keep it that way.
- The whole workspace is rustdoc-warning-free
  (`cargo doc --workspace --no-deps --document-private-items`). Prefer plain code spans
  over links for private items, and bare `[`Item`]` over explicit targets already in scope.
- File size: ~1000 lines is the concern threshold; split along module boundaries
  rather than growing a file. Cohesive test modules may exceed it.
- Public API surface is what `CONTEXT.md` documents; keep internals `pub(crate)`.
- Renderer selection is a runtime option, not a Cargo feature: `--renderer auto|gl|pixman`
  (default `auto` tries surfaceless EGL/GL and falls back to pixman with a warning).
  Only `adesk-agent` declares Cargo features (`test-support`, `e2e`); both smithay renderer
  backends are always compiled in. The software path must always work headless — CI has no
  GPU (verified: `cargo check -p adesk-render --no-default-features` and
  `-p adesk-compositor --no-default-features` are green).
- No test may require a display, GPU, real network, or a specific installed
  application; use `adesk-testkit` fixtures.

## Build & test

System libraries (libxkbcommon, pixman, EGL/GLES, libwayland, libudev, and GTK4 +
libadwaita for `adesk-viewer-gui`) come from the
Nix dev shell. **Always** build through the wrapper:

```sh
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo clippy --workspace --all-targets
```

Bare `cargo build` fails to link outside the shell — that is expected, not a code bug.
`nix develop -c <cmd>` is equivalent.

The workspace links with the `mold` linker on Linux: `.cargo/config.toml` adds a `target.'cfg(target_os = "linux")'.rustflags` entry passing `-C link-arg=-fuse-ld=mold` to rustc, and both the dev shell and the package put `mold` on PATH (`flake.nix`).

## Packaging / deployment

- There is no container/OCI/Docker packaging and no CI config in the repo (no `Dockerfile`/`Containerfile`, `.github/`, `.gitlab-ci`, Jenkins, CircleCI, Makefile or justfile).
- The only build/dev tooling is `flake.nix` and `scripts/dev.sh` (an `exec nix develop <root> -c "$@"` wrapper). The flake exposes `devShells.default`, `packages.<system>.{default,adesk}` (a `rustPlatform.buildRustPackage` over the workspace's `Cargo.lock`, installing the `adesk-server`/`adesk-viewer`/`adesk-viewer-gui`/`adesk-machine`/`adesk-agent` binaries; its `buildInputs` include GTK4/libadwaita because the virtual-manifest build compiles the `adesk-viewer-gui` crate, and its `nativeBuildInputs` provide `mold` so the workspace's `.cargo/config.toml` linker flag resolves during the package build) and `nixosModules.{default,adesk}` (the module in `./nix/adesk-module.nix`, which runs `adesk-server` as a systemd service with an optional companion agent, exposing the server's CLI/env surface incl. `accessibility` and `recordingsDir`).
- The `adesk-server` binary runs headless with no GPU: `--renderer pixman` forces the software path; `auto` (default) probes surfaceless EGL and falls back to pixman both when GL creation fails and when the probed GL renderer is a software rasterizer (Mesa llvmpipe/softpipe/swrast/lavapipe), so `auto` never renders on software GL. `--accessibility auto|off` selects the accessibility backend (`auto` connects lazily, `off` never touches D-Bus), and `--dmabuf on|off` (default `on`) requests the `zwp_linux_dmabuf_v1` global, which is advertised only when the active renderer is a hardware GL renderer (`HeadlessRenderer::imports_dmabuf()`) — pixman and software GL are SHM-only; `off` forces the SHM-only escape hatch. All flags have `ADESK_*` env fallbacks (`ADESK_SOCKET`, `ADESK_OUTPUT`, `ADESK_RENDERER`, `ADESK_DMABUF`, `ADESK_ACCESSIBILITY`, `ADESK_APPS_DIR`, `ADESK_LOG`, `ADESK_XKB_*`, plus viewer `ADESK_VIEWER_TCP`; `ADESK_VIEWER_SOCKET` names the viewer endpoint for the server and doubles as the client fallback — default on both sides is the AGP socket's sibling, `…/adesk-viewer.sock`, see `docs/viewer.md` §1.1), so it is service/container friendly.
- Two more headless binaries ship: `adesk-viewer` (VAP client: connects to the viewer endpoint, writes frames as PNG, drives scripted input) and `adesk-machine` (host-side AI Machine lifecycle CLI over a `podman` or `mock` runtime).
- Socket path resolution: `$ADESK_SOCKET` → `$XDG_RUNTIME_DIR/adesk.sock` → `<temp_dir>/adesk.sock`; the process needs a writable `XDG_RUNTIME_DIR` (Wayland socket) at runtime.

## Known issues

- The sandbox has no GPU and no system EGL on the default library path; only the dev
  shell provides them. GL rendering uses Mesa's `llvmpipe` software fallback there.
- Client DMA-BUF import fails in this environment (`Mapping the dmabuf failed`, EPERM). A
  failed import on the `create_immed` request is answered by the protocol with a fatal
  error that disconnects the client, so `zwp_linux_dmabuf_v1` is advertised only for a
  renderer that can actually import (`HeadlessRenderer::imports_dmabuf()` = hardware GL);
  pixman and software GL are SHM-only and GTK4 falls back to `wl_shm` — no default
  configuration can kill a client this way (`crates/adesk-compositor/CONTEXT.md`).
- Software-GL detection and DMA-BUF capability are deliberately conservative: a software
  GL stack is recognized only when `GL_VENDOR` names Mesa *and* `GL_RENDERER` names
  llvmpipe/softpipe/swrast/lavapipe, and only `Gl { software_gl: false }` is trusted to
  import; an unrecognized identity is treated as hardware and keeps the DMA-BUF global
  (`crates/adesk-compositor/CONTEXT.md`).
- Launched applications are forced onto adesk's Wayland socket: `adesk-server` composes
  the child environment with `DISPLAY`/`XAUTHORITY` removed and the common Wayland
  toolkit opt-ins set (`XDG_SESSION_TYPE`, `GDK_BACKEND`, `QT_QPA_PLATFORM`,
  `MOZ_ENABLE_WAYLAND`, `OZONE_PLATFORM`), so Firefox/Chrome as well as GTK apps open
  inside adesk rather than on the host X server. A single-instance app already running on
  the host with the same profile can still delegate the launch to that host instance, so a
  test that runs an app against adesk must isolate the host session (own
  `XDG_RUNTIME_DIR`, `DISPLAY`/`XAUTHORITY`/`WAYLAND_DISPLAY` cleared, session bus
  unreachable) or the app attaches to the host compositor instead.
- `xkbcommon`'s keymap data comes from `XKB_CONFIG_ROOT` set by the dev shell; running
  the binaries outside the shell will fail keyboard setup unless that variable is set.
- `RendererKind::Auto`'s GL→pixman demotion is covered by an injectable-probe unit test
  plus an `ADESK_TEST_GL=1`-gated test that asserts a real llvmpipe is demoted to pixman.
- GTK4/libadwaita (for `adesk-viewer-gui`) come only from the dev shell / package
  (`flake.nix` `adeskGuiLibraries`); outside it the GUI crate cannot build or link.
- The GUI binary needs a real display to *run*, so its widget glue (window/popover
  assembly, controllers, overlays, task-bar buttons, `gio::Action` wiring) is untested;
  every decision behind it lives in display-free unit-tested modules (CLI, socket
  addressing, letterbox mapping, cursor placement, pointer pairing, keystroke routing,
  help/status text, task-bar model, app-list filtering + launch state, window-close
  reconciliation, image decode). The VAP connection is
  one-shot — a dropped connection shows a banner but does not auto-reconnect.
- The server does not push `state` except in reply to `request_state` (the spec's
  advisory per-change push is not implemented), so a viewer refreshes the window
  list by pulling `request_state` rather than via a server push stream (the GUI pulls
  after each action and on a short timer).
- A viewer's `launch_result` reports the launched app id only — `window_id`/`action_id`
  are absent because the reply precedes launch→window correlation — so the viewer
  discovers the new window through the next `state` refresh. `close_window` on an
  already-gone window answers `unknown_window` on the VAP error path; the GUI hides the
  row optimistically and reconciles it against the next `state` refresh, and does not
  surface that asynchronous error as a notice.
- The AGP socket and the VAP viewer socket are distinct endpoints (`docs/viewer.md` §1.1):
  a VAP client pointed at `adesk.sock` is closed as an undecodable frame — the server logs
  a WARN hint naming the viewer socket, both viewer clients resolve the sibling default and
  name the path (plus an AGP-socket hint) in their errors, and `--unix` help states the
  distinction.
- Screen recording's GPU path needs an external `ffmpeg` on `PATH` plus a hardware
  encoder (VA-API/NVENC/V4L2) and a `/dev/dri` render node; the sandbox has none, so
  `adesk-recorder`'s hardware tests are detection-gated (they early-return when the
  facility is absent) and the pure-Rust MJPEG/AVI backend is the always-available path.
  `--record-encoder auto` selects GPU only when it is actually available.
- The real AT-SPI2 path needs a running accessibility bus (an activatable `at-spi` registry)
  and accessible applications; the sandbox has only the former, so `adesk-a11y`'s real backend
  is covered by detection-gated tests, a unit-tested walk and an end-to-end mock-provider suite
  on a private `dbus-daemon` (`crates/adesk-a11y/tests/atspi_provider.rs`; skips cleanly
  without the `dbus-daemon` binary), while `adesk-testkit`'s fixture backend stays deterministic.
- Accessibility *events* are explicitly out of scope: there is no reactive accessibility
  event stream and no accessibility `EventKind` — the text view is observed only on
  request.
- `AccessibleNode::role` is the toolkit's own role name normalized to lowercase
  snake_case — a free string, not a closed enum (intentional, for forward compatibility) —
  so a consumer must not assume a fixed vocabulary.

## Status
The original 12 GUI-runtime crates are implementation-complete and independently audited: zero executable `todo!()`/`unimplemented!()` in the workspace, no crate-level `allow` attributes (only `forbid(unsafe_code)` + `deny(missing_docs)`), no behavioural test skips, and all 37 `docs/protocol.md` methods handled exactly once in the server dispatcher with no handler outside the spec.
Four new crates implement the assistant runtime: `adesk-viewer-proto` (VAP v1 wire types + codec, incl. the recording and the app/window-management messages; 50 tests), `adesk-viewer` (viewer server session + client SDK + headless `adesk-viewer` binary, incl. `--record`; 159 tests), `adesk-machine` (rootless-container backend seam, machine manager, host control plane + `adesk-machine` CLI; 84 tests) and `adesk-recorder` (software MJPEG/AVI encoder + muxer, optional `ffmpeg` hardware-H.264 backend, `Recorder`/`RecordingSession`; 33 tests).
`adesk-a11y` adds the runtime's accessibility (text) view: the AT-SPI2 client (the workspace's only D-Bus speaker), window→accessible correlation, the element-handle → `AccessibleId` registry and the outline renderer behind the three §5.11 methods, backed by a deterministic fixture source that `adesk-testkit` injects and a lazy `auto`/`off` backend selection. Its suite is 106 unit + 1 integration (the mock AT-SPI2 provider suite) + 1 doc test, and `invoke_accessible_action` answers `not_supported` for any id when there is no backend because availability is probed before the id is resolved.
`adesk-server` now also serves the VAP viewer endpoint on a second listener (a Unix socket by default at the AGP socket's sibling path, opt-in `--viewer-tcp`, `--no-viewer` to disable) and applies viewer input through the same §5.5 seat path; it also backs the recording messages with an fps-paced capture loop (`--recordings-dir` / `ADESK_RECORDINGS_DIR` default) and the app/window-management messages by delegating to the AGP dispatch functions. Its suite is 289 passed / 0 failed.
`cargo check --workspace --all-targets` is green, `cargo clippy --workspace --all-targets --no-deps -- -D warnings` is clean, `cargo fmt --all --check` is clean, and `cargo doc --workspace --no-deps --document-private-items` emits zero warnings.
`cargo test --workspace --no-fail-fast` = 1912 passed, 0 failed, 5 ignored; the 5 ignored are doc-code fences only.
Application launches (AGP and VAP both) compose a Wayland-targeting child environment — `DISPLAY`/`XAUTHORITY` removed and the common toolkit opt-ins set — so apps Firefox/Chrome included open inside adesk instead of the host; `adesk-app-registry` gained the `LaunchEnv::without(..)`/`removals()` seam this uses (its suite is 175 tests).
The compositor's DMA-BUF path is hardened: every incoming dmabuf is validated before import, failures log the buffer geometry, and the `zwp_linux_dmabuf_v1` global is advertised only when the active renderer is not a software GL rasterizer — so a default (GPU-less) `adesk-server` demotes Mesa llvmpipe to pixman and never reaches the reported software-GL DMA-BUF SIGSEGV (`crates/adesk-compositor/CONTEXT.md`); the global-suppression path has unit-level and construction-smoke coverage but no end-to-end "global absent" proof.
The on-demand capture path reuses a size-keyed pool of offscreen render targets (`adesk_render::TargetPool`, owned per backend by the compositor's `HeadlessRenderer`), so repeated captures of a given size no longer re-allocate a fresh ~4 MiB target each time.
Feature-gated suites are green as well: `cargo test -p adesk-agent --features test-support,e2e` = 146 passed.
`adesk-testkit` declares no Cargo features (so `--all-features` is a no-op); its suite runs 77 passed / 3 ignored doc-fences, unchanged under `ADESK_TEST_GL=1`, which is the only environment gate.
Capstone evidence: `crates/adesk-agent/tests/e2e_runtime.rs` (14 tests) and `adesk-testkit`'s E2E suites drive a real runtime end to end — discover app → `launch_app` by desktop-file id → tiled toplevel → observation → click/type/scroll → native commit/damage events → `wait_for_quiet` → selective capture — with no screenshot loop.
Launch→window correlation is asserted in the capstone itself; clipboard publication ordering, output composition (active-only), popup pixel proofs and the single global `seq` domain each have dedicated integration proofs.
An interactive human front-end exists: `adesk-viewer-gui` is a GTK4/libadwaita app (the workspace's only GTK crate) that connects over VAP, renders the streamed desktop frames with a drawn remote cursor, routes pointer/key/scroll/text through the same seat path the agent uses, and makes the desktop operable — a bottom task bar switches windows through the runtime-native VAP `activate_window` message and closes them with `close_window`, an **Open app** launcher searches the runtime's installed apps (`list_apps`) and starts one (`launch_app`), and a header popover names the endpoint actually dialled, the active window and the control state, with a Release-control action (`Ctrl+Alt+Escape`) handing the keyboard back to the local desktop. All of this was added end to end in `adesk-viewer-proto` / `adesk-viewer` / `adesk-server` and is specified in `docs/viewer.md` §3–§5. The headless `adesk-viewer` binary remains for frames→PNG, scripted input and `--record <FILE>`; the GUI suite is 151 passed, all display-free.
The GUI's pointer/keyboard input is faithful: the drawn remote cursor follows local motion between server frames (it no longer waits for a pushed frame), a cancelled/stopped click gesture always forwards the matching release so the remote button can never stick down, and both the client bridge and the `adesk-viewer` server session apply seat input immediately rather than waiting behind a request/response round trip — so clicks no longer silently stall (the server session's read loop is also cancellation-safe via `LineReader`).
The GUI header also carries a record toggle driving the VAP recording messages.
Screen recording is proven end to end: `docs/viewer.md` is the normative recording spec, and `adesk-recorder`'s round-trip tests decode their own MJPEG output, while `adesk-viewer`/`adesk-server` tests exercise start → frames → stop across the wire and the on-disk file, with the encoder/path (`.avi` software, `.mp4` GPU) chosen to match the backend actually used.
App and window management over VAP is proven end to end in `crates/adesk-server/tests/viewer.rs`: list apps with a filter → launch a fixture app → observe the new window in `state` → close a window, plus the unknown-window VAP `error` path.
Inspector debug overlays (`inspect_capture` / `inspect_subscribe`) remain AGP-only: the VAP viewer streams plain desktop frames (overlays are negotiated on the wire but not composited into v1 frames), so overlay inspection still requires an AGP client (`crates/adesk-server/src/dispatch/inspect.rs`).
Explicitly outside v1 scope (objective step 9): XWayland, drag-and-drop, richer clipboard support, multi-window visibility, and a reactive accessibility event stream (accessibility is exposed only as the on-demand §5.11 text view).
`adesk-testkit` now provides a fixture exec-path override (`TestAppSpec::with_exec`), so a downstream crate can point a fixture at its own program and reuse `TestAppSpec::desktop_entry`;
downstream crates still ship their own fixture binary because testkit's `adesk-test-app` helper is not built during their test runs (e.g. `crates/adesk-agent/examples/adesk-e2e-app.rs`).

## Routing Table

| Area | Owner |
|---|---|
| Domain model, ids, geometry, events, umbrella error | `crates/adesk-core/` |
| AGP wire protocol, codec, frame types | `crates/adesk-proto/` |
| XDG `.desktop` registry, app launch | `crates/adesk-app-registry/` |
| Window model, tiling policy, focus | `crates/adesk-wm/` |
| Temporal observation, quiet detection, waiters | `crates/adesk-observer/` |
| Offscreen rendering, damage, readback, images | `crates/adesk-render/` |
| Smithay compositor, protocols, seat, input injection | `crates/adesk-compositor/` |
| Human inspection, debug overlays | `crates/adesk-inspector/` |
| Runtime binary, socket server, AGP dispatch, VAP viewer endpoint | `crates/adesk-server/` |
| Agent client SDK | `crates/adesk-client/` |
| Multimodal agent prototype, metrics | `crates/adesk-agent/` |
| Test harness, Wayland test client, fixtures | `crates/adesk-testkit/` |
| Viewer wire protocol (VAP) messages + codec | `crates/adesk-viewer-proto/` |
| Viewer server session, client SDK, headless viewer binary | `crates/adesk-viewer/` |
| GTK4 viewer front-end: frame view, cursor, app launcher, task bar | `crates/adesk-viewer-gui/` |
| Screen recording (frame encoding, AVI/MP4 muxing, GPU ffmpeg backend) | `crates/adesk-recorder/` |
| Accessibility tree (AT-SPI2), text rendering, element handles | `crates/adesk-a11y/` |
| AI Machine runtime, container backend, host control plane | `crates/adesk-machine/` |
| Protocol/viewer/machine specs, architecture decisions | `docs/` |
| Flake packages + NixOS module, dev shell, build wrapper | `flake.nix`, `nix/`, `scripts/` |
lient SDK | `crates/adesk-client/` |
| Multimodal agent prototype, metrics | `crates/adesk-agent/` |
| Test harness, Wayland test client, fixtures | `crates/adesk-testkit/` |
| Viewer wire protocol (VAP) messages + codec | `crates/adesk-viewer-proto/` |
| Viewer server session, client SDK, headless viewer binary | `crates/adesk-viewer/` |
| GTK4 viewer front-end: frame view, cursor, app launcher, task bar | `crates/adesk-viewer-gui/` |
| Screen recording (frame encoding, AVI/MP4 muxing, GPU ffmpeg backend) | `crates/adesk-recorder/` |
| Accessibility tree (AT-SPI2), text rendering, element handles | `crates/adesk-a11y/` |
| AI Machine runtime, container backend, host control plane | `crates/adesk-machine/` |
| Protocol/viewer/machine specs, architecture decisions | `docs/` |
| Flake packages + NixOS module, dev shell, build wrapper | `flake.nix`, `nix/`, `scripts/` |
