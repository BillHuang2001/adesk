//! The task bar's per-window close: the optimistic-prune decision and its
//! failure text (GTK-free).
//!
//! Closing a window is runtime-native on the wire — VAP's `close_window` names a
//! window and the runtime closes it, exactly like `activate_window`
//! (`docs/viewer.md` §4, §5) — but it races with the runtime's own window list:
//! the viewer only learns the window is gone from a `state` refresh, and a close
//! of a window that already vanished is answered with an `unknown_window` error
//! on the still-open connection.
//!
//! ## Decision: prune optimistically, self-heal on the state refresh
//!
//! A clicked close button hides its row **immediately** ([`CloseControl::request`])
//! instead of waiting up to one refresh tick (500 ms) for the runtime to drop the
//! window from its state: a deliberate close must feel like it did something, and
//! the runtime-native close is near-instant. The optimistic guess is then
//! reconciled against the truth every time a state arrives
//! ([`CloseControl::reconcile`]): a hidden window the runtime still reports is
//! revealed again, so a guess that turns out to be wrong (a close the runtime
//! refused, or one that has not taken effect yet) can never leave the task bar
//! permanently missing a live window. A close the worker reports as *failed* is
//! revealed at once as well.
//!
//! This module holds only that bookkeeping and the human text; the buttons live
//! in [`crate::task_bar_view`].

use std::collections::BTreeSet;

use adesk_core::WindowId;
use adesk_viewer_proto::DesktopState;

/// The optimistic-close bookkeeping behind the task bar's close buttons.
///
/// Holds the windows whose close was requested but whose disappearance the
/// runtime has not confirmed yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CloseControl {
    /// The window ids currently hidden from the task bar.
    hidden: BTreeSet<WindowId>,
}

impl CloseControl {
    /// Creates an empty control (nothing hidden).
    pub(crate) fn new() -> CloseControl {
        CloseControl::default()
    }

    /// Registers a close request for `id` and hides its row; reports whether this
    /// was a *new* request.
    ///
    /// A repeated request for an already-hidden window returns `false`, so the
    /// GTK layer sends no second `close_window` for it.
    pub(crate) fn request(&mut self, id: WindowId) -> bool {
        self.hidden.insert(id)
    }

    /// Whether `id` is currently hidden from the task bar.
    pub(crate) fn is_hidden(&self, id: WindowId) -> bool {
        self.hidden.contains(&id)
    }

    /// Reveals `id` again — the close failed, so the row must come back; reports
    /// whether anything changed.
    pub(crate) fn restore(&mut self, id: WindowId) -> bool {
        self.hidden.remove(&id)
    }

    /// Reconciles the hidden set with a freshly received state: every hidden
    /// window the state still reports is revealed again (the close failed or has
    /// not taken effect), so a wrong guess self-heals. Ids the state no longer
    /// reports stay hidden — they are gone, which is what was asked for.
    ///
    /// Reports whether anything changed, so the task bar can decide whether to
    /// rebuild.
    pub(crate) fn reconcile(&mut self, state: &DesktopState) -> bool {
        let present: BTreeSet<WindowId> = state.windows.iter().map(|window| window.id).collect();
        let before = self.hidden.len();
        self.hidden.retain(|id| !present.contains(id));
        before != self.hidden.len()
    }
}

/// The human message for a close the runtime refused, naming the window.
pub(crate) fn failure_message(id: WindowId, reason: &str) -> String {
    format!("could not close window {id}: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_core::{AppId, Rect, WindowInfo, WindowState};

    /// A window fixture with the given id.
    fn window(id: u64) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: Some(AppId::from("org.example.App")),
            title: Some(format!("window {id}")),
            geometry: Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            state: WindowState::Inactive,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq: 2,
            popup_count: 0,
        }
    }

    /// A state listing the given window ids.
    fn state(ids: &[u64]) -> DesktopState {
        DesktopState {
            active_window_id: ids.first().copied().map(WindowId),
            windows: ids.iter().copied().map(window).collect(),
        }
    }

    #[test]
    fn a_new_control_hides_nothing() {
        let control = CloseControl::new();
        assert!(!control.is_hidden(WindowId(1)));
        assert!(!control.is_hidden(WindowId(2)));
    }

    #[test]
    fn requesting_a_close_hides_the_window_once() {
        let mut control = CloseControl::new();
        assert!(control.request(WindowId(3)));
        assert!(control.is_hidden(WindowId(3)));
        assert!(!control.is_hidden(WindowId(4)));
        // A second click on the same (already hidden) row sends nothing.
        assert!(!control.request(WindowId(3)));
    }

    #[test]
    fn restoring_a_failed_close_reveals_the_row() {
        let mut control = CloseControl::new();
        control.request(WindowId(1));
        assert!(control.restore(WindowId(1)));
        assert!(!control.is_hidden(WindowId(1)));
        // Restoring an id that was never hidden is a no-op.
        assert!(!control.restore(WindowId(1)));
    }

    #[test]
    fn reconcile_keeps_a_gone_window_hidden() {
        let mut control = CloseControl::new();
        control.request(WindowId(1));
        control.request(WindowId(2));

        // Window 1 is gone (the close worked); window 2 is still there.
        assert!(control.reconcile(&state(&[2])));
        assert!(control.is_hidden(WindowId(1)));
        assert!(!control.is_hidden(WindowId(2)));
    }

    #[test]
    fn reconcile_leaves_a_confirmed_state_alone() {
        let mut control = CloseControl::new();
        control.request(WindowId(1));
        // The window is gone: nothing left to reconcile, so the task bar is not
        // rebuilt for nothing.
        assert!(!control.reconcile(&state(&[])));
        assert!(control.is_hidden(WindowId(1)));
    }

    #[test]
    fn reconcile_with_nothing_hidden_changes_nothing() {
        let mut control = CloseControl::new();
        assert!(!control.reconcile(&state(&[1, 2])));
        assert!(!control.is_hidden(WindowId(1)));
        assert!(!control.is_hidden(WindowId(2)));
    }

    #[test]
    fn a_window_that_reappears_is_revealed_again() {
        // The close failed (or has not taken effect), so the runtime keeps
        // reporting the window: the row must come back rather than stay lost.
        let mut control = CloseControl::new();
        control.request(WindowId(5));
        assert!(control.reconcile(&state(&[5])));
        assert!(!control.is_hidden(WindowId(5)));
    }

    #[test]
    fn the_failure_message_names_the_window() {
        assert_eq!(
            failure_message(WindowId(7), "backend error: unknown window 7"),
            "could not close window 7: backend error: unknown window 7"
        );
    }
}
