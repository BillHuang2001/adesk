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
| `crates/adesk-render/` | Offscreen render pipeline: render target, damage regions, crop/downsample, readback, image encoding, software path. |
| `crates/adesk-compositor/` | Smithay integration: protocol handlers, event loop, virtual output, seat/input injection, event emission. |
| `crates/adesk-inspector/` | Human inspection: full-output composition, debug overlays, inspection frames. |
| `crates/adesk-server/` | Binary + lib: composes everything, owns the Unix socket and AGP dispatch. |
| `crates/adesk-client/` | Async Rust client SDK for AGP (typed methods + event stream). |
| `crates/adesk-agent/` | Multimodal agent prototype: provider-agnostic LLM interface, context assembly, metrics, task scenarios. |
| `crates/adesk-testkit/` | Dev-only test harness: in-process runtime, Wayland test client, fixtures, image assertions. |

End-to-end tests live in `crates/adesk-server/tests/`; protocol-level compositor tests in
`crates/adesk-compositor/tests/`. Both build on `adesk-testkit`.

## Cross-crate contracts

- **Protocol:** `docs/protocol.md` is normative. `adesk-proto` implements it exactly;
  server and client must not invent methods or fields outside it.
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

System libraries (libxkbcommon, pixman, EGL/GLES, libwayland, libudev) come from the
Nix dev shell. **Always** build through the wrapper:

```sh
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo clippy --workspace --all-targets
```

Bare `cargo build` fails to link outside the shell — that is expected, not a code bug.
`nix develop -c <cmd>` is equivalent.

## Known issues

- The sandbox has no GPU and no system EGL on the default library path; only the dev
  shell provides them. GL rendering uses Mesa's `llvmpipe` software fallback there.
- `xkbcommon`'s keymap data comes from `XKB_CONFIG_ROOT` set by the dev shell; running
  the binaries outside the shell will fail keyboard setup unless that variable is set.

## Status
All 12 crates are implementation-complete: zero `todo!()` in the workspace, no skeleton-phase crate-level `allow` attributes left, and `docs/protocol.md` / `docs/architecture.md` / `docs/core-api.md` remain normative.
`./scripts/dev.sh cargo check --workspace --all-targets` is green and `cargo clippy --workspace --all-targets --no-deps -- -D warnings` is clean.
`./scripts/dev.sh cargo test --workspace --no-fail-fast` = 1225 passed, 0 failed, 5 ignored across 92 test targets (independently re-verified; the 5 ignored are doc-code fences only — no behavioural skips).
Feature-gated suites are green as well: `cargo test -p adesk-agent --features test-support,e2e` (94).
`adesk-testkit` declares no Cargo features, so `--all-features` is a no-op; its default suite runs 66 tests + 3 doctests (3 ignored doc-code fences), and the GL path is gated by the `ADESK_TEST_GL=1` env var.
Capstone evidence: `crates/adesk-agent/tests/e2e_runtime.rs` (14 tests) and `adesk-testkit`'s E2E suite drive a real runtime end to end — discover app → `launch_app` by desktop-file id → tiled toplevel → observation → click/type/scroll → native commit/damage events → `wait_for_quiet` → selective capture — with no screenshot loop.
Explicitly outside v1 scope (objective step 9): AT-SPI accessibility, XWayland, drag-and-drop, richer clipboard support, multi-window visibility, and an `adesk-testkit` exec-path override (downstream crates currently ship their own fixture binary, e.g. `crates/adesk-agent/examples/adesk-e2e-app.rs`).

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
| Runtime binary, socket server, AGP dispatch | `crates/adesk-server/` |
| Agent client SDK | `crates/adesk-client/` |
| Multimodal agent prototype, metrics | `crates/adesk-agent/` |
| Test harness, Wayland test client, fixtures | `crates/adesk-testkit/` |
| Protocol spec, architecture decisions | `docs/` |
| Dev shell, build wrapper | `flake.nix`, `scripts/` |
