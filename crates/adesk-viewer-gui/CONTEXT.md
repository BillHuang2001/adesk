# adesk-viewer-gui — GTK4/libadwaita desktop front-end for the ADesk viewer

## Intent
The interactive human front-end of an ADesk runtime — the GUI counterpart of the
headless `adesk-viewer` binary, analogous to `remote-viewer` for a SPICE VM.
It connects to the runtime's VAP viewer endpoint with `adesk_viewer::ViewerClient`,
renders the streamed desktop frames **together with the remote pointer**, provides a
minimal window **task bar** so a human can list, switch and close the tiled windows,
an **Open app** launcher so a human can list and launch the runtime's applications,
offers a **recording toggle** that starts/stops a runtime-side screen recording, and
turns local mouse/keyboard/task-bar/launcher activity into VAP messages.
Central principle, inherited from `docs/viewer.md` §5 and the root `CONTEXT.md`:
the human is placed *in the same seat the agent drives* — a viewer action is never a
special code path.
Window switching and window closing use the runtime-native VAP `activate_window` /
`close_window` messages, and app listing/launch uses the VAP `list_apps` /
`launch_app` messages; pointer/key/text use the normal seat input path.

The second goal is operability: a human who connects must be able to see *what is
happening* and *what they can do* without reading source. So the window shows the
remote pointer, keeps the human's keys out of GTK's hands while the desktop drives
them, offers an app launcher and a per-window close control, and carries an adwaita
help/status affordance (endpoint, active window, connection, control state and a
capability list) plus one documented escape hatch.

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
    (status/help text, incl. the shared `escape`), `pointer` (button press/release
    pairing), `keystroke` (keyboard forwarding decision + escape chord), `record`
    (recording-control state machine), `app_launcher` (app list, local filtering,
    launch state machine, status text), `window_close` (optimistic close/race
    decision + failure text).
  - Crate-private GTK layer: `bridge`, `frame_view`, `task_bar_view`,
    `app_launcher_view`, `app`.

## Constraints
- **The only GTK4/libadwaita crate in the workspace.** No other crate may gain a
  GTK dependency; the headless `adesk-viewer` and every runtime crate stay GTK-free.
- GTK4 + libadwaita come from the Nix dev shell / package (`flake.nix`
  `adeskGuiLibraries`). Rust deps `gtk4` and `adw` (alias for `libadwaita`) are
  declared once in the root `[workspace.dependencies]`; this crate enables the
  `v4_8` (gtk4) and `v1_4` (adw) features inline for `gtk::ContentFit`,
  `adw::ToolbarView` and `adw::Banner`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files well under the
  ~1000-line threshold (largest: `./src/bridge.rs`, 739 lines).
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
| App list: fetch policy, filtering/ordering, launch state machine | `./src/app_launcher.rs` |
| Per-window close: optimistic prune/race decision + failure text | `./src/window_close.rs` |
| tokio ↔ GLib bridge (`Bridge`, `InputCommand`, `UiEvent`, `InputHandle`) | `./src/bridge.rs` |
| Frame view widget, pointer overlay, input controllers | `./src/frame_view.rs` |
| Task-bar widget (window toggles + close buttons) | `./src/task_bar_view.rs` |
| App-launcher widgets (Open app menu, popover, search, rows) | `./src/app_launcher_view.rs` |
| Application window, header (launcher, escape hatch, help, record), notice line, event loop | `./src/app.rs` |

