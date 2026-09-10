//! The render pass itself: bind target, draw the scene, read back, post-process.
//!
//! This module is generic over Smithay's renderer traits; the concrete renderer
//! (`GlesRenderer` or `PixmanRenderer`) is created and owned by
//! `adesk-compositor` and passed in by mutable reference.

use adesk_core::Rect;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Renderer, Texture};
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::config::{RenderConfig, READBACK_FORMAT};
use crate::error::{RenderError, Result};
use crate::image::{downscale, image_from_readback};
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
/// 7. `renderer.copy_framebuffer(&framebuffer, region, READBACK_FORMAT)` →
///    mapping, `renderer.map_texture(&mapping)` → bytes. When `config.crop` is
///    set, `region` is the crop translated into target coordinates
///    (`crop.loc - source.loc`), so only the requested sub-rectangle is read
///    back instead of the whole frame (`crop == None` reads the whole target,
///    exactly as before). The row width/height/stride are taken from the actual
///    mapping result and never assumed packed: pixman hands back a freshly
///    copied region-sized image, GL a fresh region-sized pixel buffer, but a
///    future backend could return a strided view;
///    [`crate::image_from_readback`] consumes the raw rows as top-down scene
///    order on every backend (see the call site for why
///    `TextureMapping::flipped` is deliberately ignored);
/// 8. `downscale` per [`RenderConfig::max_dimension`] (a `None` dimension is a
///    no-op and does not copy the image);
/// 9. `RenderedFrame { image, commit_seq: scene.commit_seq(), damage }` where
///    `damage` is `scene.damage()` clipped to `config.source`, in scene
///    coordinates and never adjusted by crop/downscale.
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

    let mut framebuffer =
        renderer
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

    // Scratch buffers hoisted out of the draw loop: reused (cleared) per node
    // instead of being reallocated for every element.
    let mut damage = [Rectangle::<i32, Physical>::from_size(Size::from((0, 0)))];
    let mut opaque: Vec<Rectangle<i32, Physical>> = Vec::new();
    for node in scene.nodes() {
        let geometry = node.element().geometry(scale);
        let dst = Rectangle::new(to_target(node.location(), config.source), geometry.size);
        // Nodes fully outside the rendered source region contribute nothing.
        let Some(visible) = dst.intersection(target_rect) else {
            continue;
        };
        damage[0] = Rectangle::new(visible.loc - dst.loc, visible.size);
        opaque.clear();
        opaque.extend(node.element().opaque_regions(scale).iter().map(|region| {
            let mut region = *region;
            region.loc -= dst.loc;
            region
        }));
        node.element()
            .draw(&mut frame, node.element().src(), dst, &damage, &opaque)
            .map_err(|err| RenderError::RenderFailed {
                source: Box::new(err),
            })?;
    }

    let sync = frame.finish().map_err(|err| RenderError::RenderFailed {
        source: Box::new(err),
    })?;
    renderer
        .wait(&sync)
        .map_err(|err| RenderError::RenderFailed {
            source: Box::new(err),
        })?;

    // Read back only the requested sub-rectangle when a crop is configured:
    // `copy_framebuffer` accepts a sub-region, so a small crop no longer pays a
    // full-frame readback plus a full-image copy. The crop is given in scene
    // coordinates and translated into target/buffer coordinates here (exactly
    // the translation the old full-frame `crop` pass applied to the read-back
    // image). `crop == None` reads the whole target, unchanged.
    let readback = match config.crop {
        Some(crop_rect) => {
            let origin = to_target(
                Point::<i32, Physical>::from((crop_rect.x, crop_rect.y)),
                config.source,
            );
            Rectangle::<i32, BufferCoords>::new(
                Point::from((origin.x, origin.y)),
                Size::from((crop_rect.w as i32, crop_rect.h as i32)),
            )
        }
        None => Rectangle::from_size(buffer_size),
    };
    let readback_region = Rect::new(
        readback.loc.x,
        readback.loc.y,
        readback.size.w.max(0) as u32,
        readback.size.h.max(0) as u32,
    );

    let mapping = renderer
        .copy_framebuffer(&framebuffer, readback, READBACK_FORMAT)
        .map_err(|err| RenderError::Readback {
            region: readback_region,
            source: Box::new(err),
        })?;
    // Derive the geometry from the mapping itself rather than assuming the
    // sub-rect is tightly packed: neither backend guarantees that in the type
    // system, even though both currently return a fresh region-sized mapping.
    let mapped = Texture::size(&mapping);
    let width = u32::try_from(mapped.w).unwrap_or(0);
    let height = u32::try_from(mapped.h).unwrap_or(0);
    let bytes = renderer
        .map_texture(&mapping)
        .map_err(|err| RenderError::Readback {
            region: readback_region,
            source: Box::new(err),
        })?;
    // `map_texture` yields exactly `stride * height` bytes on both backends, so
    // the real row length is the mapped length divided by the row count.
    //
    // `TextureMapping::flipped()` is deliberately NOT consulted: it describes the
    // mapping relative to the renderer's *native* origin (lower-left for GL), not
    // relative to the scene. Both backends already hand back byte-identical rows
    // that are top-down in scene space — GL's projection applies `flip180`, so
    // scene `y = 0` lands in framebuffer row 0 and therefore in the first readback
    // row. Un-flipping here would mirror every GL capture vertically. Smithay's
    // own readback consumers likewise write the mapped bytes out verbatim.
    let stride = u32::try_from(bytes.len() / (height.max(1) as usize)).unwrap_or(u32::MAX);
    let image = image_from_readback(bytes, width, height, stride, /* flipped = */ false)?;

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
