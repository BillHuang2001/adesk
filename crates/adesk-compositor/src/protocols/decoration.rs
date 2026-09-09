//! `xdg-decoration` handling: server-side decorations only.
//!
//! ADesk tiles every window to fill the output and draws no chrome itself, so the
//! compositor always answers `Mode::ServerSide` and never delegates decoration to the
//! client: clients stop drawing their own title bars and the whole output stays
//! available to the window. The mode lives in the toplevel's pending state and is
//! published with [`ToplevelSurface::send_pending_configure`], which sends a configure
//! only when the state actually changed — a client that already asked for server-side
//! decoration never sees a spurious re-configure.

use smithay::{
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::state::State;

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        force_server_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        if mode != Mode::ServerSide {
            tracing::debug!(
                ?mode,
                "overriding the requested decoration mode with server-side"
            );
        }
        force_server_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        force_server_side(&toplevel);
    }
}

/// Force server-side decoration on `toplevel` and publish the change.
///
/// A toplevel whose surface is already gone is skipped: Smithay's pending-state
/// helpers require a live surface and there is nothing left to configure.
fn force_server_side(toplevel: &ToplevelSurface) {
    if !toplevel.alive() {
        tracing::debug!("decoration mode requested for a dead toplevel, ignoring");
        return;
    }
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(Mode::ServerSide);
    });
    // Sends a configure only if the pending state differs from the current one.
    toplevel.send_pending_configure();
}

smithay::delegate_xdg_decoration!(State);