## Design Decisions
- **GTK-free core, then GTK glue.** All pure logic (CLI, error, image decode,
  letterbox math, cursor placement, task-bar model, help/status text, keystroke
  routing, pointer pairing, recording control, app-list filtering + launch state
  machine, close/race decision, address diagnostics) lives in display-free modules
  with unit tests; the GTK layer (`app`/`frame_view`/`task_bar_view`/
  `app_launcher_view`) only wires widgets to those helpers and reuses them (no
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
  rows only when the visible entry model differs; the per-frame `set_active` only
  re-renders the highlight. Each window's switch uses `clicked` (not `toggled`) so
  programmatic highlight updates never re-fire `activate_window`.
- **App launcher: one `list_apps` fetch per popover open, filtered locally.** A
  header `gtk::MenuButton` ("Open app") opens a `gtk::Popover` with a search entry,
  a scrollable list and a status line; `MenuButton`'s `active` property (read by
  name via `connect_notify_local`, since several GTK traits declare an `active`
  notify handler) triggers a single request when the popover opens, and
  `app_launcher::filter` narrows the cached list in memory on every keystroke. The
  alternative — a debounced server-side `list_apps(Some(query))` — would need its
  own timer and make typing lag the network, for a registry that is small and
  effectively static over a session; the local filter also makes the "never one
  request per keystroke, never busy-poll" rule structural (there is no request path
  off the search signal at all). A failed fetch clears the in-flight flag, so the
  next open retries. Each row is its own `gtk::Button` carrying its launch target
  in its closure — no index bookkeeping, so a rebuild cannot misfire a launch — and
  shows the name (bold) over the desktop-file id, both Pango-escaped through
  `help::escape`. An entry's icon is used only when the icon theme can resolve it,
  else the generic `FALLBACK_ICON`, so an unknown name never renders GTK's
  missing-image placeholder.
- **Launch feedback is a state machine fed by the reply, plus a state refresh.**
  A click moves `app_launcher::LaunchState` to `Launching` (a second click is
  ignored while one is in flight) and the worker's `launch_result` — an outcome or
  an `unknown_app` refusal — moves it to `Launched`/`Failed`, so the pending state
  is always the runtime's, never the click's. `launch_result` never carries the
  launched window, so the worker issues `refresh_state` right after an accepted
  launch and the new window arrives through the ordinary state path (the task bar
  then shows it) — the same refresh the 500 ms ticker and the active-window change
  use, not a second mechanism. A refusal is surfaced twice on purpose: in the
  popover's status line and in the window's notice line, so it stays visible after
  the popover closes.
- **Per-window close: optimistic prune, reconciled on every state.** A close
  button beside each window switch sends the runtime-native `close_window` and
  hides its row immediately (`window_close::CloseControl`), instead of waiting up
  to one refresh tick for the runtime to drop the window — a deliberate close must
  feel like it did something. Every state then reconciles the hidden set against
  the runtime's truth: a hidden window the state still reports is revealed again
  (the close failed or has not taken effect), so a wrong guess self-heals and the
  task bar can never be left missing a live window. A close the worker reports as
  failed is revealed at once, with `window_close::failure_message` in the notice
  line. `close_window` is fire-and-forget, so the worker's failure arm covers
  write/transport failures; the stale-id refusal arrives as an asynchronous VAP
  `error` the client does not expose, and is harmless precisely because the prune
  already removed the (stale) row.
- **The notice line is for user-initiated failures.** `NoticeUi` (a `gtk::Revealer`
  over a flat, click-to-dismiss button) shows the last refused launch or refused
  window close under the connection banner; connection failures keep using the
  banner, so the two never fight over one label. `UiEvent::Notice` (frame-decode
  and seat-input failures) remains log-only, as before.
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
- **The launcher popover takes the keyboard.** The popover's `map` signal grabs
  focus onto its search entry, so typing searches instead of being forwarded to the
  remote desktop the instant the menu opens (the frame view loses focus, and the
  help hint says so).
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
- **Pointer input is per-event, unthrottled and ungated.** `frame_view` sends one
  `InputCommand::Move` per GTK `EventControllerMotion::motion` and one
  `InputCommand::Button` per press/release (decided by `PointerState`), with no
  rate limit and no gate on keyboard focus, control ownership or connection state;
  the only gate is `normalize`/`display_rect` returning `None` (no decoded frame
  yet, i.e. `dimensions == (0,0)`, or a collapsed widget), which drops motion and
  clicks alike. So the *only* way a click is dropped mid-desktop is landing outside
  the displayed image (a letterbox bar), while motion there is clamped and still
  delivered — motion and click fail differently. A `Button` command carries its own
  normalized `(x,y)`, so no separate `pointer_move` precedes a button (a click with
  no prior motion is correct); the server warps the pointer to that position before
  pressing (`crates/adesk-server/src/viewer/backend.rs`). GTK coalesces motion
  before the app sees it, but that cannot drop a click (each carries its position).
