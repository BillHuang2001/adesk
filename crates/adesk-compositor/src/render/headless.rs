//! Renderer construction and the offscreen render pipeline entry points.
//!
//! The compositor owns exactly one [`HeadlessRenderer`]. It is created once, on
//! the compositor thread, inside [`State::new`](crate::state::State) — before the
//! dmabuf global, which needs [`HeadlessRenderer::dmabuf_formats`] — and it is
//! never moved to another thread.
//!
//! Pixel production itself belongs to `adesk-render`: this module builds the
//! [`RenderConfig`], asks [`elements`] for the scene, and hands both — plus a
//! per-backend [`TargetPool`] — to [`adesk_render::render_scene`], which draws,
//! reads back, crops and downscales. Nothing here touches raw pixels. The pool
//! retains one offscreen target per source size so repeated captures of the same
//! window/output stop re-allocating a target every time.
//!
//! # Facts this module depends on
//!
//! * **`GlesRenderer` is `!Send`/`!Sync`.** Smithay marks it with
//!   `_not_send: PhantomData<*mut ()>` and it holds `Rc`-based GL state, so the
//!   renderer *and every offscreen target derived from it* may only be touched
//!   on the compositor thread. That is already the crate's threading contract.
//! * **There is no unified offscreen abstraction in Smithay 0.7.** The GL path
//!   uses `Offscreen<GlesRenderbuffer>`, the pixman path
//!   `Offscreen<pixman::Image<'static, 'static>>` (Smithay *does* re-export
//!   pixman's `Image` as `smithay::reexports::pixman::Image`, so no extra
//!   dependency is needed). Both targets are acquired from a
//!   [`adesk_render::TargetPool`] (which allocates through
//!   [`adesk_render::create_target`] on a miss) and rendered through
//!   [`adesk_render::render_scene`], which is why the two backends share one
//!   generic implementation below.
//! * **GL readback rows are already top-down in scene order.** `adesk-render`
//!   deliberately ignores `TextureMapping::flipped()` (`GlesMapping::flipped()`
//!   is `true` but describes GL's native lower-left origin, not scene space), so
//!   nothing here may flip the image again.
//! * **Render failures are never panics.** A `RenderError` becomes
//!   [`CompositorError::Render`] (`render_failed`) — except a malformed request
//!   (empty source, crop outside the source, ...), which becomes
//!   [`CompositorError::InvalidRequest`] (`invalid_request`), mirroring
//!   `adesk_render::RenderError::code`.
//! * Buffer-import failures inside the surface-tree walk are logged and dropped
//!   by Smithay itself (the element is absent, the rest of the frame renders);
//!   that is `render_elements_from_surface_tree`'s documented behaviour, not a
//!   silent error path of this crate.
//!
//! Renderer selection is the documented `--renderer auto|gl|pixman` behaviour:
//! `Gl` must succeed, `Pixman` always works headless, `Auto` prefers GL and
//! falls back to pixman with a warning. The result is reported by `ping`.

use adesk_core::{Rect, Size};
use adesk_render::{render_scene, RenderConfig, RenderError, Scene, TargetPool};
use smithay::{
    backend::{
        allocator::{format::FormatSet, Format},
        egl::{native::EGLSurfacelessDisplay, EGLContext, EGLDisplay},
        renderer::{
            element::RenderElement,
            gles::{GlesRenderbuffer, GlesRenderer},
            pixman::PixmanRenderer,
            ExportMem, ImportAll, ImportDma, Offscreen, Renderer,
        },
    },
    reexports::{pixman::Image as PixmanImage, wayland_server::protocol::wl_surface::WlSurface},
};

use super::{
    elements::{self, OutputRenderElements},
    OutputWindow,
};
use crate::{
    config::{RendererKind, RendererName},
    error::CompositorError,
    snapshot::RenderedFrame,
};

/// Offscreen target type of the GL backend.
type GlTarget = GlesRenderbuffer;

