//! The GTK4/libadwaita application: builds the window and wires the widgets to
//! the tokio ↔ GLib bridge.
//!
//! The layout is an [`adw::ApplicationWindow`] exactly `1024x640` by default,
//! containing an [`adw::ToolbarView`]: an [`adw::HeaderBar`] on top, the desktop
//! [frame view](crate::frame_view) as the (expanding) content and the
//! [task bar](crate::task_bar_view) as the bottom bar. An [`adw::Banner`] on top
//! of the content reports connection state, a hint line below it says so while
//! the desktop does not hold the keyboard, and the header carries the escape
//! hatch, the recording toggle and the help affordance — so a human who connects
//! can see at a glance what is dialled, what is on screen and what they can do.
//!
//! The GTK main loop never blocks on the network: connecting happens on the
//! bridge worker thread, and every result arrives as a [`UiEvent`] through a
//! `glib::spawn_future_local` loop. On window close the bridge is closed and the
//! loop aborted, so the worker thread ends and nothing leaks.
//!
//! All of the text this window shows is composed by GTK-free modules
//! ([`crate::help`], [`crate::record`]); this file only paints it.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk4 as gtk;
use tokio::sync::mpsc::UnboundedReceiver;

use adesk_viewer::ViewerTarget;
use adesk_viewer_proto::RecordingStatus;

use crate::app_launcher_view::AppLauncherView;
use crate::bridge::{Bridge, InputHandle, UiEvent};
use crate::frame_view::FrameView;
use crate::help::{self, ConnectionState, Help};
use crate::keystroke;
use crate::record::RecordControl;
use crate::task_bar_view::TaskBarView;
use crate::window_close;

/// The GTK application id (`docs/viewer.md`, GUI front-end).
const APP_ID: &str = "org.adesk.Viewer";

/// The window action that releases keyboard control, reachable from the header
/// button and from [`keystroke::ESCAPE_ACCELERATOR`].
const RELEASE_ACTION: &str = "win.release-control";

/// Builds the application for `target` and runs the GTK main loop with
/// `program_name` as the only argument.
///
/// `ApplicationExtManual::run` would hand the real process `argv` to
/// `g_application_run`, whose GOptionContext knows nothing about the
/// already-parsed `--unix`/`--tcp`/`--log` flags and rejects them with
/// "Unknown option --unix"; passing the program name alone keeps the parsed
/// command line out of GTK. Returns the GTK exit code; the connection itself
/// never fails the process.
pub(crate) fn run_application(target: ViewerTarget, program_name: &OsStr) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| activate(app, target.clone()));
    app.run_with_args_os(&[program_name])
}

/// The process' own program name (`argv[0]`), or `"adesk-viewer-gui"` when the
/// environment provides none.
///
/// Reads the process environment once and delegates to [`program_name_from`].
pub(crate) fn program_name() -> OsString {
    program_name_from(std::env::args_os().next())
}

/// Pure program-name helper: returns `argv0` when present, otherwise the
/// fallback `"adesk-viewer-gui"`.
///
/// Taking the `argv[0]` value as a parameter keeps the logic testable without
/// mutating process-global state, and `args_os` never panics where
/// `args` would on a non-UTF-8 `argv[0]`.
fn program_name_from(argv0: Option<OsString>) -> OsString {
    argv0.unwrap_or_else(|| OsString::from("adesk-viewer-gui"))
}

