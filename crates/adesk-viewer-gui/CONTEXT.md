# adesk-viewer-gui — GTK4/libadwaita desktop front-end for the ADesk viewer

## Intent
The interactive human front-end of an ADesk runtime — the GUI counterpart of the
headless `adesk-viewer` binary, analogous to `remote-viewer` for a SPICE VM.
It connects to the runtime's VAP viewer endpoint with `adesk_viewer::ViewerClient`,
renders the streamed desktop frames **together with the remote pointer**, provides a
minimal window **task bar** so a human can list and switch the tiled windows, offers a
**recording toggle** that starts/stops a runtime-side screen recording, and turns local
mouse/keyboard/task-bar activity into VAP messages.
Central principle, inherited from `docs/viewer.md` §5 and the root `CONTEXT.md`:
the human is placed *in the same seat the agent drives* — a viewer action is never a
special code path.
Window switching uses the runtime-native VAP `activate_window` message; pointer/key/
text use the normal seat input path.

The second goal is operability: a human who connects must be able to see *what is
happening* and *what they can do* without reading source. So the window shows the
remote pointer, keeps the human's keys out of GTK's hands while the desktop drives
them, and carries an adwaita help/status affordance (endpoint, active window,
connection, control state and a capability list) plus one documented escape hatch.

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
  - Crate-private GTK-free modules: `cursor` (remote-pointer placement), `help`
    (status/help text), `pointer` (button press/release pairing), `keystroke`
    (keyboard forwarding decision + escape chord), `record` (recording-control
    state machine).
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
  ~1000-line threshold (largest: `./src/bridge.rs`, 570 lines).
- All human input goes through `adesk_viewer::ViewerClient` (the same VAP seat path
  the agent's input uses) — never a bespoke or direct-to-compositor path.
- Coordinates that cross VAP are NORMALIZED `0.0..=1.0` output fractions, never
  pixels; widget ↔ normalized conversion lives in `./src/mapping.rs`, normalized →
  widget (the cursor) in `./src/cursor.rs`.
- Separate everything testable without a display into GTK-free modules; the GUI
  tests must never require a display, GPU or network.
- The crate is VAP-only: it never links the compositor or Smithay, and it adds no
  protocol message of its own.

## Routing Table
| Area | Owner |
|---|---|
| Crate root, `run()`, module wiring | `./src/lib.rs` |
| GTK application entry point (binary) | `./src/main.rs` |
| CLI parsing + viewer-endpoint resolution | `./src/cli.rs` |
| `GuiError` / `Result` | `./src/error.rs` |
| Image payload → RGBA8 decode | `./src/image.rs` |
| Widget ↔ normalized letterbox math | `./src/mapping.rs` |
| Normalized → widget remote-pointer placement | `./src/cursor.rs` |
| Task-bar view model | `./src/taskbar.rs` |
| Status/help/panel text, focus hint, Pango escaping | `./src/help.rs` |
| Keystroke forwarding decision + escape chord | `./src/keystroke.rs` |
| Button press/release pairing | `./src/pointer.rs` |
| Recording-control state machine (labels + next command) | `./src/record.rs` |
| tokio ↔ GLib bridge (`Bridge`, `InputCommand`, `UiEvent`, `InputHandle`) | `./src/bridge.rs` |
| Frame view widget, pointer overlay, input controllers | `./src/frame_view.rs` |
| Task-bar widget | `./src/task_bar_view.rs` |
| Application window, header (escape hatch, help, record), event loop | `./src/app.rs` |

## Design Decisions
- **GTK-free core, then GTK glue.** All pure logic (CLI, error, image decode,
  letterbox math, cursor placement, task-bar model, help/status text, keystroke
  routing, pointer pairing, recording control, address diagnostics) lives in
  display-free modules with unit tests; the GTK layer (`app`/`frame_view`/
  `task_bar_view`) only wires widgets to those helpers and reuses them (no
  duplicated math or text).
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
  decode failure becomes a `UiEvent::Notice`, never fatal. `UiEvent::Frame` carries
  `seq`, `ts_ms`, the decoded image, the frame's `CursorState` and the active window
  id, so the GTK layer never re-reads the wire.
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
- **The remote pointer is drawn over the frames.** The frame view is a `gtk::Overlay`
  whose main child is the picture and whose single overlay child is a
  `gtk::DrawingArea` spanning the whole view (`set_can_target(false)`, so clicks and
  key handling stay with the picture). Its draw function resolves the frame's
  normalized `CursorState` with the pure `cursor::overlay_position` against the same
  `mapping::letterbox` rectangle the click mapping uses, and paints a small arrow
  with cairo. Because the position is computed at draw time from the widget's current
  size and the widget is repainted on resize, a window resize needs no bookkeeping:
  `set_frame` only `queue_draw()`s when the cursor actually moved, appeared or
  vanished.
- **Clicking the desktop takes control *and* is delivered.** The click controller
  calls `grab_focus()` and still forwards the press, so the very first click on an
  unfocused desktop reaches the remote app (and wires 2, 3, 8, 9 too). The press and
  release decisions live in `pointer::PointerState`: a press on the image is
  remembered, a release is forwarded whenever its press was (with a clamped
  position), so a drag ending in a letterbox bar lifts the button instead of leaving
  the remote pointer stuck down.
- **Forwarded keystrokes stop propagating; exactly one chord is kept.** While the
  desktop holds keyboard control, `keystroke::KeyRouter` forwards `Tab`, `Escape`,
  `Space`, `Return`, arrow keys and `Ctrl`-chords to the remote app and the GTK key
  controller returns `glib::Propagation::Stop` for them, so GTK's focus traversal,
  activations and window shortcuts cannot also fire. The single exception is the
  viewer's escape chord `Ctrl+Alt+Escape` (`keystroke::ESCAPE_LABEL` /
  `ESCAPE_ACCELERATOR`), for which the router answers `Delivery::Escape`: it is not
  forwarded and the event is left to propagate to the app-level `gio::SimpleAction`
  registered as `win.release-control`, which moves keyboard focus off the frame view.
  The same action is bound to the header's visible **Release control** button, and
  clicking the desktop takes control back. `deliver_key` in `frame_view` maps the
  decision onto GTK; `crate::keystroke` owns the decision and is unit-tested.