/// Offscreen target type of the pixman backend (`smithay::reexports::pixman::Image`).
type PixmanTarget = PixmanImage<'static, 'static>;

/// The compositor's renderer: GLES (EGL) or pixman (software).
///
/// One variant is chosen at startup and never changes; `Auto` only decides
/// *which* one is constructed. Both variants support the same operations, so the
/// rest of the crate never matches on the kind — except
/// [`dmabuf_formats`](HeadlessRenderer::dmabuf_formats), which asks the concrete
/// backend.
pub(crate) enum HeadlessRenderer {
    /// EGL/GLES renderer built on a surfaceless EGL display (boxed: it is ~6 KiB),
    /// with its offscreen-target reuse pool.
    Gl {
        /// The GLES renderer.
        renderer: Box<GlesRenderer>,
        /// Cached offscreen targets for repeated captures of the same size.
        pool: TargetPool<GlTarget>,
    },
    /// pixman software renderer (always available, no GPU required), with its
    /// offscreen-target reuse pool.
    Pixman {
        /// The software renderer.
        renderer: PixmanRenderer,
        /// Cached offscreen targets for repeated captures of the same size.
        pool: TargetPool<PixmanTarget>,
    },
}

impl HeadlessRenderer {
    /// Create the renderer selected by `kind`.
    ///
    /// * [`RendererKind::Pixman`] — always succeeds headless.
    /// * [`RendererKind::Gl`] — fails startup if surfaceless EGL is unavailable.
    /// * [`RendererKind::Auto`] — tries GL, and on failure logs
    ///   `GL renderer unavailable, falling back to pixman` at `warn` level and
    ///   builds the pixman renderer.
    pub(crate) fn create(kind: RendererKind) -> crate::Result<HeadlessRenderer> {
        match kind {
            RendererKind::Pixman => Ok(HeadlessRenderer::Pixman {
                renderer: create_pixman()?,
                pool: TargetPool::new(),
            }),
            RendererKind::Gl => Ok(HeadlessRenderer::Gl {
                renderer: Box::new(create_gl()?),
                pool: TargetPool::new(),
            }),
            RendererKind::Auto => match create_gl() {
                Ok(renderer) => Ok(HeadlessRenderer::Gl {
                    renderer: Box::new(renderer),
                    pool: TargetPool::new(),
                }),
                Err(error) => {
                    tracing::warn!(error = %error, "GL renderer unavailable, falling back to pixman");
                    Ok(HeadlessRenderer::Pixman {
                        renderer: create_pixman()?,
                        pool: TargetPool::new(),
                    })
                }
            },
        }
    }

    /// The renderer that was actually created, for [`crate::ReadyInfo`] and the
    /// AGP `ping` result (`"gl"` / `"pixman"`).
    pub(crate) fn name(&self) -> RendererName {
        match self {
            HeadlessRenderer::Gl { .. } => RendererName::Gl,
            HeadlessRenderer::Pixman { .. } => RendererName::Pixman,
        }
    }

    /// The dmabuf formats this renderer can import, as advertised by the
    /// `zwp_linux_dmabuf` global.
    ///
    /// Both backends implement `ImportDma`: `GlesRenderer` reports the EGL
    /// dmabuf texture formats, `PixmanRenderer` a static set of single-plane
    /// linear formats it can map. `Format` is
    /// `smithay::backend::allocator::Format` (a `drm_fourcc::DrmFormat`:
    /// fourcc + modifier), the same type `DmabufState::create_global` consumes;
    /// `FormatSet` is `smithay::backend::allocator::format::FormatSet` and
    /// converts through `IntoIterator<Item = Format>`.
    pub(crate) fn dmabuf_formats(&self) -> Vec<Format> {
        let formats: FormatSet = match self {
            HeadlessRenderer::Gl { renderer, .. } => renderer.dmabuf_formats(),
            HeadlessRenderer::Pixman { renderer, .. } => renderer.dmabuf_formats(),
        };
        formats.into_iter().collect()
    }

