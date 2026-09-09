//! Renderer construction: surfaceless-EGL GLES or pixman software.
//!
//! The compositor owns exactly one [`HeadlessRenderer`]. It is created once, on
//! the compositor thread, inside [`State::new`](crate::state::State) — before the
//! dmabuf global, which needs [`HeadlessRenderer::dmabuf_formats`] — and it is
//! never moved to another thread.
//!
//! # Facts Phase 2 must not have to re-discover
//!
//! * **`GlesRenderer` is `!Send`/`!Sync`.** Smithay marks it with
//!   `_not_send: PhantomData<*mut ()>` and it holds `Rc`-based GL state, so the
//!   renderer *and every offscreen target derived from it* may only be touched
//!   on the compositor thread. That is already the crate's threading contract.
//! * **GL readback is y-flipped.** `GlesMapping` implements
//!   `TextureMapping::flipped()` as `true`
//!   (`smithay-0.7.0/src/backend/renderer/gles/texture.rs:216`), so pixels
//!   obtained through `ExportMem::copy_framebuffer` come out bottom-up and must
//!   be flipped before they become `ImageBuffer` rows.
//! * **There is no unified offscreen abstraction in Smithay 0.7.** The GL path
//!   uses `Offscreen<GlesTexture>` (`renderer/gles/mod.rs:1559`), the pixman
//!   path `Offscreen<Image<'static, 'static>>` (`renderer/pixman/mod.rs:1246`,
//!   where `Image` comes from the `pixman` crate and is *not* re-exported by
//!   Smithay). Both are driven through
//!   `Offscreen::create_buffer(Fourcc, Size<i32, BufferCoord>)`, and both
//!   renderers implement `ExportMem::copy_framebuffer`.
//! * **Crop, downscale, readback and encoding are `adesk-render`'s job**
//!   (`docs/architecture.md` §5). The reconciliation contract with that crate is
//!   still pending its landing, so the compositor keeps only renderer
//!   construction and element collection here and hands raw framebuffers over
//!   later. The `todo!()`s below mark exactly where.
//!
//! Renderer selection is the documented `--renderer auto|gl|pixman` behaviour:
//! `Gl` must succeed, `Pixman` always works headless, `Auto` prefers GL and
//! falls back to pixman with a warning. The result is reported by `ping`.