- **The help affordance is a model + glue.** `help::Help` (GTK-free) holds the dialled
  endpoint, the active window label, a `ConnectionState` and whether the desktop holds
  the keyboard; `HelpUi` in `app` paints it into a header `gtk::MenuButton` +
  `gtk::Popover` (a `gtk::Label` carrying Pango markup: four status rows plus the
  `help::capabilities()` list) and into the tooltip, and reveals a `gtk::Revealer`
  hint line from `help::focus_hint` while the desktop does not hold the keyboard. All
  values that come from the runtime (window titles, paths) are Pango-escaped by
  `help::markup`. The model's setters report change, so the panel re-renders only on
  a real change (once per connection/state/focus event, never per frame).
- **The active window label is shared.** `help::active_window_label(&DesktopState)`
  reuses `taskbar::entries`, so the popover and the task bar can never name a window
  differently.
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
  letterbox/pillarbox bar is dropped (not clamped); scroll reuses the last pointer
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
**96 passed / 0 failed** (all in the lib target; 0 in the bin target, 0 doctests).
- `cli` (8): `--unix`/`--tcp` parsing, conflict rejection, bad address →
  `GuiError::Config`, default target = `resolve_socket_path(None)`, explicit
  `--unix` beats the environment, default follows `$ADESK_VIEWER_SOCKET` and the
  `$ADESK_SOCKET` sibling.
- `address` (10): the AGP-file-name hint fires only for `adesk.sock` (sibling
  named, prefix/suffix matches and file-name-less paths do not), connect-failure
  and unexpected-close wording naming the path, hint appended on the AGP path,
  TCP composes without a hint.
- `mapping` (8): identity, pillarbox, letterbox, corners, center, out-of-rect
  clamping, zero-sized/non-finite → `None`, zero-size-rect guard.
- `cursor` (8): identity fraction→widget point, corners, letterbox/pillarbox
  offsets, hidden cursor → `None`, out-of-range fractions clamp into the rect,
  non-finite position → `None`, collapsed rect → `None`.
- `taskbar` (7): label from title/app_id/fallback/empty, active flag, `set_active`,
  empty state.
- `help` (12): a fresh model is connecting with the desktop in control; the four
  labelled rows; unknown window; the control row flips on release; a failed state
  names the reason; identical values are not reported as changes; the one-line
  summary; the markup carries every row and the capabilities; markup escapes
  untrusted text; the focus hint only without control; the active-window label
  follows the state; the capabilities name the escape hatch.
- `image` (4): RGBA8 round-trip, PNG decode, undecodable PNG, mismatched length.
- `error` (2): `ViewerError` → `Client`, message rendering.
- `keystroke` (13): letter→Text, letter+ctrl→Key press/release, `Return`→Key, Text
  release no-op, no name/unicode→Pass, space→Text, modified no-name→Pass, Tab/
  Escape/Space reach the remote, Ctrl-chords and arrows reach the remote, the escape
  chord is never forwarded (press and release, and it forgets a held plain `Escape`),
  it needs both modifiers, nothing is forwarded without focus, the accelerator/label
  constants agree with the chord the router detects.
- `pointer` (7): a press on the image forwards, a press in the bars drops (and its
  release drops), a release matches its press, a drag ending outside still releases,
  buttons pair independently, repeated presses do not double-record, a stray release
  drops.
