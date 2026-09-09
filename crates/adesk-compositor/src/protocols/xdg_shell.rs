//! `xdg-shell` handling: toplevels, popups and window metadata.
//!
//! Lifecycle and metadata notifications are forwarded to the matching `State`
//! hook; the ones that need to send configures or track popup grabs are Phase 2
//! work and are left unimplemented here.

use smithay::{
    reexports::wayland_server::protocol::wl_seat,
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

    fn new_toplevel(&mut self, _surface: ToplevelSurface) {
        todo!("Phase 2: register the toplevel with WmBridge and send the initial tiling configure")
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.on_popup_created(&surface);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        todo!("Phase 2: track the popup grab so pointer-leave can send popup_done")
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        todo!("Phase 2: recompute the popup position and confirm it with send_repositioned")
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
