//! `zwp_linux_dmabuf_v1` handling (DMA-BUF client buffers).
//!
//! The global is created in [`State::new`](crate::state::State::new) from the
//! renderer's supported formats; the [`DmabufHandler`] here owns the per-buffer
//! import path into the renderer.

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::state::State;

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        _dmabuf: Dmabuf,
        _notifier: ImportNotifier,
    ) {
        todo!("Phase 2: import the dmabuf into the renderer and notify the client of the result")
    }
}

smithay::delegate_dmabuf!(State);
