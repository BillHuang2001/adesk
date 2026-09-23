//! The task bar: a horizontal strip of window toggles that switches the visible
//! window via the runtime-native VAP `activate_window` message, each with a
//! close control that sends the runtime-native `close_window` message.
//!
//! Both are runtime-native paths — the same ones the agent uses, never
//! synthesized input. The view model (entries, active flag) comes from the pure
//! [`crate::taskbar`] module and the optimistic-close decision from
//! [`crate::window_close`]; this file is only the widget wiring, so the buttons
//! rebuild from those models rather than holding state of their own.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use adesk_core::WindowId;
use adesk_viewer_proto::DesktopState;

use crate::bridge::{InputCommand, InputHandle};
use crate::taskbar::{self, TaskBarEntry};
use crate::window_close::CloseControl;

/// The task bar widget plus its window list model.
pub(crate) struct TaskBarView {
    /// The scrolling container placed in the layout.
    scrolled: gtk::ScrolledWindow,
    /// The horizontal row of window groups.
    row: gtk::Box,
    /// The current entries (the model shared with the buttons).
    model: Rc<RefCell<Vec<TaskBarEntry>>>,
    /// The switch button for each *visible* entry, in the same order as
    /// [`visible`].
    buttons: Rc<RefCell<Vec<gtk::ToggleButton>>>,
    /// The optimistic-close bookkeeping (which windows are hidden right now).
    close: Rc<RefCell<CloseControl>>,
    /// Enqueues `activate_window`/`close_window` commands.
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
            close: Rc::new(RefCell::new(CloseControl::new())),
            input,
        }
    }

    /// The widget to place as the window's bottom bar.
    pub(crate) fn widget(&self) -> gtk::ScrolledWindow {
        self.scrolled.clone()
    }

    /// Rebuilds the rows from `state`, but only when something changed.
    ///
    /// This is the refresh-on-state path: it avoids rebuilding the buttons on
    /// every (frequently refreshed) state message that carries the same windows,
    /// and it reconciles the optimistic close set against the runtime's truth —
    /// a window the runtime still reports is revealed again.
    pub(crate) fn update_state(&self, state: &DesktopState) {
        let reconciled = self.close.borrow_mut().reconcile(state);
        let entries = taskbar::entries(state);
        let changed = entries != *self.model.borrow();
        if changed {
            *self.model.borrow_mut() = entries;
        }
        if changed || reconciled {
            self.rebuild();
        }
    }

    /// Updates the active highlight from a frame's active window, without a full
    /// rebuild.
    pub(crate) fn set_active(&self, active: Option<WindowId>) {
        taskbar::set_active(&mut self.model.borrow_mut(), active);
        let entries = self.model.borrow();
        let close = self.close.borrow();
        apply_highlight(&self.buttons.borrow(), &visible(&entries, &close));
    }

    /// Reveals a window again after its close failed, so the runtime's truth
    /// wins over the optimistic guess.
    pub(crate) fn restore(&self, window_id: WindowId) {
        // The borrow ends with the statement; `rebuild` re-borrows below.
        let restored = self.close.borrow_mut().restore(window_id);
        if restored {
            self.rebuild();
        }
    }

    /// Replaces the row's children with one group per visible entry.
    fn rebuild(&self) {
        rebuild(
            &self.row,
            &self.model,
            &self.buttons,
            &self.close,
            &self.input,
        );
    }
}

/// Rebuilds `row` from the model, the close control and the input handle.
///
/// A free function (rather than a method) so the per-button signal closures can
/// call it without capturing the [`TaskBarView`] itself.
fn rebuild(
    row: &gtk::Box,
    model: &Rc<RefCell<Vec<TaskBarEntry>>>,
    buttons: &Rc<RefCell<Vec<gtk::ToggleButton>>>,
    close: &Rc<RefCell<CloseControl>>,
    input: &InputHandle,
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    let entries = model.borrow();
    let control = close.borrow();
    let shown = visible(&entries, &control);
    let mut new_buttons = Vec::with_capacity(shown.len());

    for entry in &shown {
        let window_id = entry.window_id;

        // The switch half: runtime-native `activate_window`.
        let switch = gtk::ToggleButton::new();
        switch.set_label(&entry.label);
        switch.set_active(entry.active);
        switch.set_tooltip_text(Some(&format!("Show “{}”", entry.label)));
        {
            let input = input.clone();
            let model = model.clone();
            let buttons = buttons.clone();
            let close = close.clone();
            switch.connect_clicked(move |_| {
                // Runtime-native switch, then optimistically highlight so the UI
                // reacts before the next state arrives.
                input.send(InputCommand::ActivateWindow(window_id));
                taskbar::set_active(&mut model.borrow_mut(), Some(window_id));
                let entries = model.borrow();
                let control = close.borrow();
                apply_highlight(&buttons.borrow(), &visible(&entries, &control));
            });
        }

        // The close half: runtime-native `close_window`, hidden optimistically
        // (see `window_close`).
        let close_button = gtk::Button::from_icon_name("window-close-symbolic");
        close_button.add_css_class("flat");
        close_button.set_tooltip_text(Some(&format!("Close “{}”", entry.label)));
        {
            let input = input.clone();
            let close = close.clone();
            let row = row.clone();
            let model = model.clone();
            let buttons = buttons.clone();
            close_button.connect_clicked(move |_| {
                // The borrow ends with the statement; `rebuild` re-borrows below.
                let requested = close.borrow_mut().request(window_id);
                // A second click on an already-hidden row sends nothing.
                if !requested {
                    return;
                }
                input.send(InputCommand::CloseWindow(window_id));
                rebuild(&row, &model, &buttons, &close, &input);
            });
        }

        let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        group.append(&switch);
        group.append(&close_button);
        row.append(&group);
        new_buttons.push(switch);
    }

    *buttons.borrow_mut() = new_buttons;
}

/// The entries not hidden by an in-flight close, in model order.
///
/// The rebuilt buttons line up with this slice, so [`apply_highlight`] can zip
/// the two.
fn visible<'a>(entries: &'a [TaskBarEntry], close: &CloseControl) -> Vec<&'a TaskBarEntry> {
    entries
        .iter()
        .filter(|entry| !close.is_hidden(entry.window_id))
        .collect()
}

/// Applies each entry's `active` flag to its switch button.
fn apply_highlight(buttons: &[gtk::ToggleButton], entries: &[&TaskBarEntry]) {
    for (button, entry) in buttons.iter().zip(entries) {
        button.set_active(entry.active);
    }
}
