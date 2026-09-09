//! `zwp_linux_dmabuf_v1` handling (DMA-BUF client buffers).
//!
//! The global is created in [`State::new`](crate::state::State::new) from the
//! renderer's supported formats; the [`DmabufHandler`] here owns the per-buffer
//! import path into the renderer.
//!
//! Import happens synchronously on the compositor thread through the concrete
//! renderer — both backends implement
//! [`ImportDma`](smithay::backend::renderer::ImportDma): `GlesRenderer` through an
//! EGL image, `PixmanRenderer` by mapping the dmabuf. The renderer keeps its own
//! reference to the imported texture (keyed by the weak dmabuf, dropped when the
//! client's buffer is gone), so the handle returned here is only used to learn
//! whether the import succeeded.
//!
//! Failures are reported to the client with [`ImportNotifier::failed`] and logged:
//! an import failure is a client error the protocol handles, never a panic and
//! never a reason to kill the connection.

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::ImportDma},
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::{render::HeadlessRenderer, state::State};

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // The two backends return different texture/error types, so each arm
        // reports its own outcome through the shared helper.
        match &mut self.renderer {
            HeadlessRenderer::Gl(renderer) => {
                notify(renderer.import_dmabuf(&dmabuf, None), notifier)
            }
            HeadlessRenderer::Pixman(renderer) => {
                notify(renderer.import_dmabuf(&dmabuf, None), notifier)
            }
        }
    }
}

/// Report a renderer import outcome to the client.
///
/// Generic over the backend's texture and error type so both renderer paths share
/// one notification path: on success the client gets its `wl_buffer`, on failure
/// `failed()` (an implementation-dependent import failure, not a protocol error).
fn notify<T, E: std::fmt::Display>(imported: Result<T, E>, notifier: ImportNotifier) {
    match imported {
        Ok(_texture) => {
            // The renderer holds its own reference to the texture; the client may
            // still have vanished between import and notification.
            if let Err(error) = notifier.successful::<State>() {
                tracing::debug!(%error, "dmabuf imported for a client that is already gone");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "dmabuf import failed");
            notifier.failed();
        }
    }
}

smithay::delegate_dmabuf!(State);
