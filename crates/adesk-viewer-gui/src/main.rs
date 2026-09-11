//! `adesk-viewer-gui` — the GTK4/libadwaita viewer binary.
//!
//! Thin entry point only; the application lives in [`adesk_viewer_gui`].
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use adw::prelude::*;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ADESK_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let app = adw::Application::builder()
        .application_id("org.adesk.Viewer")
        .build();
    app.connect_activate(|app| {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("ADesk Viewer")
            .default_width(1024)
            .default_height(640)
            .build();
        window.present();
    });
    // `run` returns an exit code; the GUI itself never fails to start.
    let _ = app.run();
    Ok(())
}
