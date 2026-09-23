# adesk-viewer-gui — GTK4/libadwaita desktop front-end for the ADesk viewer

## Intent
The interactive human front-end of an ADesk runtime — the GUI counterpart of the
headless `adesk-viewer` binary, analogous to `remote-viewer` for a SPICE VM.
It connects to the runtime's VAP viewer endpoint with `adesk_viewer::ViewerClient`,
renders the streamed desktop frames, provides a minimal window **task bar** so a
human can list and switch the tiled windows, offers a **recording toggle** that
starts/stops a runtime-side screen recording, and turns local mouse/keyboard/task-bar
activity into VAP messages.
Central principle, inherited from `docs/viewer.md` §5 and the root `CONTEXT.md`:
the human is placed *in the same seat the agent drives* — a viewer action is never a
special code path.
Window switching uses the runtime-native VAP `activate_window` message; pointer/key/
text use the normal seat input path.

## API Surface
- Binary `adesk-viewer-gui` (`./src/main.rs`): thin entry point,
  `fn main() -> gtk4::glib::ExitCode { adesk_viewer_gui::run() }`.
- Library `adesk_viewer_gui` (`./src/lib.rs`):
  - `pub fn run() -> gtk4::glib::ExitCode` — parses the CLI, installs `tracing`
    logging, resolves the viewer endpoint and runs the GTK application; the only
    public function.
  - Public, GTK-free, unit-testable modules:
    - `pub mod cli` — `Cli` (clap: `--unix <PATH>`, `--tcp <HOST:PORT>`,
      `--log <FILTER>`/`ADESK_LOG`), `Cli::target()`, `default_socket_path()`.
      The Unix socket path is `adesk_viewer::resolve_socket_path` (explicit
      `--unix` → `$ADESK_VIEWER_SOCKET` → the sibling of the server's default
      AGP socket), so the GUI's default equals the server's viewer endpoint.
    - `pub mod address` — failure-message composition that names the dialed
      endpoint (headless-viewer wording) plus `agp_socket_hint`, which fires
      only when the dialed path's file name is `adesk.sock` (the AGP socket)
      and names the viewer sibling to try instead.
    - `pub mod error` — `GuiError { Config, Client, Image }`, `pub type Result<T>`.
    - `pub mod image` — `DecodedImage { width, height, rgba8 }`, `decode(&ImagePayload)`
      → tightly packed RGBA8.
    - `pub mod mapping` — `DisplayRect`, `letterbox()`, `widget_to_normalized()`
      (the letterbox math).
    - `pub mod taskbar` — `TaskBarEntry`, `entries(&DesktopState)`, `set_active()`.
  - Crate-private GTK-free modules: `record` (recording-control state machine),
    `keystroke` (keystroke routing state machine).
  - Crate-private GTK layer: `bridge`, `frame_view`, `task_bar_view`, `app`.

## Constraints
- **The only GTK4/libadwaita crate in the workspace.** No other crate may gain a
  GTK dependency; the headless `adesk-viewer` and every runtime crate stay GTK-free.
- GTK4 + libadwaita come from the Nix dev shell / package (`flake.nix`
  `adeskGuiLibraries`). Rust deps `gtk4` and `adw` (alias for `libadwaita`) are
  declared once in the root `[workspace.dependencies]`; this crate enables the
  `v4_8` (gtk4) and `v1_4` (adw) features inline for `gtk::ContentFit`,
  `adw::ToolbarView` and `adw::Banner`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files well under the
  ~1000-line threshold (largest: `./src/bridge.rs`, 559 lines).
