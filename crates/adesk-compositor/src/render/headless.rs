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
//! falls back to pixman with a warning. `Auto` *also* falls back when the GL it
//! gets is a **software** rasterizer (Mesa `llvmpipe`/`swrast`/...): software GL
//! succeeds at surfaceless EGL setup, so the failure arm alone would never fire,
//! yet it can segfault at raster time on a DMA-BUF import failure. The detected
//! rasterizer is logged; an explicitly requested `Gl` is still honoured but
//! marked. Only a **hardware** `Gl` renderer passes
//! [`imports_dmabuf`](HeadlessRenderer::imports_dmabuf), so both the pixman
//! fallback and software GL advertise no DMA-BUF global. The result is reported
//! by `ping`.

use std::ffi::{c_char, CStr};

use adesk_core::{Rect, Size};
use adesk_render::{render_scene, RenderConfig, RenderError, Scene, TargetPool};
use smithay::{
    backend::{
        allocator::{format::FormatSet, Format},
        egl::{native::EGLSurfacelessDisplay, EGLContext, EGLDisplay},
        renderer::{
            element::RenderElement,
            gles::{ffi, GlesRenderbuffer, GlesRenderer},
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
/// [`dmabuf_formats`](HeadlessRenderer::dmabuf_formats) and
/// [`imports_dmabuf`](HeadlessRenderer::imports_dmabuf), which ask the concrete
/// backend.
pub(crate) enum HeadlessRenderer {
    /// EGL/GLES renderer built on a surfaceless EGL display (boxed: it is ~6 KiB),
    /// with its offscreen-target reuse pool.
    Gl {
        /// The GLES renderer.
        renderer: Box<GlesRenderer>,
        /// Whether the GLES renderer is a **software** rasterizer (`llvmpipe`, ...).
        ///
        /// Only `true` for an explicitly requested [`RendererKind::Gl`]; [`RendererKind::Auto`]
        /// resolves a software GL to pixman instead of keeping it. Exposed through
        /// [`software_gl`](HeadlessRenderer::software_gl), it is the reason such a
        /// renderer fails [`imports_dmabuf`](HeadlessRenderer::imports_dmabuf) — so
        /// the `zwp_linux_dmabuf_v1` global is suppressed, and suppressed
        /// deliberately.
        software_gl: bool,
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
    ///   A **software** rasterizer (Mesa `llvmpipe`, ...) is honoured, but logged
    ///   loudly and marked via [`software_gl`](HeadlessRenderer::software_gl), which
    ///   suppresses the DMA-BUF global.
    /// * [`RendererKind::Auto`] — tries GL, and on *either* a GL creation failure
    ///   *or* a software rasterizer logs a warning and builds the pixman renderer.
    pub(crate) fn create(kind: RendererKind) -> crate::Result<HeadlessRenderer> {
        Self::create_with(kind, detect_software_gl)
    }

    /// [`create`](HeadlessRenderer::create) with an injectable GL-identity probe.
    ///
    /// The probe decides whether an already-created GL renderer is a software
    /// rasterizer; production passes [`detect_software_gl`]. The seam exists so the
    /// `Auto`→pixman and `Gl`-honoured-but-marked decisions are unit-testable
    /// independently of which rasterizer the test machine's EGL happens to expose.
    pub(crate) fn create_with(
        kind: RendererKind,
        is_software_gl: fn(&mut GlesRenderer) -> bool,
    ) -> crate::Result<HeadlessRenderer> {
        match kind {
            RendererKind::Pixman => Self::pixman(),
            RendererKind::Gl => {
                let mut renderer = create_gl()?;
                let software_gl = is_software_gl(&mut renderer);
                if software_gl {
                    tracing::warn!(
                        "GL renderer is a software rasterizer; `--renderer pixman` is \
                         recommended — software GL can crash on DMA-BUF clients"
                    );
                }
                Ok(HeadlessRenderer::Gl {
                    renderer: Box::new(renderer),
                    software_gl,
                    pool: TargetPool::new(),
                })
            }
            RendererKind::Auto => match create_gl() {
                Ok(mut renderer) => {
                    if is_software_gl(&mut renderer) {
                        tracing::warn!(
                            "GL renderer is a software rasterizer, falling back to pixman \
                             for stability (software GL can crash on DMA-BUF clients)"
                        );
                        Self::pixman()
                    } else {
                        Ok(HeadlessRenderer::Gl {
                            renderer: Box::new(renderer),
                            software_gl: false,
                            pool: TargetPool::new(),
                        })
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "GL renderer unavailable, falling back to pixman");
                    Self::pixman()
                }
            },
        }
    }

    /// The pixman software renderer with a fresh target pool.
    fn pixman() -> crate::Result<HeadlessRenderer> {
        Ok(HeadlessRenderer::Pixman {
            renderer: create_pixman()?,
            pool: TargetPool::new(),
        })
    }

    /// Whether the active renderer is a **software GL** rasterizer.
    ///
    /// Only an explicitly requested [`RendererKind::Gl`] can be software — an
    /// [`RendererKind::Auto`] selection resolves a software GL to pixman, and pixman
    /// itself is never "software GL". This is the marker
    /// [`imports_dmabuf`](HeadlessRenderer::imports_dmabuf) consults to exclude a
    /// software rasterizer; it is kept separate so the suppression log can
    /// distinguish "software GL, suppressed deliberately" from "pixman, cannot map a
    /// client buffer".
    pub(crate) fn software_gl(&self) -> bool {
        match self {
            HeadlessRenderer::Gl { software_gl, .. } => *software_gl,
            HeadlessRenderer::Pixman { .. } => false,
        }
    }

    /// Whether the active renderer can import client DMA-BUF buffers.
    ///
    /// Positive capability test [`State::new`](crate::state::State::new) gates the
    /// `zwp_linux_dmabuf_v1` global on: `true` only for a **hardware**
    /// [`RendererKind::Gl`] renderer (`Gl { software_gl: false }`). pixman returns
    /// `false` — its `import_dmabuf` maps the client buffer and a GPU-less or
    /// DMA-BUF-restricted host denies the mapping — and so does a software GL
    /// rasterizer, which is not trusted with a DMA-BUF import. Advertising the global
    /// for either would let a client pick `create_immed`, whose failed import the
    /// protocol answers with a fatal error (Smithay posts `invalid_wl_buffer`),
    /// terminating the application; with the global suppressed clients attach
    /// `wl_shm` buffers instead.
    pub(crate) fn imports_dmabuf(&self) -> bool {
        matches!(
            self,
            HeadlessRenderer::Gl {
                software_gl: false,
                ..
            }
        )
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
    /// Only consulted when [`imports_dmabuf`](HeadlessRenderer::imports_dmabuf) is
    /// `true`; a renderer that cannot import client DMA-BUFs advertises no global at
    /// all (`state.rs`), so this list is never published for it. Both backends
    /// implement `ImportDma`: `GlesRenderer` reports the EGL dmabuf texture formats,
    /// `PixmanRenderer` a static set of single-plane linear formats it can map.
    /// `Format` is `smithay::backend::allocator::Format` (a `drm_fourcc::DrmFormat`:
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
            HeadlessRenderer::Gl { renderer, pool, .. } => {
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
            HeadlessRenderer::Gl { renderer, pool, .. } => {
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

/// Software GL rasterizer names, matched as case-insensitive substrings of
/// `GL_RENDERER`.
const SOFTWARE_RASTERIZERS: [&str; 4] = ["llvmpipe", "softpipe", "swrast", "lavapipe"];

/// Whether the GL renderer/vendor strings name a **software** rasterizer.
///
/// Software is trusted only when the vendor names Mesa *and* the renderer name
/// matches one of [`SOFTWARE_RASTERIZERS`]; both checks are case-insensitive. An
/// empty or unrecognized pair is never software — the safe default, so hardware GL
/// (even a vendor this crate has never heard of) keeps its DMA-BUF support.
fn is_software_rasterizer(renderer: &str, vendor: &str) -> bool {
    let renderer = renderer.to_ascii_lowercase();
    let vendor = vendor.to_ascii_lowercase();
    vendor.contains("mesa")
        && SOFTWARE_RASTERIZERS
            .iter()
            .any(|name| renderer.contains(name))
}

/// Probe a created GL renderer and report whether it is a software rasterizer.
///
/// `GlesRenderer::with_context` makes the renderer's EGL context current around the
/// GL-string read. A failed probe (no current context) is logged at `debug` and
/// reported as *not* software (the safe default); on success the identity is logged
/// at `info`.
fn detect_software_gl(renderer: &mut GlesRenderer) -> bool {
    let (gl_renderer, gl_vendor) = match renderer.with_context(gl_identity) {
        Ok(identity) => identity,
        Err(error) => {
            tracing::debug!(error = %error, "could not probe the GL renderer identity");
            return false;
        }
    };
    tracing::info!(renderer = %gl_renderer, vendor = %gl_vendor, "GL renderer identified");
    is_software_rasterizer(&gl_renderer, &gl_vendor)
}

/// Read `GL_RENDERER` and `GL_VENDOR` from the renderer's current GL context.
fn gl_identity(gl: &ffi::Gles2) -> (String, String) {
    (gl_string(gl, ffi::RENDERER), gl_string(gl, ffi::VENDOR))
}

/// Read one GL string (`glGetString`), yielding an empty `String` for a null or
/// non-UTF-8 result instead of ever dereferencing a null pointer.
fn gl_string(gl: &ffi::Gles2, name: ffi::types::GLenum) -> String {
    // SAFETY: `glGetString` returns a pointer to a NUL-terminated string owned by
    // GL, or null. The null case is checked before any dereference, and the pointer
    // is read only while the context is current (`GlesRenderer::with_context`).
    let ptr = unsafe { gl.GetString(name) } as *const c_char;
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: `ptr` is a non-null NUL-terminated C string as returned by
    // `glGetString`; `to_string_lossy` copies it, so nothing outlives the context.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
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

    /// Creates a renderer through the injectable-probe seam, skipping (with a
    /// message) when the environment has no EGL/GLES.
    fn create_with_or_skip(
        kind: RendererKind,
        probe: fn(&mut GlesRenderer) -> bool,
    ) -> Option<HeadlessRenderer> {
        if std::env::var("ADESK_TEST_GL").as_deref() != Ok("1") {
            eprintln!("skipping GL test: set ADESK_TEST_GL=1 to enable it");
            return None;
        }
        match HeadlessRenderer::create_with(kind, probe) {
            Ok(renderer) => Some(renderer),
            Err(error) => {
                eprintln!("skipping GL test: EGL/GLES unavailable: {error}");
                None
            }
        }
    }

    #[test]
    fn software_rasterizers_are_detected_case_insensitively() {
        for renderer in [
            "llvmpipe (LLVM 17.0.6, 256 bits)",
            "softpipe",
            "Mesa SWRAST",
            "Lavapipe",
        ] {
            assert!(is_software_rasterizer(renderer, "Mesa"), "{renderer}");
            assert!(
                is_software_rasterizer(renderer, "mesa project"),
                "{renderer}: the vendor check is case-insensitive too"
            );
        }
    }

    #[test]
    fn hardware_and_unknown_renderers_are_not_software() {
        for renderer in [
            "NVIDIA GeForce RTX 4090/PCIe/SSE2",
            "AMD Radeon RX 7900 XTX (radeonsi, navi31)",
            "Intel(R) UHD Graphics 630",
        ] {
            assert!(!is_software_rasterizer(renderer, "Mesa"), "{renderer}");
            assert!(
                !is_software_rasterizer(renderer, "NVIDIA Corporation"),
                "{renderer}"
            );
        }
        // Empty / unknown / non-Mesa strings are never software.
        assert!(!is_software_rasterizer("", ""));
        assert!(!is_software_rasterizer("", "Mesa"));
        assert!(!is_software_rasterizer("unknown", "unknown"));
        // A software name without the Mesa vendor is not trusted.
        assert!(!is_software_rasterizer("llvmpipe", ""));
    }

    #[test]
    fn auto_falls_back_to_pixman_for_a_software_gl() {
        // The probe is injected, so the decision is deterministic on any machine:
        // GL creation is real (hence the EGL gate) but the software verdict is
        // forced, so this never depends on the test machine's rasterizer.
        let Some(renderer) = create_with_or_skip(RendererKind::Auto, |_| true) else {
            return;
        };
        assert_eq!(renderer.name(), RendererName::Pixman);
        assert!(!renderer.software_gl());
        assert!(
            !renderer.imports_dmabuf(),
            "the pixman fallback cannot import client DMA-BUFs, so it is SHM-only"
        );
    }

    #[test]
    fn auto_keeps_a_hardware_gl() {
        let Some(renderer) = create_with_or_skip(RendererKind::Auto, |_| false) else {
            return;
        };
        assert_eq!(renderer.name(), RendererName::Gl);
        assert!(!renderer.software_gl());
        assert!(
            renderer.imports_dmabuf(),
            "a hardware GL renderer advertises the dmabuf global"
        );
    }

    #[test]
    fn explicit_gl_is_honoured_but_marked_software() {
        let Some(renderer) = create_with_or_skip(RendererKind::Gl, |_| true) else {
            return;
        };
        assert_eq!(renderer.name(), RendererName::Gl);
        assert!(renderer.software_gl());
        assert!(
            !renderer.imports_dmabuf(),
            "software GL is not trusted with DMA-BUF imports"
        );
    }

    /// The positive capability predicate: pixman never imports client DMA-BUFs, so
    /// its clients stay on `wl_shm` and the dmabuf global is not advertised. Needs
    /// no EGL/GPU/display.
    #[test]
    fn pixman_does_not_import_dmabuf() {
        let renderer = pixman();
        assert_eq!(renderer.name(), RendererName::Pixman);
        assert!(!renderer.software_gl(), "pixman is not software *GL*");
        assert!(
            !renderer.imports_dmabuf(),
            "pixman maps the client buffer and a DMA-BUF-restricted host denies it"
        );
    }

    /// The GL half of the predicate, through the injectable probe (the EGL gate
    /// applies because `create_with` really builds the GL renderer).
    #[test]
    fn a_hardware_gl_imports_dmabuf_but_a_software_gl_does_not() {
        let Some(hardware) = create_with_or_skip(RendererKind::Gl, |_| false) else {
            return;
        };
        assert!(
            hardware.imports_dmabuf(),
            "a hardware GL renderer imports client DMA-BUFs"
        );
        let Some(software) = create_with_or_skip(RendererKind::Gl, |_| true) else {
            return;
        };
        assert!(
            !software.imports_dmabuf(),
            "a software GL rasterizer is not trusted with DMA-BUF imports"
        );
    }

    #[test]
    fn auto_resolves_a_real_software_gl_to_pixman() {
        // Cannot fail spuriously: skips when the environment has no EGL and also
        // when the probed rasterizer is real hardware GL.
        if std::env::var("ADESK_TEST_GL").as_deref() != Ok("1") {
            eprintln!("skipping GL test: set ADESK_TEST_GL=1 to enable it");
            return;
        }
        let mut gl_renderer = match create_gl() {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GL test: EGL/GLES unavailable: {error}");
                return;
            }
        };
        if !detect_software_gl(&mut gl_renderer) {
            eprintln!("skipping: this machine exposes hardware GL, not a software rasterizer");
            return;
        }
        drop(gl_renderer);
        let resolved = HeadlessRenderer::create(RendererKind::Auto)
            .expect("pixman must be creatable after a software-GL probe");
        assert_eq!(resolved.name(), RendererName::Pixman);
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
