//! `wl_compositor` / `wl_subcompositor` handling.
//!
//! Besides the compositor state getter, Smithay requires per-client state: the
//! transaction queue and scale override must live in the client's
//! [`ClientData`] so they
//! are dropped when that client disconnects. [`ClientState`] is that storage;
//! `crate::run` inserts it for every new client.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::wayland_server::{backend::ClientData, protocol::wl_surface::WlSurface, Client},
    wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState},
};

use crate::state::State;

/// Per-client Smithay state, attached as `ClientData` to every connected client.
///
/// Deriving `Default` is valid because [`CompositorClientState`] is `Default`: a
/// fresh, empty transaction queue per client is exactly what Smithay expects.
#[derive(Debug, Default)]
pub(crate) struct ClientState {
    /// The compositor state Smithay associates with this client.
    pub(crate) compositor_state: CompositorClientState,
}

impl ClientData for ClientState {}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client is created with a ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Smithay must consume the surface's pending buffer first: only then is the
        // commit observable as runtime state, and only then can the surface be
        // rendered. Damage reporting happens after that.
        on_commit_buffer_handler::<State>(surface);
        // Smithay's popup tree ingests a popup that had no parent yet only once that
        // popup commits, and the render path reads the tree: keep the manager current
        // for every surface commit.
        self.popup_manager.commit(surface);
        self.on_surface_commit(surface);
    }
}

smithay::delegate_compositor!(State);
