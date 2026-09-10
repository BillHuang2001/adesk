//! Policy decisions returned by [`crate::WindowManager`] for the compositor to apply.

use adesk_core::{Rect, WindowId};

/// A decision the window policy reached, to be applied by the compositor.
///
/// Policy methods never touch Smithay objects: they return the actions the
/// compositor must apply, in the order given. Applying an action is idempotent
/// for the compositor (re-configuring a window to its current rect, or
/// focusing the already-focused window, changes nothing visible).
///
/// Actions carry no timestamps, no sequence numbers and no event data: the
/// compositor assigns `seq`/`ts_ms` when it turns an action into a
/// [`adesk_core::RuntimeEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmAction {
    /// Configure `id` to `rect`.
    ///
    /// The compositor sends an `xdg_toplevel.configure` with `rect.size()` and
    /// places the window's surface tree at `rect`'s origin in output
    /// coordinates, so window-relative coordinates stay relative to that
    /// origin. Emitted on map and whenever the output size changes.
    ConfigureWindow {
        /// Window to configure.
        id: WindowId,
        /// Output-space rect the window must occupy.
        rect: Rect,
    },
    /// Make `id` the active (visible, keyboard-focused) window.
    ///
    /// Emitted by [`crate::WindowManager::activate`] and for the automatic
    /// focus on map. The previously active window becomes
    /// [`adesk_core::WindowState::Inactive`] but stays mapped. The compositor
    /// moves keyboard focus and emits `WindowActivated` / `FocusChanged`.
    Activate {
        /// Window that becomes active.
        id: WindowId,
    },
    /// Make `id` the active window because the previously active window was
    /// destroyed (most-recently-used fallback, `docs/architecture.md` §4).
    ///
    /// Semantically identical to [`WmAction::Activate`] for the compositor;
    /// the variant exists so the causal reason is visible in logs and events.
    ActivatePrevious {
        /// Window that becomes active.
        id: WindowId,
    },
    /// Explicit no-op: the policy evaluated the request and decided that
    /// nothing changes — for example activating the already-active window, or
    /// mapping a surface key that is already tracked.
    ///
    /// An *empty* action list means the request had no effect at all (unknown
    /// window id, duplicate event); `None` means the decision was evaluated
    /// and is deliberately "do nothing", which lets the compositor skip
    /// emitting an event.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::Rect;

    #[test]
    fn actions_are_comparable_and_copyable() {
        let configure = WmAction::ConfigureWindow {
            id: WindowId(1),
            rect: Rect::new(0, 0, 1280, 800),
        };
        let copy = configure;
        assert_eq!(configure, copy);
        assert_ne!(WmAction::None, WmAction::Activate { id: WindowId(1) });
        assert_ne!(
            WmAction::Activate { id: WindowId(1) },
            WmAction::ActivatePrevious { id: WindowId(1) }
        );
    }
}
