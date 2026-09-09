//! The render pass itself: bind target, draw the scene, read back, post-process.
//!
//! This module is generic over Smithay's renderer traits; the concrete renderer
//! (`GlesRenderer` or `PixmanRenderer`) is created and owned by
//! `adesk-compositor` and passed in by mutable reference.

use adesk_core::Rect;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, ImportAll, Renderer, TextureMapping,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{
    Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform,
};
use smithay::wayland::compositor::SurfaceData;

use crate::config::{RenderConfig, READBACK_FORMAT};
use crate::error::{RenderError, Result};
use crate::image::{crop, downscale, image_from_readback};
use crate::output::{OffscreenTarget, RenderedFrame};
use crate::scene::Scene;

/// Renders `scene` into `target` and reads the result back as an
/// [`adesk_core::ImageBuffer`].
///
/// Steps:
///
/// 1. `config.validate()`, then check that the target is exactly
///    `config.target_size()` (the readback/crop math assumes target size ==
///    source size; a mismatch is [`RenderError::InvalidConfig`]);
/// 2. `renderer.bind(target.texture_mut())` → framebuffer;
/// 3. `renderer.render(&mut framebuffer, target_size, Transform::Normal)` → frame;
/// 4. `frame.clear(config.clear_color, &[target_rect])`;
/// 5. draw nodes bottom-to-top (painter's algorithm, `scene.nodes()` order):
///    `dst = (node.location - source.loc)` with the size from
///    `element.geometry(scale)`, `src = element.src()`, opaque regions
///    translated into element-local coordinates. Every node that intersects the
///    target is drawn over its **full visible rectangle**: the target is cleared
///    first, so clipping the draw to `SceneNode::damage` would punch
///    clear-color holes into a complete frame. `SceneNode::damage` is *evidence*
///    and is reported in [`RenderedFrame::damage`], not used to clip drawing;
/// 6. `frame.finish()` and wait for the returned sync point;
/// 7. `renderer.copy_framebuffer(&framebuffer, target_rect, READBACK_FORMAT)` →
///    mapping, `renderer.map_texture(&mapping)` → bytes,
///    [`crate::image_from_readback`] (honours `TextureMapping::flipped`, which is
///    `true` for GL and `false` for pixman);
/// 8. `crop` then `downscale` per [`RenderConfig`] (crop is given in scene
///    coordinates and is translated to target coordinates here);
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
    config.validate()?;

    let target_size = config.target_size();
    let allocated = target.core_size();
    if allocated != target_size {
        return Err(RenderError::InvalidConfig {
            reason: format!(
                "render target is {allocated:?} but the configuration renders {target_size:?}; \
                 readback and crop math assume target size == source size"
            ),
        });
    }
    let buffer_size = target.size();
    let target_physical = Size::<i32, Physical>::from((buffer_size.w, buffer_size.h));
    let target_rect = Rectangle::<i32, Physical>::from_size(target_physical);
    let scale = Scale::from(1.0);

    let mut framebuffer = renderer
        .bind(target.texture_mut())
        .map_err(|err| RenderError::TargetBind {
            size: target_size,
            source: Box::new(err),
        })?;

    let mut frame = renderer
        .render(&mut framebuffer, target_physical, Transform::Normal)
        .map_err(|err| RenderError::RenderFailed {
            source: Box::new(err),
        })?;

    // `Color32F` is premultiplied RGBA, but `clear` does not blend, so the bytes
    // round-trip unchanged through the readback.
    let clear_color = Color32F::from(config.clear_color.map(|channel| channel as f32 / 255.0));
    frame
        .clear(clear_color, &[target_rect])
        .map_err(|err| RenderError::RenderFailed {
            source: Box::new(err),
        })?;

    for node in scene.nodes() {
        let geometry = node.element().geometry(scale);
        let dst = Rectangle::new(to_target(node.location(), config.source), geometry.size);
        // Nodes fully outside the rendered source region contribute nothing.
        let Some(visible) = dst.intersection(target_rect) else {
            continue;
        };
        let damage = [Rectangle::new(visible.loc - dst.loc, visible.size)];
        let opaque: Vec<Rectangle<i32, Physical>> = node
            .element()
            .opaque_regions(scale)
            .iter()
            .map(|region| {
                let mut region = *region;
                region.loc -= dst.loc;
                region
            })
            .collect();
        node.element()
            .draw(&mut frame, node.element().src(), dst, &damage, &opaque)
            .map_err(|err| RenderError::RenderFailed {
                source: Box::new(err),
            })?;
    }

    let sync = frame.finish().map_err(|err| RenderError::RenderFailed {
        source: Box::new(err),
    })?;
    renderer.wait(&sync).map_err(|err| RenderError::RenderFailed {
        source: Box::new(err),
    })?;

    let readback_region = Rect::from_size(target_size);
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(buffer_size),
            READBACK_FORMAT,
        )
        .map_err(|err| RenderError::Readback {
            region: readback_region,
            source: Box::new(err),
        })?;
    let flipped = mapping.flipped();
    let bytes = renderer
        .map_texture(&mapping)
        .map_err(|err| RenderError::Readback {
            region: readback_region,
            source: Box::new(err),
        })?;
    // `Abgr8888` is four bytes per pixel, so a full-width row is `width * 4`.
    let image = image_from_readback(
        bytes,
        target_size.w,
        target_size.h,
        target_size.w.saturating_mul(4),
        flipped,
    )?;

    let image = match config.crop {
        Some(crop_rect) => {
            let origin = to_target(Point::<i32, Physical>::from((crop_rect.x, crop_rect.y)), config.source);
            crop(&image, Rect::new(origin.x, origin.y, crop_rect.w, crop_rect.h))
        }
        None => image,
    };
    let image = match config.max_dimension {
        Some(max_dimension) => downscale(&image, max_dimension),
        None => image,
    };

    Ok(RenderedFrame::new(
        image,
        scene.commit_seq(),
        scene.damage().clip(&config.source),
    ))
}

/// Maps a scene-coordinate point into target coordinates by subtracting the
/// origin of [`RenderConfig::source`] (scene `source.loc` is target pixel
/// `(0, 0)`).
///
/// Saturating arithmetic keeps a pathological configuration from panicking.
fn to_target(point: Point<i32, Physical>, source: Rect) -> Point<i32, Physical> {
    Point::from((
        point.x.saturating_sub(source.x),
        point.y.saturating_sub(source.y),
    ))
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
    match renderer.import_buffer(buffer, surface, damage) {
        Some(Ok(texture)) => Ok(texture),
        Some(Err(err)) => Err(RenderError::ImportFailed {
            reason: err.to_string(),
        }),
        None => Err(RenderError::UnsupportedBuffer {
            reason: format!(
                "wl_buffer {} has no texture representation (unknown buffer type or single-pixel buffer)",
                buffer.id().protocol_id()
            ),
        }),
    }
}