use adesk_core::{OverlayKind, Rect};
use smithay::{
    backend::{
        allocator::{format::FormatSet, Format},
        egl::{native::EGLSurfacelessDisplay, EGLContext, EGLDisplay},
        renderer::{gles::GlesRenderer, pixman::PixmanRenderer, ImportDma},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
};

use super::OutputWindow;
use crate::{
    config::{RendererKind, RendererName},
    error::CompositorError,
    snapshot::RenderedFrame,
};

/// The compositor's renderer: GLES (EGL) or pixman (software).
///
/// One variant is chosen at startup and never changes; `Auto` only decides
/// *which* one is constructed. Both variants support the same operations, so the
/// rest of the crate never matches on the kind — except
/// [`dmabuf_formats`](HeadlessRenderer::dmabuf_formats), which asks the concrete
/// backend.
pub(crate) enum HeadlessRenderer {
    /// EGL/GLES renderer built on a surfaceless EGL display.
    Gl(GlesRenderer),
    /// pixman software renderer (always available, no GPU required).
    Pixman(PixmanRenderer),
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
            RendererKind::Pixman => Ok(HeadlessRenderer::Pixman(create_pixman()?)),
            RendererKind::Gl => Ok(HeadlessRenderer::Gl(create_gl()?)),
            RendererKind::Auto => match create_gl() {
                Ok(renderer) => Ok(HeadlessRenderer::Gl(renderer)),
                Err(error) => {
                    tracing::warn!(error = %error, "GL renderer unavailable, falling back to pixman");
                    Ok(HeadlessRenderer::Pixman(create_pixman()?))
                }
            },
        }
    }

    /// The renderer that was actually created, for [`crate::ReadyInfo`] and the
    /// AGP `ping` result (`"gl"` / `"pixman"`).
    pub(crate) fn name(&self) -> RendererName {
        match self {
            HeadlessRenderer::Gl(_) => RendererName::Gl,
            HeadlessRenderer::Pixman(_) => RendererName::Pixman,
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
            HeadlessRenderer::Gl(renderer) => renderer.dmabuf_formats(),
            HeadlessRenderer::Pixman(renderer) => renderer.dmabuf_formats(),
        };
        formats.into_iter().collect()
    }

    /// Render one window's surface tree (toplevel + subsurfaces + popups) into
    /// an `Rgba8` frame.
    ///
    /// `geometry` is the window rectangle in output pixels and sizes the
    /// offscreen target; `region` crops the result and `max_dimension` caps the
    /// long edge after cropping (`docs/architecture.md` §5). The returned
    /// `commit_seq`/`damage` come from the surface tree's damage state.
    pub(crate) fn render_window(
        &mut self,
        surface: &WlSurface,
        geometry: Rect,
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> crate::Result<RenderedFrame> {
        match self {
            HeadlessRenderer::Gl(renderer) => {
                render_window_gl(renderer, surface, geometry, region, max_dimension)
            }
            HeadlessRenderer::Pixman(renderer) => {
                render_window_pixman(renderer, surface, geometry, region, max_dimension)
            }
        }
    }

    /// Compose the whole virtual output: every window in z-order plus optional
    /// debug overlays, then crop/downscale/read back.
    ///
    /// `commit_seq` of the resulting frame is `0`: an output composition is not
    /// tied to a single window's commit counter.
    pub(crate) fn render_output(
        &mut self,
        windows: &[OutputWindow],
        overlays: &[OverlayKind],
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> crate::Result<RenderedFrame> {
        match self {
            HeadlessRenderer::Gl(renderer) => {
                render_output_gl(renderer, windows, overlays, region, max_dimension)
            }
            HeadlessRenderer::Pixman(renderer) => {
                render_output_pixman(renderer, windows, overlays, region, max_dimension)
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

/// GL path for [`HeadlessRenderer::render_window`].
///
/// Phase 2: `Offscreen<GlesTexture>::create_buffer(Fourcc::Abgr8888, size)`,
/// `OutputDamageTracker::render_output`, `ExportMem::copy_framebuffer(..,
/// Fourcc::Abgr8888)`, `map_texture` (y-flipped: `GlesMapping::flipped()` is
/// `true`), then crop/downscale/readback through `adesk-render`.
fn render_window_gl(
    _renderer: &mut GlesRenderer,
    _surface: &WlSurface,
    _geometry: Rect,
    _region: Option<Rect>,
    _max_dimension: Option<u32>,
) -> crate::Result<RenderedFrame> {
    todo!("Phase 2: Offscreen<GlesTexture>::create_buffer(Fourcc::Abgr8888, size), OutputDamageTracker::render_output, ExportMem::copy_framebuffer(.., Fourcc::Abgr8888), map_texture (y-flipped: GlesMapping::flipped()==true), then crop/downscale/readback via adesk-render")
}

/// pixman path for [`HeadlessRenderer::render_window`].
///
/// Phase 2: `Offscreen<Image>::create_buffer(Fourcc::Xrgb8888, size)`, render
/// the surface tree, `ExportMem::copy_framebuffer(.., Fourcc::Xrgb8888)`, then
/// readback through `adesk-render`.
fn render_window_pixman(
    _renderer: &mut PixmanRenderer,
    _surface: &WlSurface,
    _geometry: Rect,
    _region: Option<Rect>,
    _max_dimension: Option<u32>,
) -> crate::Result<RenderedFrame> {
    todo!("Phase 2: Offscreen<Image>::create_buffer(Fourcc::Xrgb8888, size), render, ExportMem::copy_framebuffer(.., Fourcc::Xrgb8888), readback via adesk-render")
}

/// GL path for [`HeadlessRenderer::render_output`].
///
/// Phase 2: compose the windows (and popups) plus overlays into render elements
/// and push them through one `OutputDamageTracker` sized to the virtual output.
fn render_output_gl(
    _renderer: &mut GlesRenderer,
    _windows: &[OutputWindow],
    _overlays: &[OverlayKind],
    _region: Option<Rect>,
    _max_dimension: Option<u32>,
) -> crate::Result<RenderedFrame> {
    todo!("Phase 2: compose windows + overlays through OutputDamageTracker")
}

/// pixman path for [`HeadlessRenderer::render_output`].
///
/// Phase 2: same composition as the GL path, but into an `Offscreen<Image>`
/// target (the software path must fully render SHM-backed windows; an
/// unsupported DMA-BUF must surface as `render_failed`, never a panic).
fn render_output_pixman(
    _renderer: &mut PixmanRenderer,
    _windows: &[OutputWindow],
    _overlays: &[OverlayKind],
    _region: Option<Rect>,
    _max_dimension: Option<u32>,
) -> crate::Result<RenderedFrame> {
    todo!("Phase 2: compose windows + overlays through OutputDamageTracker")
}
