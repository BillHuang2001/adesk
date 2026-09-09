//! `xdg-shell` handling: toplevels, popups and window metadata.
//!
//! Lifecycle and metadata notifications are forwarded to the matching `State` hook.
//! The three stateful paths live here:
//!
//! - `new_toplevel` registers the surface with [`WmBridge`](crate::wm::WmBridge) and
//!   sends the initial tiling configure (`Activated`, output size) immediately: a
//!   client cannot commit before it has acked a configure, and the map trigger is the
//!   first buffer commit.
//! - `new_popup` sends the initial `xdg_popup.configure` from the positioner geometry and
//!   *then* registers the popup: a popup may not commit a buffer before the compositor
//!   has configured it, and the geometry recorded as the popup's window-relative origin
//!   has to be the one the configure confirmed.
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
    utils::{Logical, Point, Rectangle, Serial, Size},
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

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // The protocol forbids the client from committing a popup buffer before the
        // compositor configured the popup, so the initial configure is sent as soon as
        // the popup exists. The geometry is finalised *before* the popup is registered,
        // because `on_popup_created` records the popup's window-relative origin from the
        // pending geometry — configure and recorded origin must not disagree.
        let geometry = initial_popup_geometry(&positioner);
        surface.with_pending_state(|state| state.geometry = geometry);
        match surface.send_configure() {
            Ok(serial) => tracing::debug!(
                x = geometry.loc.x,
                y = geometry.loc.y,
                width = geometry.size.w,
                height = geometry.size.h,
                ?serial,
                "initial popup configure sent"
            ),
            // A fresh popup has no configure yet, so this is unreachable in practice;
            // `PopupConfigureError` is never unwrapped anywhere (see `reposition_request`).
            Err(error) => tracing::debug!(%error, "initial popup configure rejected"),
        }
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

/// The rectangle the initial `xdg_popup.configure` confirms.
///
/// `PositionerState::get_geometry` applies the client's anchor rect, anchor, gravity and
/// offset to the size set with `xdg_positioner.set_size`; that rectangle is the placement
/// the compositor accepts. v1 has no popup constraint policy (popups are placed relative
/// to their parent and never constrained against the output), so no further adjustment is
/// applied. A positioner that yields no usable size falls back to the unconstrained
/// `(0, 0)` origin and a zero size, which lets the popup choose its own size.
fn initial_popup_geometry(positioner: &PositionerState) -> Rectangle<i32, Logical> {
    let geometry = positioner.get_geometry();
    if geometry.size.w > 0 && geometry.size.h > 0 {
        geometry
    } else {
        Rectangle::new(Point::from((0, 0)), Size::from((0, 0)))
    }
}

smithay::delegate_xdg_shell!(State);

#[cfg(test)]
mod tests {
    use smithay::{
        reexports::wayland_protocols::xdg::shell::server::xdg_positioner,
        utils::{Point, Rectangle, Size},
        wayland::shell::xdg::PositionerState,
    };

    use super::initial_popup_geometry;

    /// The positioner the Wayland test client uses: 120x80 at offset (16, 24) inside a
    /// 320x240 anchor rect, anchored top-left with bottom-right gravity.
    fn testkit_positioner() -> PositionerState {
        PositionerState {
            rect_size: Size::from((120, 80)),
            anchor_rect: Rectangle::new(Point::from((0, 0)), Size::from((320, 240))),
            anchor_edges: xdg_positioner::Anchor::TopLeft,
            gravity: xdg_positioner::Gravity::BottomRight,
            offset: (16, 24).into(),
            ..PositionerState::default()
        }
    }

    #[test]
    fn initial_configure_uses_the_positioner_geometry() {
        let geometry = initial_popup_geometry(&testkit_positioner());
        assert_eq!(
            geometry,
            Rectangle::new(Point::from((16, 24)), Size::from((120, 80)))
        );
    }

    #[test]
    fn initial_configure_is_unconstrained_without_a_positioner_size() {
        // No `set_size`: the popup picks its own size, so the compositor confirms 0x0.
        let geometry = initial_popup_geometry(&PositionerState::default());
        assert_eq!(
            geometry,
            Rectangle::new(Point::from((0, 0)), Size::from((0, 0)))
        );
    }
}
