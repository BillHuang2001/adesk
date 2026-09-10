//! A tiny, bounded cache of offscreen render targets.
//!
//! Every capture (`capture_window`, `capture_region`, `observe(include_image)`,
//! `inspect_capture`, one VAP frame) renders into an offscreen target of the
//! source size — roughly 4 MiB at 1280×800. Allocating a fresh target per
//! capture is wasteful, but a target cannot be reused blindly either: after a
//! pass it still holds the previous frame's pixels.
//!
//! [`render_scene`](crate::render_scene) fully **clears** the target rectangle
//! before drawing anything, so a reused target's stale contents can never leak
//! into the read-back: reuse is byte-identical to a fresh allocation. That is
//! the whole correctness argument for this pool, and it is why only a target
//! that a *successful* render produced is ever cached — a target that saw a
//! failed render is dropped by the caller, never returned here.
//!
//! Keying is by **exact** target size: a cached target is reused only when its
//! `core_size()` equals the requested size.

use adesk_core::Size;
use smithay::backend::renderer::Offscreen;

use crate::error::Result;
use crate::output::{create_target, OffscreenTarget};

/// Maximum number of idle targets the pool retains.
///
/// One target per distinct size, up to this many sizes; once the pool is full a
/// newly released target is dropped instead of cached. A target is ~4 MiB at
/// 1280×800, so the pool holds at most a few tens of MiB regardless of how many
/// distinct sizes are observed — bounded memory, never growth proportional to
/// the number of captures.
const MAX_POOLED_TARGETS: usize = 4;

/// A small, bounded cache of offscreen render targets keyed by exact pixel size.
///
/// The pool is per-renderer state (targets are owned by the renderer that
/// allocated them) and lives on the compositor thread together with its
/// renderer. See the module docs for the reuse correctness argument.
#[derive(Debug)]
pub struct TargetPool<T> {
    idle: Vec<OffscreenTarget<T>>,
}

impl<T> TargetPool<T> {
    /// Creates an empty pool.
    pub fn new() -> Self {
        Self { idle: Vec::new() }
    }

    /// Returns a cached target of exactly `size`, or allocates a fresh one.
    ///
    /// A returned target may still hold the previous pass's pixels; the caller
    /// must fully clear it before drawing, which
    /// [`render_scene`](crate::render_scene) does.
    pub fn acquire<R>(&mut self, renderer: &mut R, size: Size) -> Result<OffscreenTarget<T>>
    where
        R: Offscreen<T>,
    {
        if let Some(index) = self
            .idle
            .iter()
            .position(|target| target.core_size() == size)
        {
            return Ok(self.idle.swap_remove(index));
        }
        create_target(renderer, size)
    }

    /// Returns `target` to the pool so a later [`acquire`](Self::acquire) of the
    /// same size can reuse it.
    ///
    /// At most one target per size is retained and at most
    /// `MAX_POOLED_TARGETS` in total; a surplus target is dropped and its memory
    /// reclaimed. Callers must not release a target whose render failed (its
    /// contents are undefined) — the compositor drops such targets instead.
    pub fn release(&mut self, target: OffscreenTarget<T>) {
        let size = target.core_size();
        if self.idle.iter().any(|pooled| pooled.core_size() == size) {
            // Exactly one cached target per size keeps the pool's key unique.
            return;
        }
        if self.idle.len() < MAX_POOLED_TARGETS {
            self.idle.push(target);
        }
    }
}