- **The event loop takes one widget bundle.** `Widgets` carries the frame view, task
  bar, launcher, notice line, banner, recording control and help model into
  `event_loop_fn(events, widgets)`, so adding a control never grows the loop's
  argument list (clippy's `too_many_arguments`).
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
**129 passed / 0 failed** (all in the lib target; 0 in the bin target, 0 doctests).
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
- `bridge` (10): decodable frame→`Frame` event (carrying the cursor), hidden cursor
  still reported, undecodable frame→`Notice`, `InputHandle::close` stops accepting
  commands, a recording status→`Recording(Ok)` (returning `active`), a recording
  error→`Recording(Err)`, recording commands reach the worker, the launcher commands
  reach the worker, a close command reaches the worker, and the `Apps`/`Launch`/
  `WindowClosed` event arms carry their replies.
- `record` (6): new control is idle/`Record`/`StartRecording`, a recording
  status→Recording/`Stop`/`REC 12s`/`StopRecording`, a finished status→
  Finished/`Record`/`saved <path>`/`StartRecording`, an error status→Idle with
  the reason surfaced, an idle status clears the line, duration truncation.
- `app_launcher` (22): empty/whitespace query lists everything by name; filtering
  matches the name and the desktop-file id case-insensitively and ignores
  surrounding whitespace; no match yields no rows; ordering is case-insensitive by
  name then id; a blank name falls back to the id; a missing/empty icon falls back
  to `FALLBACK_ICON`; row markup escapes the name and the id while the tooltip stays
  plain; a new launcher wants a fetch and has no status; opening the popover fetches
  once and shows `loading applications…`, then the count; the match count follows the
  query; an unmatched query says so; an unchanged query is not reported as a change;
  a failed fetch is surfaced and retried; a launch is pending until the reply and a
  second click is ignored; a correlated launch names its window; a refused launch is
  surfaced in the status line and the notice and leaves the machine retryable; an app
  outside the fetched list launches under its id; a stale reply is ignored; a launch
  failure outranks the list status.
- `window_close` (8): a new control hides nothing; a close request hides the window
  once (a repeated click sends nothing); `restore` reveals a failed close (and is a
  no-op otherwise); reconcile keeps a gone window hidden; reconcile leaves an
  already-confirmed state alone; reconcile with nothing hidden changes nothing; a
  window the runtime still reports is revealed again; the failure message names the
  window.
- `app` (4): the argument vector GTK receives is exactly the program name
  (`program_name_from` passthrough; `None` → `"adesk-viewer-gui"` fallback) — the
  regression pin for the "Unknown option --unix" bug — the escape action's
  accelerator name is the window action (`win.release-control`), and the help
  markup parses as Pango (`gtk::pango::parse_markup`, display-free) with the
  escaped window title rendering back exactly.
- **Untested widget glue (needs a display):** everything inside `app`'s
  `HelpUi`/`RecordUi`/`NoticeUi` construction and `activate()` (header/popover/
  revealer assembly, `gio::SimpleAction` registration, focus handoff),
  `frame_view`'s controllers, overlay and cairo pointer glyph, `task_bar_view`'s
  switch and close buttons (and their `CloseControl` wiring), and
  `app_launcher_view` in full (the `MenuButton`/popover assembly, the `active`
  notify fetch hook, the search signal, `render_rows`, the icon-theme lookup and
  the row buttons). Their decisions are all covered by the GTK-free modules above;
  test pure logic instead, never GUI behavior.

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
- `MenuButton:active` is also declared by `CheckButton`/`ToggleButton`/`ComboBox`/
  `SocketService`, so `connect_active_notify` does not resolve on a `MenuButton`:
  use `glib::ObjectExt::connect_notify_local(Some("active"), …)` and read
  `property::<bool>("active")`, or spell the trait out.
- A `RefCell` `borrow_mut()` used as an `if` condition is released at the end of the
  condition, but write `let x = …borrow_mut()…;` on its own line when the block then
  re-borrows (`rebuild`, `sync_status`), so the intent is unambiguous.
- Pango-escape every runtime-supplied string: `help::escape` is `pub(crate)` and is
  the one escaper (task-bar/launcher labels travel through `set_text`/tooltips and
  need no escaping; the launcher rows and the help panel use markup and do).
- `InputCommand` deliberately has no `Debug` (its `Text` arm must never reach a log),
  so tests match its variants instead of formatting them.

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
- The launcher issues one `list_apps` round trip per popover open and re-renders the
  whole row list per keystroke; that is deliberate (a local filter, never a request
  per keystroke) and fine for a registry of hundreds of entries, but a much larger
  registry would want incremental rendering. A runtime without an app registry
  answers `not_supported`, surfaced in the popover's status line.
- The launched window reaches the task bar through the ordinary state refresh the
  worker issues right after an accepted launch, so it can lag the `launched …`
  status line by up to one round trip (there is no server push of `state`).
- `close_window` is fire-and-forget: `ViewerClient::close_window` resolves once the
  message is written, and the runtime's asynchronous VAP `error` for a stale id
  (e.g. `unknown_window`) is not exposed by the client (it has no public error-stream
  accessor), so that race is absorbed by the optimistic prune + state reconcile
  rather than shown as a notice. Failures the client does report (a write/transport
  failure) are surfaced in the notice line and the row is restored.
- **Input is fire-and-forget with no delivery confirmation, and only keyboard
  input is control-gated.** `set_control(ControlOwner::Human)` is sent once on
  connect (`bridge.rs`); there is no local control-ownership state, so
  `Move`/`Button`/`Scroll` are written to the socket unconditionally and only the
  keyboard path is gated — by the frame view's GTK focus (`focused` cell fed to
  `KeyRouter::press`/`release`, which return `Pass` when unfocused), never by the
  advisory control handshake. The worker applies commands one at a time in its
  `select!` loop and awaits `request_state`/`request_recording` round trips inline,
  so a slow server head-of-line-blocks queued input (it is buffered in an unbounded
  channel, never dropped); a transport failure of an input message surfaces only as
  a log-only `UiEvent::Notice`, and a runtime *rejection* (`invalid_request`,
  `unknown_window`, …) is invisible to the GUI because the client's input methods
  are fire-and-forget and its error broadcast has no public accessor.
