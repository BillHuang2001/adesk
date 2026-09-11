//! The task bar: a horizontal strip of window toggles that switches the visible
//! window via the runtime-native VAP `activate_window` message.
//!
//! A click on an entry sends [`InputCommand::ActivateWindow`] — the same
//! runtime-native path the agent uses, never synthesized input. The view model
//! (entries, active flag) comes from the pure [`crate::taskbar`] module; this
//! file is only the widget wiring.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use adesk_core::WindowId;
use adesk_viewer_proto::DesktopState;

use crate::bridge::{InputCommand, InputHandle};
use crate::taskbar::{self, TaskBarEntry};

/// The task bar widget plus its window list model.
pub(crate) struct TaskBarView {
    /// The scrolling container placed in the layout.
    scrolled: gtk::ScrolledWindow,
    /// The horizontal row of toggle buttons.
    row: gtk::Box,
    /// The current entries (the model shared with the buttons).
    model: Rc<RefCell<Vec<TaskBarEntry>>>,
    /// The toggle button for each entry, in the same order as `model`.
    buttons: Rc<RefCell<Vec<gtk::ToggleButton>>>,
    /// Enqueues `activate_window` commands.
    input: InputHandle,
}

impl TaskBarView {
    /// Builds an empty task bar.
    pub(crate) fn new(input: InputHandle) -> TaskBarView {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        scrolled.set_child(Some(&row));

        TaskBarView {
            scrolled,
            row,
            model: Rc::new(RefCell::new(Vec::new())),
            buttons: Rc::new(RefCell::new(Vec::new())),
            input,
        }
    }

    /// The widget to place as the window's bottom bar.
    pub(crate) fn widget(&self) -> gtk::ScrolledWindow {
        self.scrolled.clone()
    }

    /// Rebuilds the rows from `state`, but only when the window list changed.
    ///
    /// This is the refresh-on-state path: it avoids rebuilding the buttons on
    /// every (frequently refreshed) state message that carries the same windows.
    pub(crate) fn update_state(&self, state: &DesktopState) {
        let entries = taskbar::entries(state);
        if entries == *self.model.borrow() {
            return;
        }
        *self.model.borrow_mut() = entries;
        self.rebuild();
    }

    /// Updates the active highlight from a frame's active window, without a full
    /// rebuild.
    pub(crate) fn set_active(&self, active: Option<WindowId>) {
        taskbar::set_active(&mut self.model.borrow_mut(), active);
        apply_highlight(&self.buttons.borrow(), &self.model.borrow());
    }

    /// Replaces the row's children with a toggle per entry.
    fn rebuild(&self) {
        while let Some(child) = self.row.first_child() {
            self.row.remove(&child);
        }

        let entries = self.model.borrow();
        let mut new_buttons = Vec::with_capacity(entries.len());
        for entry in entries.iter() {
            let button = gtk::ToggleButton::new();
            button.set_label(&entry.label);
            button.set_active(entry.active);

            let input = self.input.clone();
            let model = self.model.clone();
            let buttons = self.buttons.clone();
            let window_id = entry.window_id;
            button.connect_clicked(move |_| {
                // Runtime-native switch, then optimistically highlight so the UI
                // reacts before the next state arrives.
                input.send(InputCommand::ActivateWindow(window_id));
                taskbar::set_active(&mut model.borrow_mut(), Some(window_id));
                apply_highlight(&buttons.borrow(), &model.borrow());
            });

            self.row.append(&button);
            new_buttons.push(button);
        }

        *self.buttons.borrow_mut() = new_buttons;
    }
}

/// Applies each entry's `active` flag to its toggle button.
fn apply_highlight(buttons: &[gtk::ToggleButton], entries: &[TaskBarEntry]) {
    for (button, entry) in buttons.iter().zip(entries) {
        button.set_active(entry.active);
    }
}