- `bridge` (7): decodable frame→`Frame` event (carrying the cursor), hidden cursor
  still reported, undecodable frame→`Notice`, `InputHandle::close` stops accepting
  commands, a recording status→`Recording(Ok)` (returning `active`), a recording
  error→`Recording(Err)`, recording commands reach the worker.
- `record` (6): new control is idle/`Record`/`StartRecording`, a recording
  status→Recording/`Stop`/`REC 12s`/`StopRecording`, a finished status→
  Finished/`Record`/`saved <path>`/`StartRecording`, an error status→Idle with
  the reason surfaced, an idle status clears the line, duration truncation.
- `app` (4): the argument vector GTK receives is exactly the program name
  (`program_name_from` passthrough; `None` → `"adesk-viewer-gui"` fallback) — the
  regression pin for the "Unknown option --unix" bug — the escape action's
  accelerator name is the window action (`win.release-control`), and the help
  markup parses as Pango (`gtk::pango::parse_markup`, display-free) with the
  escaped window title rendering back exactly.
- **Untested widget glue (needs a display):** everything inside `app`'s
  `HelpUi`/`RecordUi` construction and `activate()` (header/popover/revealer
  assembly, `gio::SimpleAction` registration, focus handoff), `frame_view`'s
  controllers, overlay and cairo pointer glyph, and `task_bar_view`'s buttons.
  Their decisions are all covered by the GTK-free modules above; test pure logic
  instead, never GUI behavior.

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
- `gtk::prelude::GtkWindowExt::set_focus` must be spelled explicitly: `gtk::Window`
  and `gtk::Root` both expose `set_focus`, and the call is ambiguous otherwise.
- The pointer overlay must keep `set_can_target(false)`; without it the drawing area
  swallows the clicks the picture needs.

## Known Issues
- The connection is one-shot: a dropped connection shows a banner naming the
  viewer socket (with an AGP-misdirection hint when the path's file name is
  `adesk.sock`) but there is no reconnect affordance (a restart re-connects).
- The recording toggle always starts with the crate-default `RecordRequest` (a
  runtime-chosen path, 30 fps, `auto` encoder); there is no UI to choose a path,
  frame rate or encoder. A runtime that does not support recording answers with
  an `error`, surfaced as a `recording failed: …` status line.
- While the desktop holds the keyboard, every chord the frame view forwards is
  stopped in GTK — including `Alt+F4`. Quitting or handing the window back to the
  window manager therefore needs the header's **Release control** button (or the
  escape chord) first; the window close button always works.
- The help panel's active-window line is refreshed from `state` messages (requests
  the worker sends on a 500 ms timer and whenever the active window changes), so a
  freshly focused window's title can lag the task-bar highlight by up to one round
  trip.
- **VAP capability boundary (why there is no app menu).** The GUI's only runtime
  channel is VAP, and VAP's entire client vocabulary is `request_frame`,
  `request_state`, `set_control`, `pointer_move`, `pointer_button`, `scroll`,
  `key`, `text`, `activate_window`, `start_recording`, `stop_recording`,
  `request_recording`, `bye` (`docs/viewer.md` §4). There is **no** VAP message
  to list installed apps, launch an app, or close a window — app discovery/launch
  (§5.1/§5.2) and window close live in AGP only. So the UI cannot offer those
  operations without a protocol/ARCH change; `DesktopState` carries windows only.
- No menus, toolbar entries or `gio` keyboard accelerators exist beyond the
  release action: the header bar holds **Release control**, the help menu button
  and the Record/Stop toggle, the bottom bar is the window task bar, and all
  pointer/keyboard/text input is implicit on the frame view (the help popover and
  the focus hint line are the on-screen explanation for it).

## Status
Implemented and green: `run()`, the GTK application (frame view + remote-pointer
overlay, task bar, release-control escape hatch, recording toggle + status line,
help popover + focus hint, status banner), the tokio↔GLib bridge, the CLI, and
every pure module with unit tests. Socket resolution goes through
`adesk_viewer::resolve_socket_path` (server-parity default) and connect/disconnect
failures name the dialed endpoint, with an AGP-socket hint when the file name is
`adesk.sock` (`crates/adesk-viewer-gui/src/address.rs`).
Gates all pass through `./scripts/dev.sh`: `cargo build -p adesk-viewer-gui`;
`cargo test -p adesk-viewer-gui` (95 passed); `cargo clippy -p adesk-viewer-gui
--all-targets --no-deps -- -D warnings`; `cargo fmt --all --check`;
`cargo doc -p adesk-viewer-gui --no-deps --document-private-items` (zero warnings);
and `cargo check --workspace --all-targets`.
The window layout is deliberately flat and additive: `adw::ToolbarView` with a
header bar, a vertical content box (banner, hint revealer, recording status, frame
view) and the task bar as the bottom bar, so another header action or a side panel
can be added without restructuring.