/// Builds the window and event loop for a single `activate` (window creation).
fn activate(app: &adw::Application, target: ViewerTarget) {
    let bridge = Bridge::connect(target.clone());
    let input = bridge.input();
    let events = bridge.into_events();

    // The status/help affordance and the focus hint, both fed by one model.
    let help_ui = Rc::new(HelpUi::new(&target.to_string()));

    let frame_view = {
        // The frame view reports its keyboard-focus changes so the hint line can
        // appear the moment the desktop stops receiving keys.
        let focus_ui = help_ui.clone();
        FrameView::new(input.clone(), move |focused| focus_ui.set_focused(focused))
    };
    let task_bar = TaskBarView::new(input.clone());
    // The application launcher: a header "Open app" menu listing the runtime's
    // applications and launching the chosen one (`docs/viewer.md` §3, §5).
    let launcher = AppLauncherView::new(input.clone());

    let banner = adw::Banner::builder()
        .title(format!("Connecting to {target}…"))
        .revealed(true)
        .build();

    // The global notice line: a refused launch or a refused window close has to
    // stay visible after the popover that started it closed.
    let notice = NoticeUi::new();

    // The recording control: a header toggle that drives the worker, plus a
    // status line. Both are updated from the server's status (never just the
    // local click).
    let record = RecordUi::new(&input);

    // The escape hatch's visible half: one click moves keyboard focus off the
    // desktop, so the human gets their keys back without terminating anything.
    let release_button = gtk::Button::with_label("Release control");
    release_button.set_tooltip_text(Some(&format!(
        "Move the keyboard back to this window ({})",
        keystroke::ESCAPE_LABEL
    )));

    let header = adw::HeaderBar::new();
    header.pack_start(&launcher.widget());
    header.pack_start(&release_button);
    header.pack_end(&help_ui.menu);
    header.pack_end(&record.button);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&help_ui.revealer);
    content.append(&record.status);
    content.append(&notice.widget());
    content.append(&frame_view.widget());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&task_bar.widget());
    toolbar.set_content(Some(&content));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("ADesk Viewer")
        .default_width(1024)
        .default_height(640)
        .content(&toolbar)
        .build();

    // One release routine behind both halves of the escape hatch: the header
    // button and the Ctrl+Alt+Escape accelerator. Keyboard focus is moved onto
    // the button so GTK (and the human) can see the desktop is no longer
    // receiving keys; clicking the desktop takes control back.
    let release_action = gio::SimpleAction::new("release-control", None);
    let action_window = window.clone();
    let action_button = release_button.clone();
    release_action.connect_activate(move |_, _| release_control(&action_window, &action_button));
    window.add_action(&release_action);
    app.set_accels_for_action(RELEASE_ACTION, &[keystroke::ESCAPE_ACCELERATOR]);

    let click_window = window.clone();
    let click_button = release_button.clone();
    release_button.connect_clicked(move |_| release_control(&click_window, &click_button));

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
    // The desktop starts with the keyboard, so typing works without a preceding
    // click; the header button (or Ctrl+Alt+Escape) releases it. The report
    // below keeps the hint honest when the window cannot take focus at all.
    frame_view.focus();
    help_ui.set_focused(frame_view.is_focused());

    let handle = glib::spawn_future_local(event_loop_fn(
        events,
        Widgets {
            frame_view,
            task_bar,
            launcher,
            notice,
            banner,
            record,
            help_ui,
        },
    ));
    *event_loop.borrow_mut() = Some(handle);
}

/// The escape hatch: moves keyboard focus off the desktop (onto `target`), so
/// GTK handles the human's keys again until they click the desktop.
///
/// Reached from the header's "Release control" button and from the
/// [`keystroke::ESCAPE_ACCELERATOR`] accelerator, so a human whose keys are all
/// going to the remote app still has a way back out.
fn release_control(window: &adw::ApplicationWindow, target: &gtk::Button) {
    tracing::debug!("viewer keyboard control released");
    // `set_focus` exists on both `GtkWindowExt` and `RootExt`; the window's is
    // the one that moves the keyboard focus within this window.
    gtk::prelude::GtkWindowExt::set_focus(window, Some(target));
}

/// The help affordance: a header menu button with a popover (endpoint, active
/// window, connection, control and the capability list) plus the one-line hint
/// shown while the desktop does not hold the keyboard.
///
/// Everything it shows comes from the GTK-free [`Help`] model; this type is only
/// the widget glue that paints it.
struct HelpUi {
    /// The model behind every text this affordance shows.
    help: Rc<RefCell<Help>>,
    /// The header button that opens the popover.
    menu: gtk::MenuButton,
    /// The popover's body.
    body: gtk::Label,
    /// Reveals the hint line while the desktop lacks keyboard control.
    revealer: gtk::Revealer,
    /// The hint line's label.
    hint: gtk::Label,
}

impl HelpUi {
    /// Builds the affordance for the endpoint `target`, showing the model's
    /// current text.
    ///
    /// A fresh [`Help`] model reports that the desktop holds keyboard control, so
    /// the hint starts hidden; the first [`HelpUi::set_focused`] report reveals it
    /// if that is not the case.
    fn new(target: &str) -> HelpUi {
        let help = Rc::new(RefCell::new(Help::new(target)));

        let body = gtk::Label::new(None);
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.set_size_request(380, -1);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&body));

        let menu = gtk::MenuButton::new();
        menu.set_icon_name("help-about-symbolic");
        menu.set_popover(Some(&popover));

        let hint = gtk::Label::new(None);
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.set_margin_top(4);
        hint.set_margin_bottom(4);
        hint.set_margin_start(12);
        hint.set_margin_end(12);
        hint.add_css_class("dim-label");

        let revealer = gtk::Revealer::new();
        revealer.set_child(Some(&hint));

        let ui = HelpUi {
            help,
            menu,
            body,
            revealer,
            hint,
        };
        ui.sync();
        ui
    }

    /// Records whether the desktop holds keyboard control and re-renders.
    fn set_focused(&self, focused: bool) {
        self.help.borrow_mut().set_focused(focused);
        self.sync();
    }

    /// Re-renders every widget from the model.
    fn sync(&self) {
        let help = self.help.borrow();
        self.menu.set_tooltip_text(Some(&help.summary()));
        self.body.set_markup(&help.markup());
        match help::focus_hint(help.is_focused()) {
            Some(text) => {
                self.hint.set_text(text);
                self.revealer.set_reveal_child(true);
            }
            None => self.revealer.set_reveal_child(false),
        }
    }

    /// Applies `edit` to the model, re-rendering only when it reports a change.
    fn update(&self, edit: impl FnOnce(&mut Help) -> bool) {
        if edit(&mut self.help.borrow_mut()) {
            self.sync();
        }
    }
}

