//! The application launcher's widgets: the header **Open app** menu button, its
//! popover (a search entry over a scrollable application list) and the status
//! line under it.
//!
//! Thin glue over the GTK-free [`crate::app_launcher`] model — filtering,
//! ordering, the launch state machine and every string live there; this file
//! only builds the widgets and forwards signals:
//! - opening the popover asks the worker for the application list **once**
//!   (never per keystroke), and every keystroke after that re-renders the rows
//!   from the cached list locally;
//! - clicking a row sends [`InputCommand::LaunchApp`] and shows the pending
//!   state until the worker's reply arrives.
//!
//! The launcher is reached over VAP only (like the task bar), so it lists and
//! launches applications through the same seat-independent runtime path the rest
//! of the crate uses (`docs/viewer.md` §4, §5).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use adesk_viewer_proto::{AppEntry, LaunchOutcome};

use crate::app_launcher::{self, AppLauncher, AppRow, LaunchState};
use crate::bridge::{InputCommand, InputHandle};

/// The launcher's widget set (built once, re-rendered from the model).
pub(crate) struct AppLauncherView {
    /// The header affordance that opens the popover.
    button: gtk::MenuButton,
    /// The popover itself (closed after a successful launch).
    popover: gtk::Popover,
    /// The vertical container the row buttons are rebuilt into.
    list: gtk::Box,
    /// The search box; its changes filter locally.
    search: gtk::SearchEntry,
    /// The status line: loading, match count or the last launch outcome.
    status: gtk::Label,
    /// The GTK-free model behind every decision.
    model: Rc<RefCell<AppLauncher>>,
    /// Enqueues `list_apps`/`launch_app` commands.
    input: InputHandle,
}

impl AppLauncherView {
    /// Builds the affordance and wires it to `input`.
    pub(crate) fn new(input: InputHandle) -> AppLauncherView {
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search applications"));

        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&list));
        scrolled.set_min_content_height(240);
        scrolled.set_vexpand(true);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("dim-label");

        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.set_margin_top(8);
        body.set_margin_bottom(8);
        body.set_margin_start(8);
        body.set_margin_end(8);
        body.set_size_request(360, -1);
        body.append(&search);
        body.append(&scrolled);
        body.append(&status);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&body));

        let button = gtk::MenuButton::new();
        button.set_label("Open app");
        button.set_tooltip_text(Some(
            "List and launch the applications this runtime can start",
        ));
        button.set_popover(Some(&popover));

        let view = AppLauncherView {
            button,
            popover,
            list,
            search,
            status,
            model: Rc::new(RefCell::new(AppLauncher::new())),
            input,
        };
        view.wire();
        view.sync();
        view
    }

    /// The widget to place in the header bar.
    pub(crate) fn widget(&self) -> gtk::MenuButton {
        self.button.clone()
    }

    /// Applies the worker's `list_apps` reply and re-renders.
    pub(crate) fn apply_list(&self, result: Result<Vec<AppEntry>, String>) {
        self.model.borrow_mut().apply_list(result);
        self.sync();
    }

    /// Applies the worker's `launch_app` reply and re-renders.
    ///
    /// Returns the notice text to show outside the popover when the launch
    /// failed (so a refusal stays visible after the popover closes), or [`None`]
    /// otherwise. A successful launch closes the popover: the human asked for
    /// the application, and the window now shows up in the task bar instead.
    pub(crate) fn apply_launch(&self, result: Result<LaunchOutcome, String>) -> Option<String> {
        let (launched, notice) = {
            let mut model = self.model.borrow_mut();
            model.apply_launch(result);
            (
                matches!(model.launch(), LaunchState::Launched { .. }),
                model.launch_notice(),
            )
        };
        if launched {
            self.popover.popdown();
        }
        self.sync();
        notice
    }

    /// Hooks the widget signals onto the model.
    fn wire(&self) {
        // Typing filters the cached list in memory — never a request.
        let model = self.model.clone();
        let list = self.list.clone();
        let status = self.status.clone();
        let input = self.input.clone();
        self.search.connect_search_changed(move |entry| {
            // The borrow ends with the statement, before `render_rows` re-borrows.
            let changed = model.borrow_mut().set_query(entry.text().as_str());
            if !changed {
                return;
            }
            render_rows(&list, &model, &status, &input);
        });

        // Opening the popover asks for the list once (a failed fetch is retried
        // on the next open). `MenuButton:active` is true exactly while its
        // popover is shown; the property is read by name because several GTK
        // traits declare an `active` notify handler.
        let model = self.model.clone();
        let status = self.status.clone();
        let input = self.input.clone();
        self.button
            .connect_notify_local(Some("active"), move |button, _| {
                if button.property::<bool>("active") {
                    fetch_if_needed(&model, &status, &input);
                }
            });

        // Typing must reach the search box, not the remote desktop, the moment
        // the popover appears.
        let search = self.search.clone();
        self.popover.connect_map(move |_| {
            search.grab_focus();
        });
    }

    /// Re-renders the rows and the status line from the model.
    fn sync(&self) {
        render_rows(&self.list, &self.model, &self.status, &self.input);
    }
}

/// Asks the worker for the application list when no fetch is in flight.
fn fetch_if_needed(model: &Rc<RefCell<AppLauncher>>, status: &gtk::Label, input: &InputHandle) {
    {
        let mut model = model.borrow_mut();
        if !model.should_fetch() {
            return;
        }
        model.begin_fetch();
    }
    input.send(InputCommand::ListApps);
    sync_status(&model.borrow(), status);
}

/// Replaces the row buttons with those of the model's current query.
fn render_rows(
    list: &gtk::Box,
    model: &Rc<RefCell<AppLauncher>>,
    status: &gtk::Label,
    input: &InputHandle,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let theme = gtk::IconTheme::for_display(&list.display());
    let rows = model.borrow().rows();
    for row in &rows {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_tooltip_text(Some(&row.tooltip()));

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let image = gtk::Image::from_icon_name(&icon_for(row, &theme));
        image.set_pixel_size(24);
        let label = gtk::Label::new(None);
        label.set_markup(&row.markup());
        label.set_xalign(0.0);
        content.append(&image);
        content.append(&label);
        button.set_child(Some(&content));

        // One closure per row, carrying its own launch target: no index
        // bookkeeping, so a rebuild cannot misfire a launch.
        let model = model.clone();
        let status = status.clone();
        let input = input.clone();
        let app_id = row.id.clone();
        button.connect_clicked(move |_| {
            // The borrow ends with the statement; `sync_status` re-borrows below.
            let started = model.borrow_mut().begin_launch(&app_id);
            if !started {
                return;
            }
            input.send(InputCommand::LaunchApp(app_id.clone()));
            sync_status(&model.borrow(), &status);
        });

        list.append(&button);
    }

    sync_status(&model.borrow(), status);
}

/// Paints the model's status line, hiding it when there is nothing to say.
fn sync_status(model: &AppLauncher, status: &gtk::Label) {
    match model.status_text() {
        Some(text) => {
            status.set_text(&text);
            status.set_visible(true);
        }
        None => status.set_visible(false),
    }
}

/// The icon name to hand GTK for `row`: the entry's own name when the theme can
/// resolve it, else the generic fallback, so an unknown icon name never renders
/// as GTK's missing-image placeholder.
fn icon_for(row: &AppRow, theme: &gtk::IconTheme) -> String {
    let name = row.icon_name();
    if theme.has_icon(name) {
        name.to_owned()
    } else {
        app_launcher::FALLBACK_ICON.to_owned()
    }
}
