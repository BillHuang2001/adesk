//! `adesk-viewer-gui` — the GTK4/libadwaita viewer binary.
//!
//! Thin entry point only; the application lives in [`adesk_viewer_gui::run`].
#![forbid(unsafe_code)]
#![deny(missing_docs)]

fn main() -> gtk4::glib::ExitCode {
    adesk_viewer_gui::run()
}
