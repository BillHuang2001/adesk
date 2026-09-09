//! The render pass itself: bind target, draw the scene, read back, post-process.
//!
//! This module is generic over Smithay's renderer traits; the concrete renderer
//! (`GlesRenderer` or `PixmanRenderer`) is created and owned by
//! `adesk-compositor` and passed in by mutable reference.

use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::{Bind, ExportMem, ImportAll, Renderer};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::utils::{Buffer as BufferCoords, Rectangle};
use smithay::wayland::compositor::SurfaceData;

use crate::config::RenderConfig;
use crate::error::Result;
use crate::output::{OffscreenTarget, RenderedFrame};
use crate::scene::Scene;

/// Renders `scene` into `target` and reads the result back as an
/// [`adesk_core::ImageBuffer`].
///
/// Steps (see `CONTEXT.md` for the exact Smithay calls):
///
/// 1. `config.validate()`;
/// 2. `renderer.bind(target.texture_mut())` → framebuffer;
/// 3. `renderer.render(&mut framebuffer, target_size, Transform::Normal)` → frame;
/// 4. `frame.clear(config.clear_color, &[target_rect])`;
/// 5. draw nodes bottom-to-top (painter's algorithm, `scene.nodes()` order):
///    `dst = (node.location - source.loc)` with the size from
///    `element.geometry(scale)`, `src = element.src()`, per-node damage and
///    opaque regions translated into element-local coordinates and clipped to
///    the target; empty node damage means full redraw;
/// 6. `frame.finish()`;
/// 7. `renderer.copy_framebuffer(&framebuffer, target_rect, READBACK_FORMAT)` →
///    mapping, `renderer.map_texture(&mapping)` → bytes,
///    [`crate::image_from_readback`] (honours `TextureMapping::flipped`, which is
///    `true` for GL and `false` for pixman);
/// 8. `crop` then `downscale` per [`RenderConfig`];
/// 9. `RenderedFrame { image, commit_seq: scene.commit_seq(), damage }` where
///    `damage` is `scene.damage()` clipped to `config.source`.
///
/// Any renderer error is boxed into a structured [`crate::RenderError`]; this
/// function never panics on a renderer failure.
pub fn render_scene<R, T, E>(
    renderer: &mut R,
    target: &mut OffscreenTarget<T>,
    scene: &Scene<E>,
    config: &RenderConfig,
) -> Result<RenderedFrame>
where
    R: Renderer + Bind<T> + ExportMem,
    R::Error: Send + Sync + 'static,
    E: RenderElement<R>,
{
    let _ = (renderer, target, scene, config);
    todo!("render_scene: bind, draw elements back-to-front, readback, crop/downscale")
}

/// Imports a wayland buffer (SHM, DMA-BUF or EGL) as a renderer texture.
///
/// `adesk-compositor` uses this while building a scene; the structured error is
/// how an unsupported DMA-BUF format on the software path surfaces as AGP
/// `render_failed` instead of a panic. `Smithay`'s `ImportAll::import_buffer`
/// returns `None` for buffers without a texture representation (unknown buffer
/// type, single-pixel buffers), which maps to
/// [`crate::RenderError::UnsupportedBuffer`].
pub fn import_buffer<R: ImportAll>(
    renderer: &mut R,
    buffer: &WlBuffer,
    surface: Option<&SurfaceData>,
    damage: &[Rectangle<i32, BufferCoords>],
) -> Result<R::TextureId>
where
    R::Error: Send + Sync + 'static,
{
    let _ = (renderer, buffer, surface, damage);
    todo!("import_buffer: renderer.import_buffer(...) -> TextureId, mapping None/Err to RenderError")
}