- **A cancelled click forwards a press but no release.** `frame_view` connects only
  `GestureClick::pressed`/`released` (per button); it does not handle the gesture's
  `cancel`/`stopped`, so a press whose sequence GTK cancels is never matched by a
  forwarded `Button` release and the remote button can stay down until the next
  click. `pointer::PointerState` pairs only events that actually arrive.
- **VAP capability boundary.** The GUI's only runtime channel is VAP, and VAP's
  client vocabulary is `request_frame`, `request_state`, `set_control`,
  `pointer_move`, `pointer_button`, `scroll`, `key`, `text`, `activate_window`,
  `close_window`, `list_apps`, `launch_app`, `start_recording`, `stop_recording`,
  `request_recording`, `bye` (`docs/viewer.md` §4). There is no VAP message to
  raise, move or resize a window, and `DesktopState` carries windows only (no app
  list) — so the launcher fetches the app list on demand rather than showing a live
  app menu, and there is no "close all".
- No menus, toolbar entries or `gio` keyboard accelerators exist beyond the
  release action: the header bar holds **Open app**, **Release control**, the help
  menu button and the Record/Stop toggle, the bottom bar is the window task bar
  (window switch + per-window close), and all pointer/keyboard/text input is
  implicit on the frame view (the help popover and the focus hint line are the
  on-screen explanation for it).

## Status
Implemented and green: `run()`, the GTK application (frame view + remote-pointer
overlay, task bar with per-window close, app launcher, release-control escape hatch,
recording toggle + status line, help popover + focus hint, notice line, status
banner), the tokio↔GLib bridge, the CLI, and every pure module with unit tests.
Socket resolution goes through `adesk_viewer::resolve_socket_path` (server-parity
default) and connect/disconnect failures name the dialed endpoint, with an
AGP-socket hint when the file name is `adesk.sock`
(`crates/adesk-viewer-gui/src/address.rs`).
Gates all pass through `./scripts/dev.sh`: `cargo build -p adesk-viewer-gui`;
`cargo test -p adesk-viewer-gui` (129 passed); `cargo clippy -p adesk-viewer-gui
--all-targets --no-deps -- -D warnings`; `cargo fmt --all --check`;
`cargo doc -p adesk-viewer-gui --no-deps --document-private-items` (zero warnings);
and `cargo check --workspace --all-targets`.
The window layout is deliberately flat and additive: `adw::ToolbarView` with a
header bar, a vertical content box (banner, hint revealer, recording status, notice
line, frame view) and the task bar as the bottom bar, so another header action or a
side panel can be added without restructuring.
