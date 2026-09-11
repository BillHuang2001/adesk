//! The GTK4/libadwaita application: builds the window and wires the widgets to
//! the tokio ↔ GLib bridge.
//!
//! The layout is an [`adw::ApplicationWindow`] exactly `1024x640` by default,
//! containing an [`adw::ToolbarView`]: an [`adw::HeaderBar`] on top, the desktop
//! [frame view](crate::frame_view) as the (expanding) content and the
//! [task bar](crate::task_bar_view) as the bottom bar. An [`adw::Banner`] on top
//! of the content reports connection state.
//!
//! The GTK main loop never blocks on the network: connecting happens on the
//! bridge worker thread, and every result arrives as a [`UiEvent`] through a
//! `glib::spawn_future_local` loop. On window close the bridge is closed and the
//! loop aborted, so the worker thread ends and nothing leaks.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use tokio::sync::mpsc::UnboundedReceiver;

use adesk_viewer::ViewerTarget;

use crate::bridge::{Bridge, UiEvent};
use crate::frame_view::FrameView;
use crate::task_bar_view::TaskBarView;

/// The GTK application id (`docs/viewer.md`, GUI front-end).
const APP_ID: &str = "org.adesk.Viewer";

/// Builds the application for `target` and runs the GTK main loop.
///
/// Returns the GTK exit code; the connection itself never fails the process.
pub(crate) fn run_application(target: ViewerTarget) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| activate(app, target.clone()));
    app.run()
}

/// Builds the window and event loop for a single `activate` (window creation).
fn activate(app: &adw::Application, target: ViewerTarget) {
    let bridge = Bridge::connect(target.clone());
    let input = bridge.input();
    let events = bridge.into_events();

    let frame_view = FrameView::new(input.clone());
    let task_bar = TaskBarView::new(input.clone());

    let banner = adw::Banner::builder()
        .title(format!("Connecting to {target}…"))
        .revealed(true)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&frame_view.widget());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.add_bottom_bar(&task_bar.widget());
    toolbar.set_content(Some(&content));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("ADesk Viewer")
        .default_width(1024)
        .default_height(640)
        .content(&toolbar)
        .build();

    // The event-loop future, kept abortable so window close ends it promptly.
    let event_loop: Rc<RefCell<Option<glib::JoinHandle<()>>>> = Rc::new(RefCell::new(None));

    let close_input = input.clone();
    let close_loop = event_loop.clone();
    window.connect_close_request(move |_| {
        // Closing the input handle drops the worker's command sender, so its
        // `recv()` resolves to `None` and the thread ends.
        close_input.close();
        if let Some(handle) = close_loop.borrow_mut().take() {
            handle.abort();
        }
        glib::Propagation::Proceed
    });

    window.present();
    // Give the frame view keyboard focus up front so typing works without a
    // preceding click (a click re-grabs it anyway).
    frame_view.widget().grab_focus();

    let handle = glib::spawn_future_local(event_loop_fn(events, frame_view, task_bar, banner));
    *event_loop.borrow_mut() = Some(handle);
}

/// Consumes [`UiEvent`]s from the worker and updates the widgets until the
/// worker drops its sender (which ends this loop).
async fn event_loop_fn(
    mut events: UnboundedReceiver<UiEvent>,
    frame_view: FrameView,
    task_bar: TaskBarView,
    banner: adw::Banner,
) {
    while let Some(event) = events.recv().await {
        match event {
            UiEvent::Connected { target, hello } => {
                tracing::info!(
                    %target,
                    width = hello.output.w,
                    height = hello.output.h,
                    renderer = ?hello.renderer,
                    "viewer connected"
                );
                banner.set_revealed(false);
            }
            UiEvent::State(state) => task_bar.update_state(&state),
            UiEvent::Frame {
                seq,
                ts_ms,
                image,
                active_window_id,
            } => {
                tracing::trace!(seq, ts_ms, "rendered a viewer frame");
                frame_view.set_frame(image);
                task_bar.set_active(active_window_id);
            }
            UiEvent::Disconnected(message) => {
                tracing::warn!(%message, "viewer disconnected");
                banner.set_title(&format!("Connection lost: {message}"));
                banner.set_revealed(true);
            }
            UiEvent::Notice(message) => {
                tracing::debug!(%message, "viewer notice");
            }
        }
    }
}