    /// Render one window's surface tree (toplevel + subsurfaces + popups) into
    /// an `Rgba8` frame.
    ///
    /// `geometry` is the window rectangle in output pixels; the target is sized
    /// to it and the tree is rendered window-relative (toplevel at `(0, 0)`,
    /// popups at their offsets). `region` crops the result and `max_dimension`
    /// caps the long edge after cropping (`docs/architecture.md` §5); both are in
    /// window-relative pixels. The returned `commit_seq` is `0` — the caller
    /// (`State::render_window`) stamps the window's commit counter — and `damage`
    /// is the union of the drawn element rectangles, clipped to the window.
    pub(crate) fn render_window(
        &mut self,
        surface: &WlSurface,
        geometry: Rect,
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> crate::Result<RenderedFrame> {
        let config = render_config(geometry.size(), region, max_dimension);
        match self {
            HeadlessRenderer::Gl { renderer, pool } => {
                render_window_gl(renderer, pool, surface, &config)
            }
            HeadlessRenderer::Pixman { renderer, pool } => {
                render_window_pixman(renderer, pool, surface, &config)
            }
        }
    }

    /// Compose the whole virtual output: the single **visible** window, then
    /// crop/downscale/read back.
    ///
    /// `output_size` is the virtual output's pixel size and sizes the target, so
    /// a candidate list with no active window is a **valid clear frame**, not an
    /// error. `windows` is the candidate list; [`elements::output_scene`] draws
    /// only the active candidate, because ADesk shows one toplevel at a time.
    /// `region` and `max_dimension` are output-relative. `commit_seq` of the
    /// resulting frame is `0`: an output composition is not tied to a single
    /// window's commit counter.
    pub(crate) fn render_output(
        &mut self,
        output_size: Size,
        windows: &[OutputWindow],
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> crate::Result<RenderedFrame> {
        let config = render_config(output_size, region, max_dimension);
        match self {
            HeadlessRenderer::Gl { renderer, pool } => {
                render_output_gl(renderer, pool, windows, &config)
            }
            HeadlessRenderer::Pixman { renderer, pool } => {
                render_output_pixman(renderer, pool, windows, &config)
            }
        }
    }
}

/// Create a GLES renderer on a surfaceless EGL display.
///
/// This is the whole EGL bootstrap: platform display → configless context →
/// make current → renderer. Every step maps to
/// [`CompositorError::Renderer`] so startup reports a single, readable cause.
fn create_gl() -> crate::Result<GlesRenderer> {
    // SAFETY: `EGLSurfacelessDisplay` is a zero-sized `EGLNativeDisplay` that
    // selects `EGL_MESA_platform_surfaceless` with `EGL_DEFAULT_DISPLAY`; no
    // borrowed native handle escapes this call. Smithay refcounts `EGLDisplay`s
    // internally and the `EGLContext` created below keeps a clone alive, so
    // dropping the local binding does not terminate the display.
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }
        .map_err(|error| CompositorError::Renderer(format!("surfaceless EGL display: {error}")))?;

    // Configless context: succeeds when the display exposes
    // `EGL_KHR_no_config_context`, `EGL_MESA_configless_context` or
    // `EGL_KHR_surfaceless_context` (Mesa's llvmpipe exposes them).
    let context = EGLContext::new(&display)
        .map_err(|error| CompositorError::Renderer(format!("EGL context: {error}")))?;

    // SAFETY: the context was just created and is not current on any thread, so
    // binding it here cannot race. From now on it belongs to the compositor
    // thread only (see the threading contract in the crate root docs).
    unsafe { context.make_current() }
        .map_err(|error| CompositorError::Renderer(format!("EGL make_current: {error}")))?;

    // SAFETY: the context is current on this thread and not active elsewhere,
    // which is exactly what `GlesRenderer::new` requires. The renderer takes
    // ownership of the context and is `!Send`, so it cannot leave this thread.
    unsafe { GlesRenderer::new(context) }
        .map_err(|error| CompositorError::Renderer(format!("GLES renderer: {error}")))
}

/// Create the pixman software renderer.
///
/// Needs no GPU, no EGL and no display: this is the path CI and the sandbox
/// always have. The only failure mode is pixman itself being unusable.
fn create_pixman() -> crate::Result<PixmanRenderer> {
    PixmanRenderer::new()
        .map_err(|error| CompositorError::Renderer(format!("pixman renderer: {error}")))
}

/// Render configuration of one render source: target sized to `source`, with
/// optional crop (`region`, relative to `source`) and optional downscale
/// (`max_dimension`, the capped long edge).
///
/// Both render entry points use it: [`HeadlessRenderer::render_window`] passes the
/// window rectangle's size (so the origin is never part of the source) and
/// [`HeadlessRenderer::render_output`] the virtual output's size.
fn render_config(source: Size, region: Option<Rect>, max_dimension: Option<u32>) -> RenderConfig {
    let mut config = RenderConfig::new(Rect::from_size(source));
    if let Some(region) = region {
        config = config.with_crop(region);
    }
    if let Some(max_dimension) = max_dimension {
        config = config.with_max_dimension(max_dimension);
    }
    config
}

/// Shared render pass: acquire a backend target from `pool`, render the scene,
/// return the target to the pool (on success only), and convert the pipeline
/// frame into the compositor's reply payload.
///
/// `T` is the backend's offscreen target type; the caller picks it, everything
/// else is renderer-independent.
fn render_scene_frame<R, T, E>(
    renderer: &mut R,
    pool: &mut TargetPool<T>,
    scene: &Scene<E>,
    config: &RenderConfig,
) -> crate::Result<RenderedFrame>
where
    R: Renderer + Offscreen<T> + ExportMem,
    R::Error: Send + Sync + 'static,
    E: RenderElement<R>,
{
    // Validate before allocating: an empty source or a crop outside the source is
    // a client mistake (`invalid_request`), not a renderer failure.
    config.validate().map_err(render_error)?;
    // A cached target of the same source size is reused; otherwise one is
    // allocated. The pipeline clears the whole target before drawing, so reuse
    // is byte-identical to a fresh allocation (see `adesk_render::TargetPool`).
    let mut target = pool
        .acquire(renderer, config.target_size())
        .map_err(render_error)?;
    match render_scene(renderer, &mut target, scene, config) {
        Ok(frame) => {
            // Only a target that completed a full, successful clear-and-draw
            // pass is safe to cache; a failed render drops it here.
            pool.release(target);
            Ok(RenderedFrame::new(
                frame.image,
                frame.commit_seq,
                frame.damage.simplified(),
            ))
        }
        Err(error) => Err(render_error(error)),
    }
}

/// Maps a pipeline failure onto the compositor error that carries the right AGP
/// code, mirroring `adesk_render::RenderError::code`.
fn render_error(error: RenderError) -> CompositorError {
    let message = error.to_string();
    match error {
        RenderError::InvalidConfig { .. } | RenderError::InvalidImage { .. } => {
            CompositorError::InvalidRequest(message)
        }
        _ => CompositorError::Render(message),
    }
}

/// GL path for [`HeadlessRenderer::render_window`].
fn render_window_gl(
    renderer: &mut GlesRenderer,
    pool: &mut TargetPool<GlTarget>,
    surface: &WlSurface,
    config: &RenderConfig,
) -> crate::Result<RenderedFrame> {
    render_window_frame::<_, GlTarget>(renderer, pool, surface, config)
}

/// pixman path for [`HeadlessRenderer::render_window`].
///
/// The software path renders SHM-backed windows fully; a DMA-BUF the software
/// renderer cannot import is dropped by Smithay's element walk (logged), and any
/// pipeline failure surfaces as [`CompositorError::Render`] — never a panic.
fn render_window_pixman(
    renderer: &mut PixmanRenderer,
    pool: &mut TargetPool<PixmanTarget>,
    surface: &WlSurface,
    config: &RenderConfig,
) -> crate::Result<RenderedFrame> {
    render_window_frame::<_, PixmanTarget>(renderer, pool, surface, config)
}

/// GL path for [`HeadlessRenderer::render_output`].
fn render_output_gl(
    renderer: &mut GlesRenderer,
    pool: &mut TargetPool<GlTarget>,
    windows: &[OutputWindow],
    config: &RenderConfig,
) -> crate::Result<RenderedFrame> {
    render_output_frame::<_, GlTarget>(renderer, pool, windows, config)
}

/// pixman path for [`HeadlessRenderer::render_output`].
///
/// Same composition as the GL path, but into a software target, so the whole
/// output (including the clear frame with no windows) works without a GPU.
fn render_output_pixman(
    renderer: &mut PixmanRenderer,
    pool: &mut TargetPool<PixmanTarget>,
    windows: &[OutputWindow],
    config: &RenderConfig,
) -> crate::Result<RenderedFrame> {
    render_output_frame::<_, PixmanTarget>(renderer, pool, windows, config)
}

/// Backend-independent window render: collect the window's scene and render it.
fn render_window_frame<R, T>(
    renderer: &mut R,
    pool: &mut TargetPool<T>,
    surface: &WlSurface,
    config: &RenderConfig,
) -> crate::Result<RenderedFrame>
where
    R: Renderer + ImportAll + Offscreen<T> + ExportMem,
    R::TextureId: Clone + 'static,
    R::Error: Send + Sync + 'static,
{
    let scene = elements::window_scene(renderer, surface);
    render_scene_frame(renderer, pool, &scene, config)
}

/// Backend-independent output render: collect the composition and render it.
fn render_output_frame<R, T>(
    renderer: &mut R,
    pool: &mut TargetPool<T>,
    windows: &[OutputWindow],
    config: &RenderConfig,
) -> crate::Result<RenderedFrame>
where
    R: Renderer + ImportAll + Offscreen<T> + ExportMem,
    R::TextureId: Clone + 'static,
    R::Error: Send + Sync + 'static,
{
    let scene: Scene<OutputRenderElements<R>> = elements::output_scene(renderer, windows);
    render_scene_frame(renderer, pool, &scene, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_render::DEFAULT_CLEAR_COLOR;

    /// Creates the software renderer; pixman needs no display, GPU or EGL.
    fn pixman() -> HeadlessRenderer {
        HeadlessRenderer::create(RendererKind::Pixman).expect("pixman renderer")
    }

    /// Creates the GL renderer only when the test environment provides EGL
    /// (`ADESK_TEST_GL=1`); returns `None` (skip) otherwise.
    fn gl() -> Option<HeadlessRenderer> {
        if std::env::var("ADESK_TEST_GL").as_deref() != Ok("1") {
            eprintln!("skipping GL test: set ADESK_TEST_GL=1 to enable it");
            return None;
        }
        match HeadlessRenderer::create(RendererKind::Gl) {
            Ok(renderer) => Some(renderer),
            Err(error) => {
                eprintln!("skipping GL test: EGL/GLES unavailable: {error}");
                None
            }
        }
    }

    /// Asserts that a frame is a uniform clear frame of the expected size.
    fn assert_clear_frame(frame: &RenderedFrame, width: u32, height: u32) {
        assert_eq!(frame.image.width, width);
        assert_eq!(frame.image.height, height);
        assert_eq!(frame.commit_seq, 0, "output composition has no commit seq");
        assert!(
            frame.damage.is_empty(),
            "an empty composition draws nothing, so it reports no damage"
        );
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    frame.image.pixel(x, y),
                    Some(DEFAULT_CLEAR_COLOR),
                    "pixel ({x}, {y}) is not the clear color"
                );
            }
        }
    }

    #[test]
    fn pixman_output_without_windows_is_a_clear_frame() {
        let mut renderer = pixman();
        let frame = renderer
            .render_output(Size::new(4, 3), &[], None, None)
            .expect("empty output composition must render");
        assert_clear_frame(&frame, 4, 3);
    }

    #[test]
    fn pixman_output_applies_crop_and_downscale() {
        let mut renderer = pixman();
        let frame = renderer
            .render_output(Size::new(16, 8), &[], Some(Rect::new(2, 1, 8, 4)), Some(4))
            .expect("crop and downscale must apply to the output composition");
        // 8x4 crop, longest edge capped at 4 -> 4x2.
        assert_eq!(frame.size(), Size::new(4, 2));
        assert_eq!(frame.commit_seq, 0);
    }

    #[test]
    fn empty_output_size_is_an_invalid_request() {
        let mut renderer = pixman();
        let error = renderer
            .render_output(Size::ZERO, &[], None, None)
            .expect_err("a 0x0 output cannot be rendered");
        assert!(matches!(error, CompositorError::InvalidRequest(_)));
        assert_eq!(error.code(), adesk_core::ErrorCode::InvalidRequest);
    }

    #[test]
    fn invalid_window_crop_is_an_invalid_request() {
        let mut renderer = create_pixman().expect("pixman renderer");
        let mut pool = TargetPool::new();
        let scene = Scene::<OutputRenderElements<PixmanRenderer>>::new(0);
        let config = render_config(Size::new(10, 10), Some(Rect::new(50, 50, 4, 4)), None);
        let error =
            render_scene_frame::<_, PixmanTarget, _>(&mut renderer, &mut pool, &scene, &config)
                .expect_err("a crop outside the window is rejected");
        assert!(matches!(error, CompositorError::InvalidRequest(_)));
        assert_eq!(error.code(), adesk_core::ErrorCode::InvalidRequest);
    }

    #[test]
    fn render_config_sizes_the_target_to_the_window_source() {
        let config = render_config(
            Size::new(640, 480),
            Some(Rect::new(10, 20, 100, 50)),
            Some(64),
        );
        // Window-relative source: the geometry origin is never part of it.
        assert_eq!(config.source, Rect::new(0, 0, 640, 480));
        assert_eq!(config.target_size(), Size::new(640, 480));
        assert_eq!(config.crop, Some(Rect::new(10, 20, 100, 50)));
        assert_eq!(config.max_dimension, Some(64));
        // 100x50 crop downscaled to a longest edge of 64 -> 64x32.
        assert_eq!(config.output_size(), Size::new(64, 32));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn render_config_sizes_the_target_to_the_output_size() {
        let config = render_config(Size::new(1280, 800), None, None);
        assert_eq!(config.source, Rect::new(0, 0, 1280, 800));
        assert_eq!(config.target_size(), Size::new(1280, 800));
        assert_eq!(config.output_size(), Size::new(1280, 800));
        assert_eq!(config.clear_color, DEFAULT_CLEAR_COLOR);
    }

    #[test]
    fn gl_output_without_windows_is_a_clear_frame() {
        let Some(mut renderer) = gl() else {
            return;
        };
        let frame = renderer
            .render_output(Size::new(4, 3), &[], None, None)
            .expect("empty output composition must render on GL too");
        // The GL readback is consumed top-down in scene order by adesk-render, so
        // the frame must be identical to the software clear frame.
        assert_clear_frame(&frame, 4, 3);
        eprintln!("GL test ran: surfaceless EGL clear frame matched the software path");
    }

    #[test]
    fn renderer_name_and_scene_scale_match_the_contract() {
        assert_eq!(pixman().name(), RendererName::Pixman);
        assert_eq!(elements::SCENE_SCALE, 1.0);
    }
}
