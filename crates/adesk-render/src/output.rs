//! Offscreen render targets and the frames read back from them.

use adesk_core::{ImageBuffer, Region, Size};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::Offscreen;
use smithay::utils::{Buffer as BufferCoords, Size as TargetSize};

use crate::error::Result;

/// An offscreen render target of a known size and pixel format.
///
/// The inner `T` is the renderer's own target type (`GlesRenderbuffer` for the
/// GL renderer, `pixman::Image` for the software renderer). This crate never
/// names those types: `adesk-compositor` creates the target through
/// [`create_target`] and the pipeline binds it via `Bind<T>`.
#[derive(Debug)]
pub struct OffscreenTarget<T> {
    target: T,
    size: TargetSize<i32, BufferCoords>,
    format: Fourcc,
}

impl<T> OffscreenTarget<T> {
    /// Wraps an already allocated renderer target.
    pub fn new(target: T, size: TargetSize<i32, BufferCoords>, format: Fourcc) -> Self {
        Self {
            target,
            size,
            format,
        }
    }

    /// Target size in buffer coordinates.
    pub fn size(&self) -> TargetSize<i32, BufferCoords> {
        self.size
    }

    /// Target size as a core [`Size`].
    pub fn core_size(&self) -> Size {
        Size::new(self.size.w.max(0) as u32, self.size.h.max(0) as u32)
    }

    /// Pixel format of the target.
    pub fn format(&self) -> Fourcc {
        self.format
    }

    /// The renderer-specific target object.
    pub fn texture(&self) -> &T {
        &self.target
    }

    /// Mutable access to the renderer-specific target object (used for binding).
    pub fn texture_mut(&mut self) -> &mut T {
        &mut self.target
    }

    /// Consumes the wrapper, returning the renderer target.
    pub fn into_inner(self) -> T {
        self.target
    }
}

/// Allocates an offscreen target of `size` pixels through the renderer.
///
/// `T` is the renderer's offscreen target type; the renderer must implement
/// `Offscreen<T>` (both `GlesRenderer` and `PixmanRenderer` do, for
/// `GlesRenderbuffer` and `pixman::Image<'static, 'static>` respectively).
pub fn create_target<R, T>(renderer: &mut R, size: Size) -> Result<OffscreenTarget<T>>
where
    R: Offscreen<T>,
{
    let _ = (renderer, size);
    todo!("create_target: renderer.create_buffer(TARGET_FORMAT, size), wrap in OffscreenTarget")
}

/// The result of one offscreen render pass.
///
/// `image` is the (optionally cropped and downscaled) read-back in
/// [`adesk_core::PixelFormat::Rgba8`]; `commit_seq` is the commit watermark of
/// the scene the frame was rendered from; `damage` is the scene-level damage
/// evidence in **scene coordinates** (window-relative for a window render),
/// independent of crop/downscale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFrame {
    /// Read-back pixels, `Rgba8`.
    pub image: ImageBuffer,
    /// Commit watermark of the scene this frame was rendered from.
    pub commit_seq: u64,
    /// Damage evidence in scene coordinates.
    pub damage: Region,
}

impl RenderedFrame {
    /// Creates a rendered frame.
    pub fn new(image: ImageBuffer, commit_seq: u64, damage: Region) -> Self {
        Self {
            image,
            commit_seq,
            damage,
        }
    }

    /// Size of the returned image.
    pub fn size(&self) -> Size {
        self.image.size()
    }
}