- All human input goes through `adesk_viewer::ViewerClient` (the same VAP seat path
  the agent's input uses) — never a bespoke or direct-to-compositor path.
- Coordinates that cross VAP are NORMALIZED `0.0..=1.0` output fractions, never
  pixels; the conversion lives in `./src/mapping.rs`.
- Separate everything testable without a display into GTK-free modules; the GUI
  tests must never require a display, GPU or network.
- The binary connects over VAP only; it never links the compositor or Smithay.

## Routing Table
| Area | Owner |
|---|---|
| Crate root, `run()`, module wiring | `./src/lib.rs` |
| GTK application entry point (binary) | `./src/main.rs` |
| CLI parsing + viewer-endpoint resolution | `./src/cli.rs` |
| `GuiError` / `Result` | `./src/error.rs` |
| Image payload → RGBA8 decode | `./src/image.rs` |
| Widget ↔ normalized letterbox math | `./src/mapping.rs` |
| Task-bar view model | `./src/taskbar.rs` |
| Recording-control state machine (labels + next command) | `./src/record.rs` |
| Keystroke routing (key vs text), pure | `./src/keystroke.rs` |
| tokio ↔ GLib bridge (`Bridge`, `InputCommand`, `UiEvent`, `InputHandle`) | `./src/bridge.rs` |
| Frame view widget + input controllers | `./src/frame_view.rs` |
| Task-bar widget | `./src/task_bar_view.rs` |
| Application window + event loop | `./src/app.rs` |

## Design Decisions
- **GTK-free core, then GTK glue.** All pure logic (CLI, error, image decode,
  letterbox math, task-bar model, keystroke routing, recording control, address
  diagnostics) lives in display-free modules with unit tests; the GTK layer
  (`app`/`frame_view`/`task_bar_view`) only wires widgets to those helpers and
  reuses them (no duplicated math).
- **One resolver for every VAP client.** The GUI derives its default Unix socket
  with `adesk_viewer::resolve_socket_path` — the same function the headless
  `adesk-viewer` uses and the exact mirror of the server's bind derivation — so
  the GUI default can never drift from (or omit) the server's viewer endpoint
  the way a GUI-local `$XDG_RUNTIME_DIR/adesk-viewer.sock` guess could.
- **tokio ↔ GLib bridge.** `ViewerClient` is tokio-based while GTK runs a glib main
  loop. `bridge` spawns one named OS thread running a single-thread tokio runtime
  that owns the client; the two threads exchange plain `tokio::sync::mpsc`
  unbounded channels (runtime-agnostic — the GTK side needs no tokio executor).
  The GTK side drives a `glib::spawn_future_local` loop over `UiEvent`s; input flows
  back through a cloneable `InputHandle`. The worker never touches GTK and the GTK
  loop never blocks on the network.
- **Frames are decoded on the worker.** `bridge` decodes each `ImagePayload`
  (`image::decode`) before sending it, so the GTK thread only builds a
  `gdk::MemoryTexture` from owned RGBA8 (no base64/PNG work on the main loop). A
  decode failure becomes a `UiEvent::Notice`, never fatal.
- **Initial state + frame.** On connect the worker sends `Connected { target, hello }`,
  calls `set_control(ControlOwner::Human)` (the advisory ownership handshake), then
  `request_state()` + `request_frame()` so the task bar and the view fill in
  immediately.
- **Task-bar refresh = refresh-on-change + modest timer, coalesced.** The server has
  no pushed `state` stream, so the worker calls `request_state()` when a frame's
  `active_window_id` changes AND on a 500 ms `tokio::time::interval` (a third
  `select!` branch). The request is awaited inline, so at most one is ever in flight
  (inherent coalescing); this also converges window create/destroy/title changes that
  leave the active id unchanged. `UiEvent::Frame` carries `active_window_id`, so the
  highlight updates instantly without waiting for a state refresh.
- **The task bar rebuilds only on change.** `TaskBarView::update_state` rebuilds the
  toggle rows only when `taskbar::entries` differs from the current model; the
  per-frame `set_active` only re-renders the highlight. Each toggle uses `clicked`
  (not `toggled`) so programmatic highlight updates never re-fire `activate_window`.
- **Recording control is server-driven.** A `gtk::ToggleButton` in the header
  sends `InputCommand::StartRecording(RecordRequest::default())` when idle or
  finished and `InputCommand::StopRecording` while recording; the button's label
  and active state are then corrected from the server's `RecordingStatus`
  (`UiEvent::Recording`) — the local click never decides the state. The pure
  `record::RecordControl` derives `RecordState` from the status (`recording` ⇒
  Recording, else `error` ⇒ Idle for a retry, else a known `path` ⇒ Finished,
  else Idle) and produces the labels: button `Record`/`Stop`, status line
  `REC 12s` while recording, `saved <path>` once finished, or
  `recording failed: …`. The worker polls `request_recording()` on the existing
  500 ms tick **only while a recording is active** (gated by the last status),
  so an idle viewer sends no extra traffic. A start uses the crate-default
  request (runtime-chosen path, 30 fps, `auto` encoder) — there is no path/fps/
  encoder UI in v1.
- **Keystroke routing avoids double-typing.** Both VAP `key` and `text` ultimately
  inject compositor key events (`crates/adesk-server/src/viewer/backend.rs`), so
  `keystroke::KeyRouter` routes a plain text-producing key with no ctrl/alt/super to
  `Text`, and every named/control key or modified chord to `Key` (a keysym name the
  runtime resolves case-insensitively). A release forwards a `Key` only when its
  press did. Input-method commits are forwarded only when multi-character, since a
  single-character commit duplicates the key path.
- **Click/motion mapping uses the displayed-image rectangle.** A click inside a
  letterbox/pillarbox bar is ignored (not clamped); scroll reuses the last pointer
  position, with the widget center as fallback. The frame view is focused up front
  (and re-focused on click) so typing works immediately.
- **Lifecycle.** On `close-request` the app calls `InputHandle::close()` (drops the
  shared sender → the worker's `recv()` returns `None` → it ends) and aborts the
  event-loop `glib::JoinHandle`; nothing leaks.
- **GTK runs on `argv[0]` alone.** `ApplicationExtManual::run()` would hand the real
  process `argv` to `g_application_run`, whose GOptionContext re-parses (and rejects)
  the flags clap already consumed — the "Unknown option --unix" bug. `run_application`
  therefore takes the program name and calls `run_with_args_os(&[argv0])` (the `OsStr`
  variant, so a non-UTF-8 `argv[0]` cannot panic like `env::args()` would), with
  `program_name_from` as the pure, unit-tested helper (falling back to
  `"adesk-viewer-gui"` when the environment provides no `argv[0]`).

## Test Strategy
No display, GPU or network. `./scripts/dev.sh cargo test -p adesk-viewer-gui` →
**63 passed / 0 failed** (all in the lib target; 0 in the bin target, 0 doctests).
- `cli` (9): `--unix`/`--tcp` parsing, conflict rejection, bad address →
  `GuiError::Config`, default target = `resolve_socket_path(None)`, explicit
  `--unix` beats the environment, default follows `$ADESK_VIEWER_SOCKET` and the
  `$ADESK_SOCKET` sibling.
- `address` (10): the AGP-file-name hint fires only for `adesk.sock` (sibling
  named, prefix/suffix matches and file-name-less paths do not), connect-failure
  and unexpected-close wording naming the path, hint appended on the AGP path,
  TCP composes without a hint.
- `mapping` (8): identity, pillarbox, letterbox, corners, center, out-of-rect
  clamping, zero-sized/non-finite → `None`, zero-size-rect guard.
- `taskbar` (7): label from title/app_id/fallback/empty, active flag, `set_active`,
  empty state.
- `image` (4): RGBA8 round-trip, PNG decode, undecodable PNG, mismatched length.
- `error` (2): `ViewerError` → `Client`, message rendering.
- `keystroke` (7): letter→Text, letter+ctrl→Key press/release, `Return`→Key, Text
  release no-op, no name/unicode→Ignore, space→Text, modified no-name→Ignore.
- `bridge` (6): decodable frame→`Frame` event, undecodable frame→`Notice`,
  `InputHandle::close` stops accepting commands, a recording status→
  `Recording(Ok)` (returning `active`), a recording error→`Recording(Err)`,
  recording commands reach the worker.
- `record` (6): new control is idle/`Record`/`StartRecording`, a recording
  status→Recording/`Stop`/`REC 12s`/`StopRecording`, a finished status→
  Finished/`Record`/`saved <path>`/`StartRecording`, an error status→Idle with
  the reason surfaced, an idle status clears the line, duration truncation.
- `app` (2): the argument vector GTK receives is exactly the program name
  (`program_name_from` passthrough; `None` → `"adesk-viewer-gui"` fallback) — the
  regression pin for the "Unknown option --unix" bug.
- The GTK widget wiring (`app`'s widget glue, `frame_view`, `task_bar_view`) is
  untested glue — it needs a display; test pure logic instead, never GUI behavior.

## Notes for Agents
- Run every command through `./scripts/dev.sh` (bare `cargo` cannot link outside the
  Nix dev shell).
- This crate is the only place a GTK dependency is allowed; never add GTK to a
  headless crate.
- `image` is a workspace dep with only the `png` feature; `rgba8` payloads bypass it
  via `ImagePayload::to_rgba8_buffer`.
- The runtime's `Keysym::parse` is case-insensitive and accepts xkb keysym names plus
  single characters, so `gdk::Key::name()` output (`Return`, `space`, `a`, `Shift_L`)
  routes directly onto the `key` path.
- Colors: desktop frames are opaque, so the straight-alpha `gdk::MemoryFormat::R8g8b8a8`
  texture is correct.

## Known Issues
- The connection is one-shot: a dropped connection shows a banner naming the
  viewer socket (with an AGP-misdirection hint when the path's file name is
  `adesk.sock`) but there is no reconnect affordance (a restart re-connects).
- The recording toggle always starts with the crate-default `RecordRequest` (a
  runtime-chosen path, 30 fps, `auto` encoder); there is no UI to choose a path,
  frame rate or encoder. A runtime that does not support recording answers with
  an `error`, surfaced as a `recording failed: …` status line.

## Status
Implemented and green: `run()`, the GTK application (frame view, task bar,
recording toggle + status line, status banner), the tokio↔GLib bridge, the CLI,
and every pure module with unit tests. Socket resolution goes through
`adesk_viewer::resolve_socket_path` (server-parity default) and connect/disconnect
failures name the dialed endpoint, with an AGP-socket hint when the file name is
`adesk.sock` (`crates/adesk-viewer-gui/src/address.rs`).
Gates all pass through `./scripts/dev.sh`: `cargo build -p adesk-viewer-gui`;
`cargo test -p adesk-viewer-gui` (49 passed); `cargo clippy -p adesk-viewer-gui
--all-targets --no-deps -- -D warnings`; `cargo fmt --all --check`;
`cargo doc -p adesk-viewer-gui --no-deps --document-private-items` (zero warnings);
and `cargo check --workspace --all-targets`.