impl<T> Default for TargetPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::pixman::PixmanRenderer;
    use smithay::backend::renderer::sync::SyncPoint;
    use smithay::backend::renderer::{
        Bind, ContextId, DebugFlags, Offscreen, Renderer, RendererSuper, TextureFilter,
    };
    use smithay::reexports::pixman::Image as PixmanImage;
    use smithay::utils::{Buffer as BufferCoords, Physical, Size as TargetSize, Transform};

    use super::{TargetPool, MAX_POOLED_TARGETS};
    use adesk_core::Size;

    /// The software backend's offscreen target type.
    type FakeTarget = PixmanImage<'static, 'static>;

    /// A real `PixmanRenderer` wrapper that counts `create_buffer` calls.
    ///
    /// Every trait method delegates to the wrapped renderer; the only behaviour
    /// this adds is the allocation counter, so a test can prove that a
    /// `TargetPool::acquire` reused a cached target instead of allocating a new
    /// one. The tests never render, so the delegated methods are inert.
    #[derive(Debug)]
    struct CountingRenderer {
        inner: PixmanRenderer,
        created: usize,
    }

    impl CountingRenderer {
        fn new() -> Self {
            Self {
                inner: PixmanRenderer::new().expect("pixman renderer"),
                created: 0,
            }
        }

        /// Number of `create_buffer` calls observed so far.
        fn created(&self) -> usize {
            self.created
        }
    }

    impl RendererSuper for CountingRenderer {
        type Error = <PixmanRenderer as RendererSuper>::Error;
        type TextureId = <PixmanRenderer as RendererSuper>::TextureId;
        type Framebuffer<'buffer> = <PixmanRenderer as RendererSuper>::Framebuffer<'buffer>;
        type Frame<'frame, 'buffer>
            = <PixmanRenderer as RendererSuper>::Frame<'frame, 'buffer>
        where
            'buffer: 'frame;
    }

    impl Renderer for CountingRenderer {
        fn context_id(&self) -> ContextId<Self::TextureId> {
            self.inner.context_id()
        }

        fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
            self.inner.downscale_filter(filter)
        }

        fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
            self.inner.upscale_filter(filter)
        }

        fn set_debug_flags(&mut self, flags: DebugFlags) {
            self.inner.set_debug_flags(flags)
        }

        fn debug_flags(&self) -> DebugFlags {
            self.inner.debug_flags()
        }

        fn render<'frame, 'buffer>(
            &'frame mut self,
            framebuffer: &'frame mut Self::Framebuffer<'buffer>,
            output_size: TargetSize<i32, Physical>,
            dst_transform: Transform,
        ) -> Result<Self::Frame<'frame, 'buffer>, Self::Error>
        where
            'buffer: 'frame,
        {
            self.inner.render(framebuffer, output_size, dst_transform)
        }

        fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
            self.inner.wait(sync)
        }
    }

    impl Bind<FakeTarget> for CountingRenderer {
        fn bind<'a>(
            &mut self,
            target: &'a mut FakeTarget,
        ) -> Result<Self::Framebuffer<'a>, Self::Error> {
            self.inner.bind(target)
        }
    }

    impl Offscreen<FakeTarget> for CountingRenderer {
        fn create_buffer(
            &mut self,
            format: Fourcc,
            size: TargetSize<i32, BufferCoords>,
        ) -> Result<FakeTarget, Self::Error> {
            self.created += 1;
            self.inner.create_buffer(format, size)
        }
    }

    #[test]
    fn acquire_reuses_a_cached_target_of_the_same_size() {
        let mut renderer = CountingRenderer::new();
        let mut pool = TargetPool::new();
        let size = Size::new(64, 48);

        let first = pool.acquire(&mut renderer, size).expect("first acquire");
        assert_eq!(first.core_size(), size);
        assert_eq!(renderer.created(), 1, "the first acquire must allocate");
        pool.release(first);

        let second = pool.acquire(&mut renderer, size).expect("second acquire");
        assert_eq!(second.core_size(), size);
        assert_eq!(
            renderer.created(),
            1,
            "a same-size acquire must reuse the cached target, not allocate"
        );
    }

    #[test]
    fn acquire_of_a_different_size_does_not_reuse() {
        let mut renderer = CountingRenderer::new();
        let mut pool = TargetPool::new();

        let first = pool.acquire(&mut renderer, Size::new(64, 48)).unwrap();
        pool.release(first);

        let other = pool.acquire(&mut renderer, Size::new(32, 32)).unwrap();
        assert_eq!(other.core_size(), Size::new(32, 32));
        assert_eq!(
            renderer.created(),
            2,
            "a different size must not be served from the cached target"
        );
    }

    #[test]
    fn release_then_acquire_serves_the_same_size_and_reallocates_others() {
        let mut renderer = CountingRenderer::new();
        let mut pool = TargetPool::new();

        let target = pool.acquire(&mut renderer, Size::new(10, 10)).unwrap();
        pool.release(target);

        let reused = pool.acquire(&mut renderer, Size::new(10, 10)).unwrap();
        assert_eq!(renderer.created(), 1, "same size: produced by reuse");
        pool.release(reused);

        let fresh = pool.acquire(&mut renderer, Size::new(20, 20)).unwrap();
        assert_eq!(fresh.core_size(), Size::new(20, 20));
        assert_eq!(renderer.created(), 2, "different size: freshly allocated");
    }

    #[test]
    fn release_keeps_at_most_one_target_per_size() {
        let mut renderer = CountingRenderer::new();
        let mut pool = TargetPool::new();
        let size = Size::new(8, 8);

        // Two separate allocations of the same size, both released.
        let a = pool.acquire(&mut renderer, size).unwrap();
        let b = pool.acquire(&mut renderer, size).unwrap();
        assert_eq!(renderer.created(), 2);
        pool.release(a);
        pool.release(b); // dropped: one target is already cached for this size

        let first = pool.acquire(&mut renderer, size).unwrap();
        assert_eq!(renderer.created(), 2, "the single cached target is reused");
        let second = pool.acquire(&mut renderer, size).unwrap();
        assert_eq!(
            renderer.created(),
            3,
            "only one target was cached for this size, so the second acquire allocates"
        );
        drop((first, second));
    }

    #[test]
    fn pool_is_bounded_across_distinct_sizes() {
        let mut renderer = CountingRenderer::new();
        let mut pool = TargetPool::new();

        // Release more distinct sizes than the pool can hold.
        for offset in 0..(MAX_POOLED_TARGETS as u32 + 2) {
            let target = pool
                .acquire(&mut renderer, Size::new(64, 48 + offset))
                .unwrap();
            pool.release(target);
        }
        let allocations = renderer.created();
        assert_eq!(allocations, MAX_POOLED_TARGETS + 2);

        // A retained size is served from the cache without allocating.
        let reused = pool.acquire(&mut renderer, Size::new(64, 48)).unwrap();
        assert_eq!(renderer.created(), allocations, "retained size must reuse");
        pool.release(reused);

        // An evicted size reallocates.
        let evicted = Size::new(64, 48 + MAX_POOLED_TARGETS as u32 + 1);
        let target = pool.acquire(&mut renderer, evicted).unwrap();
        assert_eq!(target.core_size(), evicted);
        assert_eq!(
            renderer.created(),
            allocations + 1,
            "a size evicted by the cap must allocate again"
        );
    }
}
