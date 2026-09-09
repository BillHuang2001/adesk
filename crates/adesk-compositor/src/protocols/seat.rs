//! `wl_seat` handling: one seat with a keyboard and a pointer.
//!
//! Focus targets are bare `wl_surface`s. `adesk-wm` maps surfaces to windows, so
//! the seat stays window-agnostic; coordinates and focus policy never live here.

use smithay::{
    input::{SeatHandler, SeatState},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
};

use crate::state::State;

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }
}

smithay::delegate_seat!(State);
