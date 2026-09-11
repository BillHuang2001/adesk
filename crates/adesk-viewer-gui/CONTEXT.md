# adesk-viewer-gui — GTK4/libadwaita desktop front-end for the ADesk viewer

## Intent
The interactive human front-end of an ADesk runtime (the GUI counterpart of the
headless `adesk-viewer` binary, analogous to `remote-viewer` for a SPICE VM).
It connects to the runtime's VAP viewer endpoint with `adesk_viewer::ViewerClient`,
renders the streamed desktop frames, provides a minimal window **task bar** so a
human can list and switch the tiled windows, and turns local mouse/keyboard/task-bar
activity into VAP messages.
Central principle, inherited from `docs/viewer.md` §5 and the root `CONTEXT.md`:
the human is placed *in the same seat the agent drives* — a viewer action is never a
special code path.
Window switching uses the runtime-native VAP `activate_window` message; pointer/key/
text use the normal seat input path.

## API Surface
- Binary `adesk-viewer-gui` (`./src/main.rs`): the GTK application entry point.
- Library `adesk_viewer_gui` (`./src/lib.rs`).
- (Filled in as the app is implemented — see Routing Table.)

## Constraints
- **This is the only GTK4/libadwaita crate in the workspace.** No other crate may
  gain a GTK dependency; the headless `adesk-viewer` crate stays GTK-free.
- GTK4 + libadwaita come from the Nix dev shell / package
  (`flake.nix` `adeskGuiLibraries`: `gtk4`, `libadwaita`); Rust deps `gtk4` and the
  `adw` alias for `libadwaita` are declared once in the root `Cargo.toml`
  `[workspace.dependencies]` and referenced with `<dep>.workspace = true`.
  (libadwaita 0.9 publishes its lib as `libadwaita`; it is aliased to `adw`.)
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay under the
  ~1000-line threshold.
- All human input goes through `adesk_viewer::ViewerClient` (the same VAP seat path
  the agent's input uses) — never a bespoke or direct-to-compositor path.
- **Separate everything that is testable without a display.** Coordinate mapping,
  the task-bar model, and any state machine must live in GTK-free modules with
  unit tests; the GUI tests must never require a display, GPU, or network.
- The binary connects over VAP to the runtime's viewer endpoint (Unix socket by
  default); it never links the compositor or Smithay.

## Routing Table
| Area | Owner |
|---|---|
| Crate root, re-exports | `./src/lib.rs` |
| GTK application entry point (binary) | `./src/main.rs` |
| (Implemented areas are added here as the app is built.) | |

## Status
Skeleton only: `Cargo.toml`, `src/lib.rs` and `src/main.rs` (a minimal
`adw::Application`) exist and build against GTK4/libadwaita via the dev shell;
the frame view, task bar and input routing are not yet implemented.