/// Consumes [`UiEvent`]s from the worker and updates the widgets until the
/// worker drops its sender (which ends this loop).
///
/// The widgets travel as one [`Widgets`] bundle so adding a control never grows
/// this function's signature.
async fn event_loop_fn(mut events: UnboundedReceiver<UiEvent>, widgets: Widgets) {
    let Widgets {
        frame_view,
        task_bar,
        launcher,
        notice,
        banner,
        record,
        help_ui,
    } = widgets;

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
                help_ui.update(|help| help.set_state(ConnectionState::Connected));
            }
            UiEvent::State(state) => {
                task_bar.update_state(&state);
                help_ui.update(|help| help.set_window(help::active_window_label(&state)));
            }
            UiEvent::Frame {
                seq,
                ts_ms,
                image,
                cursor,
                active_window_id,
            } => {
                tracing::trace!(seq, ts_ms, "rendered a viewer frame");
                frame_view.set_frame(image, cursor);
                task_bar.set_active(active_window_id);
            }
            UiEvent::Recording(result) => record.apply(result),
            UiEvent::Apps(result) => launcher.apply_list(result),
            UiEvent::Launch(result) => {
                // A refused launch is surfaced twice on purpose: in the
                // launcher's own status line and in the window's notice line, so
                // it is still visible once the popover closes.
                if let Some(message) = launcher.apply_launch(result) {
                    tracing::debug!(%message, "application launch failed");
                    notice.show(&message);
                }
            }
            UiEvent::WindowClosed { window_id, result } => match result {
                // The runtime acknowledged the close; the row was already
                // pruned optimistically and the next state confirms it.
                Ok(()) => tracing::debug!(window = %window_id, "window closed"),
                // A close the runtime refused (e.g. `unknown_window` for a
                // window that had already gone) leaves the connection open: the
                // row comes back and the reason is shown, never swallowed.
                Err(message) => {
                    tracing::debug!(window = %window_id, %message, "window close failed");
                    task_bar.restore(window_id);
                    notice.show(&window_close::failure_message(window_id, &message));
                }
            },
            UiEvent::ConnectFailed(message) => {
                tracing::warn!(%message, "viewer connection failed");
                banner.set_title(&message);
                banner.set_revealed(true);
                help_ui.update(|help| help.set_state(ConnectionState::Failed(message)));
            }
            UiEvent::Disconnected(message) => {
                tracing::warn!(%message, "viewer disconnected");
                banner.set_title(&message);
                banner.set_revealed(true);
                help_ui.update(|help| help.set_state(ConnectionState::Failed(message)));
            }
            UiEvent::Notice(message) => {
                tracing::debug!(%message, "viewer notice");
            }
        }
    }
}

/// The window's widget set, handed to the event loop in one piece.
struct Widgets {
    /// Renders the desktop frames and the remote pointer.
    frame_view: FrameView,
    /// Lists and switches the runtime's windows.
    task_bar: TaskBarView,
    /// Lists and launches the runtime's applications.
    launcher: AppLauncherView,
    /// The window's global notice line.
    notice: NoticeUi,
    /// The connection banner.
    banner: adw::Banner,
    /// The recording toggle and its status line.
    record: RecordUi,
    /// The help/status model.
    help_ui: Rc<HelpUi>,
}

/// The window's global notice line: the last action the runtime refused (a
/// launch, a window close), shown under the connection banner until a click
/// dismisses it or a newer notice replaces it.
///
/// Connection failures keep the banner, so this line is only for failures of
/// things the human did — a launch or a close must be visible, never silently
/// swallowed, even after the popover that started it has closed.
struct NoticeUi {
    /// Reveals the line while it carries a message.
    revealer: gtk::Revealer,
    /// The message itself.
    label: gtk::Label,
}

