//! `xdg-decoration` handling: server-side decorations only.
//!
//! ADesk tiles every window to fill the output and draws no chrome itself, so all
//! three notifications are Phase 2 work: the compositor must answer with
//! `Mode::ServerSide` (never delegate decoration to the client) so clients stop
//! drawing their own title bars.

use smithay::{
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::state::State;

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, _toplevel: ToplevelSurface) {
        todo!("Phase 2: answer the new decoration object with Mode::ServerSide")
    }

    fn request_mode(&mut self, _toplevel: ToplevelSurface, _mode: Mode) {
        todo!("Phase 2: accept or override the client's requested decoration mode")
    }

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {
        todo!("Phase 2: fall back to the default server-side decoration mode")
    }
}

smithay::delegate_xdg_decoration!(State);
