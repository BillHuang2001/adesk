//! `wl_shm` handling plus the shared buffer-destruction hook.
//!
//! `delegate_compositor!` does not dispatch `wl_buffer`; that object is owned by
//! the buffer-owning protocols. Smithay therefore requires a [`BufferHandler`]
//! impl, which `delegate_shm!` and `delegate_dmabuf!` both rely on.

use smithay::{
    reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    wayland::{
        buffer::BufferHandler,
        shm::{ShmHandler, ShmState},
    },
};

use crate::state::State;

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {
        todo!("Phase 2: drop cached renderer state for the destroyed wl_buffer")
    }
}

smithay::delegate_shm!(State);
