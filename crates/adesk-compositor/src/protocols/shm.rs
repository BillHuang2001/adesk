//! `wl_shm` handling plus the shared buffer-destruction hook.
//!
//! `delegate_compositor!` does not dispatch `wl_buffer`; that object is owned by
//! the buffer-owning protocols. Smithay therefore requires a [`BufferHandler`]
//! impl, which `delegate_shm!` and `delegate_dmabuf!` both rely on.
//!
//! # Dropping renderer state for a destroyed buffer
//!
//! Smithay 0.7 keeps no per-buffer renderer state a compositor could drop by hand:
//! the buffer is referenced from *surface* state (`RendererSurfaceState`, whose
//! textures are cleared on the next commit and by the destruction hook that
//! `on_commit_buffer_handler` installs), while each renderer caches the imported
//! texture keyed by the weak dmabuf/buffer and frees it as soon as the client's
//! buffer is gone. The documented hook for forcing that sweep is
//! [`Renderer::cleanup_texture_cache`], so `buffer_destroyed` calls it on the
//! concrete backend and lets it release whatever belonged to the destroyed buffer.

use smithay::{
    backend::renderer::Renderer,
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::{
        buffer::BufferHandler,
        shm::{ShmHandler, ShmState},
    },
};

use crate::{render::HeadlessRenderer, state::State};

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {
        // Both backends implement `Renderer::cleanup_texture_cache`; it drops the
        // cache entries whose client buffer/dmabuf is gone.
        match &mut self.renderer {
            HeadlessRenderer::Gl(renderer) => sweep(renderer.cleanup_texture_cache()),
            HeadlessRenderer::Pixman(renderer) => sweep(renderer.cleanup_texture_cache()),
        }
    }
}

/// Log a failed texture-cache sweep.
///
/// A backend that cannot clean its cache leaks resources but must not fail the
/// client request that triggered the destruction.
fn sweep<E: std::fmt::Display>(result: Result<(), E>) {
    if let Err(error) = result {
        tracing::debug!(%error, "renderer texture cache cleanup failed");
    }
}

smithay::delegate_shm!(State);
