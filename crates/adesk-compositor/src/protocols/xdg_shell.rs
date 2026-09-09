//! `xdg-shell` handling: toplevels, popups and window metadata.
//!
//! Lifecycle and metadata notifications are forwarded to the matching `State` hook.
//! The three stateful paths live here:
//!
//! - `new_toplevel` registers the surface with [`WmBridge`](crate::wm::WmBridge) and
//!   sends the initial tiling configure (`Activated`, output size) immediately: a
//!   client cannot commit before it has acked a configure, and the map trigger is the
//!   first buffer commit.
//! - `grab` records the popup grab. v1 semantics: a grab is *bookkeeping only* — the
//!   headless runtime has no physical pointer, so there is no pointer-leave that could
//!   dismiss a popup. `State` sends `popup_done` when a runtime-native operation
//!   (`activate_window` elsewhere, owner destroy) invalidates the grab.
//! - `reposition_request` recomputes the popup geometry from the new positioner and
//!   confirms it with `xdg_popup.repositioned(token)`.

use smithay::{
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_popup,
        wayland_server::{protocol::wl_seat, Resource},
    },
    utils::Serial,
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};

use crate::state::State;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.on_toplevel_registered(&surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.on_popup_created(&surface);
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Recorded, never enforced by the seat (see the module docs).
        self.wm.note_popup_grab(&surface);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        // Precondition of a successful re-configure, mirroring the guard inside
        // `PopupSurface::send_configure`: once the initial configure was sent, the
        // client must support `xdg_popup.repositioned` (v3+) and the positioner must
        // be reactive. We cannot call `send_configure` here because it would send a
        // configure *without* the token that `send_repositioned` must carry.
        let can_reconfigure = !surface.is_initial_configure_sent()
            || (surface.xdg_popup().version() >= xdg_popup::EVT_REPOSITIONED_SINCE
                && positioner.reactive);
        if !can_reconfigure {
            // The popup cannot follow its parent: dismiss it rather than leave it at a
            // stale position. `PopupConfigureError` is never unwrapped anywhere.
            tracing::debug!(
                "popup cannot be repositioned (version < 3 or non-reactive positioner), dismissing"
            );
            surface.send_popup_done();
            self.on_popup_destroyed(&surface);
            return;
        }
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = positioner.get_geometry();
        });
        surface.send_repositioned(token);
        tracing::debug!(token, "popup repositioned");
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.on_toplevel_destroyed(&surface);
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.on_popup_destroyed(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.on_title_changed(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.on_app_id_changed(&surface);
    }
}

smithay::delegate_xdg_shell!(State);