impl NoticeUi {
    /// Builds the (initially hidden) notice line.
    fn new() -> NoticeUi {
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        label.set_margin_start(12);
        label.set_margin_end(12);

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&label));
        button.set_tooltip_text(Some("Click to dismiss this message"));

        let revealer = gtk::Revealer::new();
        revealer.set_child(Some(&button));

        let dismiss = revealer.clone();
        button.connect_clicked(move |_| dismiss.set_reveal_child(false));

        NoticeUi { revealer, label }
    }

    /// The widget to place in the window's content box.
    fn widget(&self) -> gtk::Revealer {
        self.revealer.clone()
    }

    /// Shows `message`, replacing any previous one.
    fn show(&self, message: &str) {
        self.label.set_text(message);
        self.revealer.set_reveal_child(true);
    }
}

/// The recording affordance: the header toggle, its status line and the pure
/// [`RecordControl`] state machine behind both.
///
/// A click only sends the next command; the label and the active state always
/// come back from the server's status, so a runtime that refuses the request
/// cannot leave the toggle lying about what is happening.
struct RecordUi {
    /// The recording model.
    control: Rc<RefCell<RecordControl>>,
    /// The header toggle (placed in the header bar).
    button: gtk::ToggleButton,
    /// The status line (placed under the connection banner).
    status: gtk::Label,
}

impl RecordUi {
    /// Builds the toggle and the status line, and wires the toggle to `input`.
    fn new(input: &InputHandle) -> RecordUi {
        let button = gtk::ToggleButton::new();
        button.set_label("Record");
        button.set_tooltip_text(Some("Start or stop a recording of the remote desktop"));

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_visible(false);

        let ui = RecordUi {
            control: Rc::new(RefCell::new(RecordControl::new())),
            button,
            status,
        };

        let control = ui.control.clone();
        let input = input.clone();
        ui.button.connect_clicked(move |_| {
            input.send(control.borrow().toggle_command());
        });

        ui
    }

    /// Applies the worker's answer to a recording command or status refresh.
    fn apply(&self, result: Result<RecordingStatus, String>) {
        match result {
            Ok(status) => {
                self.control.borrow_mut().apply(status);
                self.sync();
            }
            Err(message) => {
                tracing::debug!(%message, "viewer recording command failed");
                self.status
                    .set_text(&format!("recording failed: {message}"));
                self.status.set_visible(true);
            }
        }
    }

    /// Reflects the model onto the toggle and the status line (the reactive
    /// path: the server's status always wins over the click).
    fn sync(&self) {
        let control = self.control.borrow();
        self.button.set_label(control.button_label());
        self.button.set_active(control.is_recording());
        match control.status_text() {
            Some(text) => {
                self.status.set_text(&text);
                self.status.set_visible(true);
            }
            None => self.status.set_visible(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_gets_the_program_name_alone() {
        // The regression: `run_application` must hand GTK only `argv[0]`, never
        // the parsed-away flags (GTK's GOption rejects them with
        // "Unknown option --unix"). This pins the argument vector computed by
        // `program_name_from`, the exact slice `run_with_args_os` receives.
        assert_eq!(
            program_name_from(Some(OsString::from("/usr/bin/adesk-viewer-gui"))),
            OsString::from("/usr/bin/adesk-viewer-gui")
        );
        assert_eq!(
            program_name_from(Some(OsString::from("adesk-viewer-gui"))),
            OsString::from("adesk-viewer-gui")
        );
    }

    #[test]
    fn a_missing_argv0_falls_back_to_the_binary_name() {
        assert_eq!(program_name_from(None), OsString::from("adesk-viewer-gui"));
    }

    #[test]
    fn the_release_action_is_a_window_action() {
        // The action the escape hatch's accelerator names must be the window
        // action added above (`win.` prefix), or GTK would silently never fire it.
        assert_eq!(RELEASE_ACTION, "win.release-control");
        assert!(keystroke::ESCAPE_ACCELERATOR.ends_with(keystroke::ESCAPE_KEY));
    }

    #[test]
    fn the_help_markup_is_valid_pango() {
        // `HelpUi` hands this string to `gtk::Label::set_markup`: malformed markup
        // would leave the help panel empty (GTK bails out on the parse), which is
        // the invisible-affordance failure this front-end exists to avoid. Parsing
        // needs no display, and the plain text it returns proves the escaping is
        // render-exact.
        let mut help = Help::new("unix:/run/user/1000/adesk-viewer.sock");
        help.set_state(ConnectionState::Connected);
        help.set_window(Some("a <weird> & window title".to_owned()));

        let (_, text, _) =
            gtk::pango::parse_markup(&help.markup(), '\0').expect("valid Pango markup");

        assert!(
            text.contains("Endpoint unix:/run/user/1000/adesk-viewer.sock"),
            "{text}"
        );
        assert!(text.contains("a <weird> & window title"), "{text}");
        assert!(text.contains(keystroke::ESCAPE_LABEL), "{text}");
    }
}
