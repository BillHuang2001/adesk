# adesk-render — offscreen render pipeline

## Intent

`adesk-render` owns everything between "a window's surface tree exists" and "here is an `ImageBuffer` plus damage evidence": scene description, offscreen target, read-back, crop/downscale, PNG encoding and damage coalescing.
It implements `docs/architecture.md` §5 and serves `docs/protocol.md` §5.4 (`capture_window`, `capture_region`, `observe(include_image)`, `inspect_capture`).
The concrete renderer (`GlesRenderer` over surfaceless EGL, or `PixmanRenderer`) is **created and owned by `adesk-compositor`**; this crate is generic over Smithay 0.7's renderer traits and must never depend on `adesk-compositor`.
Rendering is on demand only: no frame loop, no continuous composition, no periodic readback.
The crate contains no unsafe code, never panics on a renderer/import/encoding failure, and its pure half (image ops, damage math) works fully headless.

## API Surface

All items are re-exported flat at the crate root; the modules are `pub` as well.

### config (`src/config.rs`)
- `RenderConfig { source: Rect, crop: Option<Rect>, max_dimension: Option<u32>, clear_color: [u8; 4] }` + `new(source)`, `with_crop`, `with_max_dimension`, `with_clear_color`, `target_size() -> Size`, `output_size() -> Size`, `validate() -> Result<()>`.
  - `source` is the scene-coordinate rect rendered into the target (its size is the target size, its origin maps to target pixel `(0,0)`); `crop` is in the same coordinate space and must be contained in `source`.
- `TARGET_FORMAT: Fourcc = Abgr8888`, `READBACK_FORMAT: Fourcc = Abgr8888`, `DEFAULT_CLEAR_COLOR: [u8; 4] = [0,0,0,255]`.

### scene (`src/scene.rs`)
- `SceneNode<E> { element: E, location: Point<i32, Physical>, damage: Region }` + `new`, `with_damage`, `element()`, `element_mut()`, `location()`, `set_location`, `damage()`, `set_damage`, `into_element()`.
  - `location` is authoritative placement in scene coordinates; empty `damage` means "unknown → redraw the whole element".
- `Scene<E> { commit_seq: u64, nodes: Vec<SceneNode<E>> }` + `new(commit_seq)`, `push`, `extend`, `nodes()`, `commit_seq()`, `len`, `is_empty`, `damage() -> Region`.
  - Nodes are bottom-to-top z-order; `E` is the compositor's own render element type (e.g. `WaylandSurfaceRenderElement<R>`); no trait objects, no lifetime plumbing.

### output (`src/output.rs`)
- `OffscreenTarget<T> { target: T, size: Size<i32, Buffer>, format: Fourcc }` + `new`, `size()`, `core_size()`, `format()`, `texture()`, `texture_mut()`, `into_inner()`; `T` is the renderer's own target type.
- `RenderedFrame { image: ImageBuffer, commit_seq: u64, damage: Region }` + `new`, `size()`; `damage` stays in scene coordinates, independent of crop/downscale.
- `create_target<R: Offscreen<T>, T>(&mut R, size: Size) -> Result<OffscreenTarget<T>>` — allocates `TARGET_FORMAT`; a zero-sized request fails with a structured error before any backend call.

### pipeline (`src/pipeline.rs`)
- `render_scene<R, T, E>(&mut R, &mut OffscreenTarget<T>, &Scene<E>, &RenderConfig) -> Result<RenderedFrame> where R: Renderer + Bind<T> + ExportMem, R::Error: Send + Sync + 'static, E: RenderElement<R>`.
  - Requires `target.core_size() == config.target_size()` (`InvalidConfig` otherwise — readback/crop math assume target == source size); draws every node intersecting the target over its full visible rect (the target is cleared first, so node damage never clips drawing); waits on the `finish()` sync point before readback; consumes the readback as top-down scene order on every backend.
- `import_buffer<R: ImportAll>(&mut R, &WlBuffer, Option<&SurfaceData>, &[Rectangle<i32, Buffer>]) -> Result<R::TextureId> where R::Error: Send + Sync + 'static`.

### image (`src/image.rs`) — pure, implemented, tested
- `crop(&ImageBuffer, Rect) -> ImageBuffer` (clamps; disjoint/empty → `0x0`).
- `downscale(&ImageBuffer, max_dimension: u32) -> ImageBuffer` (integer-boundary box filter, half-up rounding; `0` or already-fitting → copy).
- `fit_dimensions(Size, max_dimension: u32) -> Size` (aspect-preserving, never upscales).
- `encode_png(&ImageBuffer) -> Result<Vec<u8>>` (bytes, not an `ImageBuffer`; base64 is `adesk-proto`'s job; re-packs padded strides; empty images error).
- `image_from_readback(&[u8], width, height, stride, flipped) -> Result<ImageBuffer>` (normalizes stride; `flipped` is the caller's statement about row order — `true` reverses rows. The pipeline passes `false`: both backends' raw readback is already top-down in scene space).

### damage (`src/damage.rs`) — pure, implemented, tested
- `coalesce_damage(&Region, &Rect, min_area: u32) -> Region` (clip → coalesce → drop small).
- `DamageAccumulator { bounds, pending, commits, last_commit_seq }` + `new(bounds)`, `bounds()`, `set_bounds` (re-clips pending), `record_commit(commit_seq, &Region)`, `record_rect(Rect)`, `peek()`, `take()` (coalesced + reset pending, keeps lifetime counters), `clear()`, `is_empty()`, `commits()`, `last_commit_seq()`.

### error (`src/error.rs`)
- `RenderError` (10 variants) + `code() -> ErrorCode` + `impl From<RenderError> for adesk_core::Error`; `pub type Result<T>`.
- Mapping: `TargetCreation`/`TargetBind`/`RenderFailed`/`Readback`/`UnsupportedFormat`/`ImportFailed`/`UnsupportedBuffer` → `render_failed`; `InvalidConfig`/`InvalidImage` → `invalid_request`; `Encode` → `capture_failed`.

## Constraints

- Dependencies come only from root `[workspace.dependencies]`: `adesk-core`, `smithay` (features `wayland_frontend,desktop,renderer_pixman,renderer_glow`), `image` (png), `thiserror`.
- Never depend on `adesk-compositor`, `adesk-wm`, `adesk-observer` or any I/O crate; never construct a renderer, an EGL display or a wayland display here.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; no `unwrap`/`expect`/panics on request paths — every failure is a `RenderError`.
- Coordinates: scene coordinates are window-relative pixels; `RenderConfig::source` maps to target `(0,0)`; `RenderedFrame::damage` is scene-relative.
- Files stay well under the ~1000-line threshold; split along module boundaries.
- Public API surface is exactly what this file documents; keep internals private.
- Renderer errors are boxed; the extra `R::Error: Send + Sync + 'static` bound is required because Smithay only guarantees `std::error::Error` — pinned by `tests/renderer_bounds.rs` (absent on `create_target`, see Design Decisions).

## Routing Table

| Area | Owner |
|---|---|
| `RenderConfig`, target/readback formats, clear color | `./src/config.rs` |
| `Scene`, `SceneNode`, scene damage | `./src/scene.rs` |
| `OffscreenTarget`, `RenderedFrame`, `create_target` | `./src/output.rs` |
| `render_scene`, `import_buffer` (renderer-generic entry points) | `./src/pipeline.rs` |
| `crop`, `downscale`, `fit_dimensions`, `encode_png`, readback conversion | `./src/image.rs` |
| `DamageAccumulator`, `coalesce_damage` | `./src/damage.rs` |
| `RenderError`, AGP `ErrorCode` mapping | `./src/error.rs` |
| Exact-pixel tests for image ops | `./tests/image_ops.rs` |
| Damage coalescing/accumulator tests | `./tests/damage.rs` |
| Smithay trait-bound pinning | `./tests/renderer_bounds.rs` |
| Pixman/GL integration tests, exact-pixel scene rendering | `./tests/render_backend.rs` |

## Design Decisions

### Smithay 0.7 facts this crate relies on (vendor: `~/.cargo/registry/src/*/smithay-0.7.0`)

- `RendererSuper { type Error; type TextureId: Texture; type Framebuffer<'buffer>: Texture; type Frame<'frame,'buffer>: Frame }`; `Renderer::render<'frame,'buffer>(&'frame mut self, framebuffer: &'frame mut Self::Framebuffer<'buffer>, output_size: Size<i32, Physical>, dst_transform: Transform) -> Result<Self::Frame<'frame,'buffer>, Self::Error>`.
- `RendererSuper::Error: std::error::Error` only — **not** `Send + Sync + 'static`; boxing renderer errors therefore needs an explicit bound.
- `Bind<Target>::bind<'a>(&mut self, target: &'a mut Target) -> Result<Self::Framebuffer<'a>, Self::Error>` — the framebuffer borrows the **target**, not the renderer (so `render`/`copy_framebuffer` can take `&mut self` afterwards), but it blocks further `target` access: cache `target.size()` before binding.
- `Offscreen<Target>: Renderer + Bind<Target>` with `create_buffer(&mut self, format: Fourcc, size: Size<i32, Buffer>) -> Result<Target, Self::Error>`.
- `ExportMem { type TextureMapping: TextureMapping; fn copy_framebuffer(&mut self, target: &Self::Framebuffer<'_>, region: Rectangle<i32, Buffer>, format: Fourcc) -> Result<Self::TextureMapping, Self::Error>; fn map_texture<'a>(&'a mut self, &'a Self::TextureMapping) -> Result<&'a [u8], Self::Error> }` — **`copy_framebuffer` takes an explicit `Fourcc` in 0.7** (0.6 did not).
- `Frame::clear(color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error>`, `Frame::draw_solid(dst, damage, Color32F)`, `Frame::finish(self) -> Result<SyncPoint, Self::Error>` (`SyncPoint` is `#[must_use]`); `Color32F: From<[f32; 4]>` only. `Renderer::wait(&SyncPoint)` blocks on the fence (no-op for pixman); required before `copy_framebuffer` because GL readback has no implicit sync.
- `Texture { width, height, size() -> Size<i32, Buffer>, format() -> Option<Fourcc> }`; `TextureMapping::flipped() -> bool` (`true` for `GlesMapping`, `false` for `PixmanMapping`) is renderer-**native**-relative ("flipped compared to the lower left being (0,0)"), not a canonical orientation signal: raw readback rows from both backends are top-down in scene space (GL `render` applies `flip180` to `transform.matrix()`, so scene `y = 0` → framebuffer row 0 → first readback row). Consuming the flag double-flips GL.
- `ImportAll::import_buffer(&mut self, &wl_buffer::WlBuffer, Option<&SurfaceData>, &[Rectangle<i32, Buffer>]) -> Option<Result<Self::TextureId, Self::Error>>` — `None` means "no texture representation" (unknown buffer type / single-pixel buffer).
- `RenderElement<R: Renderer>: Element { fn draw(&self, frame: &mut R::Frame<'_,'_>, src: Rectangle<f64, Buffer>, dst: Rectangle<i32, Physical>, damage: &[Rectangle<i32, Physical>], opaque_regions: &[Rectangle<i32, Physical>]) -> Result<(), R::Error> }` — **0.7 has no `cache: Option<&CommitCounter>` parameter** (0.6 did).
- `Element` supplies `id()`, `current_commit()`, `location(scale)`, `src()`, `transform()`, `geometry(scale) -> Rectangle<i32, Physical>` (already includes the element location), `damage_since(scale, Option<CommitCounter>) -> DamageSet<i32, Physical>` (rects in element-local coordinates), `opaque_regions(scale)`, `alpha()`, `kind()`.
- Element draw loop as implemented by `OutputDamageTracker::render_output` (`src/backend/renderer/damage/mod.rs`): elements are given **front-to-back (topmost first)** — its own doc comment says so — and drawn with `render_elements.iter().rev()` (back-to-front, painter's algorithm); `dst = element.geometry(scale)`; `src = element.src()`; per-element damage = output damage ∩ geometry minus the opaque regions of elements above, then translated back to element-local (`d.loc -= geometry.loc`); opaque regions translated the same way; `frame.clear(clear_color, &damage_rects)` runs before drawing; elements with empty damage are skipped. A `Scene` is bottom-to-top, so `adesk-compositor` must `.iter().rev()` if it feeds `scene.nodes()` into the tracker.
- `OutputDamageTracker::new(size: impl Into<Size<i32, Physical>>, scale: impl Into<Scale<f64>>, transform: Transform)`; `render_output<E, R>(&mut self, renderer: &mut R, framebuffer: &mut R::Framebuffer<'_>, age: usize, elements: &[E], clear_color: impl Into<Color32F>) -> Result<RenderOutputResult<'_>, damage::Error<R::Error>>` — it tracks per-element commit state and can skip the whole render when nothing changed; `adesk-compositor` may use it, this crate does not (our damage model is window-scoped and renderer-independent; the tracker also ignores `SceneNode::location`).
- Pixman: `PixmanRenderer::new() -> Result<PixmanRenderer, PixmanError>`; `Offscreen<pixman::Image<'static,'static>>` + `Bind<pixman::Image<'static,'static>>` (the type is reachable as `smithay::reexports::pixman::Image`); `Abgr8888` is in its `SUPPORTED_FORMATS`; readback mapping is top-down and tightly packed.
- GL: `EGLDisplay::new(native)` (unsafe) → `EGLContext::new(&display)` → `GlesRenderer::new(context)` (unsafe); headless native display is `EGLSurfacelessDisplay` (EGL_MESA_platform_surfaceless), `EGLDevice` for DRM nodes; offscreen targets are `GlesRenderbuffer` (needs `Capability::Renderbuffer`) or `GlesTexture`; `Bind`/`Offscreen`/`ExportMem` are implemented for `GlesRenderbuffer`; GL readback is bottom-up **in GL window coordinates** — which is exactly scene top-down after the `flip180` projection (see `TextureMapping::flipped` above).
- Both renderers implement `ImportMemWl` + `ImportDmaWl`, hence `ImportAll`, via the blanket impl `impl<R: Renderer + ImportMemWl + ImportDmaWl> ImportAll for R` — the `ImportEgl`-requiring variant is gated on `backend_egl` + `use_system_lib`, which this crate's feature set (`wayland_frontend,desktop,renderer_pixman,renderer_glow`) does not enable — pinned by `tests/renderer_bounds.rs`.
- `GlesRenderer` and `GlesRenderbuffer` are **not** `Send` (raw GL pointers, `Rc`, `mpsc::Receiver`); `PixmanRenderer` is `Send`.
- Import paths: `smithay::backend::allocator::Fourcc` (= `drm_fourcc::DrmFourcc`), `smithay::backend::renderer::{Bind, ExportMem, Frame, ImportAll, Offscreen, Renderer, Texture, TextureFilter, TextureMapping}`, `smithay::backend::renderer::element::{Element, RenderElement, AsRenderElements}`, `smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform}`, `smithay::wayland::compositor::SurfaceData`, `smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer`, `smithay::reexports::pixman`.

### Crate decisions

- `TARGET_FORMAT = READBACK_FORMAT = Fourcc::Abgr8888`: DRM fourcc whose memory byte order is `R,G,B,A` on little-endian, i.e. exactly `PixelFormat::Rgba8`; GLES maps it to `GL_RGBA`/`GL_UNSIGNED_BYTE`, pixman to `A8B8G8R8`. Readback therefore never needs a channel swizzle.
- The scene is generic over the element type (`Scene<E>`) rather than boxing `dyn RenderElement<R>`: no object-safety or lifetime friction, and it matches Smithay's own `render_output<E, R>` style.
- `SceneNode::location` is authoritative for placement (the pipeline ignores the element's own `location(scale)` when computing `dst`), so the compositor can translate a whole scene without rebuilding elements; size still comes from `element.geometry(scale)`.
- Every node that intersects the target is drawn over its **full visible rect** (damage passed to `draw` is element-local). The target is cleared first, so clipping a draw to `SceneNode::damage` would punch clear-color holes into an otherwise complete frame; `SceneNode::damage` is *evidence*, reported via `RenderedFrame::damage` (`scene.damage().clip(source)`), never a draw clip.
- Readback is consumed as top-down scene order on every backend: `TextureMapping::flipped()` is deliberately ignored. Both backends' raw rows are byte-identical and top-down in scene space — GL's projection applies `flip180` (scene `y = 0` → framebuffer row 0 → first readback row); `flipped()` is renderer-native-relative (GL origin lower-left), so un-flipping would mirror every GL capture. Smithay's own readback consumers (`examples/buffer_test.rs`, `multigpu`) write the mapped bytes verbatim. Pinned by the `ADESK_TEST_GL=1` test asserting exact equality with the pixman reference.
- `RenderedFrame::damage` is the scene-level damage (`Scene::damage()` clipped to `source`) in window/scene coordinates — never crop- or downscale-adjusted, because `changed_regions` is defined window-relative in `docs/protocol.md` §5.4.
- `crop` clamps to the image and returns an empty `0x0` image for disjoint rects (never an error); `RenderConfig::validate` is the strict check for protocol-supplied rects (crop must be contained in source, source non-empty).
- `downscale` uses integer box boundaries `[dx*w/dw, (dx+1)*w/dw)` (every source pixel contributes to exactly one output pixel, no fractional weights); each channel is averaged as `(sum + count/2) / count` (integer division = half-up).
- `fit_dimensions(size, M)` with `longest = max(w, h)` returns `size` unchanged when `M == 0`, a dimension is 0, or `longest <= M`; otherwise each edge is `clamp((value*M + longest/2) / longest, 1, value)` — one shared scale `M/longest`, per-edge half-up integer rounding, never upscales, both edges ≥ 1 (`64x1` with `M = 16` → `16x1`; `M = 1` → `1x1`), so aspect ratio is only approximate for degenerate edges. No `max_dimension` value produces a `RenderError`.
- `encode_png` returns raw PNG bytes (`Vec<u8>`): `ImagePayload.data` is base64, which is `adesk-proto`'s concern; the function re-packs padded strides so any valid `ImageBuffer` encodes correctly.
- `image_from_readback` produces a tightly packed `ImageBuffer` (so `ImageBuffer::from_rgba`, which rejects padded strides, always succeeds); its `flipped` parameter is the caller's row-order statement, not a renderer signal.
- `DamageAccumulator` is a self-contained, per-window clipped damage set: `take()` resets pending damage but keeps lifetime `commits`/`last_commit_seq` counters. No production caller wires it in yet (see Known Issues) — the compositor reports damage through `Region::simplified()` directly.
- `RenderError::code()` is the single place that maps pipeline failures to AGP codes; DMA-BUF import failures on the software path become `ImportFailed` → `render_failed` as `docs/architecture.md` §5 requires.
- `create_target`'s frozen signature has no `R::Error: Send + Sync + 'static` bound (Smithay only guarantees `std::error::Error`), so `TargetCreation.source` wraps the renderer error text in a private `RendererErrorText` (`Display + Error`); the typed downcast/source chain is lost. API gap: adding the bound would restore typed sources.
- The renderer-backed entry points (`create_target`, `render_scene`, `import_buffer`) are implemented; `render_scene`'s step-by-step contract is documented on the function itself.

## Test Strategy

- `./tests/image_ops.rs` (16 tests): exact-pixel `crop` (copy, clamp, disjoint, empty), `downscale` (box averages, half-up rounding, no-op, aspect ratio, non-integer ratios), `fit_dimensions`, PNG round-trip through the `image` decoder (tight and padded strides), empty-image encode error, `image_from_readback` (tight, padded, flipped, malformed).
- `./tests/damage.rs` (11 tests): overlap/adjacency merging, disjoint preservation, clipping, `min_area` filtering, accumulator record/take/peek/clear/set_bounds/record_rect, counter behavior.
- `./tests/renderer_bounds.rs` (2 tests): compile-time assertions that `GlesRenderer`/`PixmanRenderer` satisfy `Renderer + Bind<T> + ExportMem + Offscreen<T> + ImportAll` for their real target types and that `GlesError`/`PixmanError` are `Send + Sync + 'static`.
- `./tests/render_backend.rs` (12 tests): pixman-backed integration tests using a local recording `Element`/`RenderElement` double — clear color, canonical exact pixels, z-order, `-source.loc` translation, element-local full-rect damage despite partial `SceneNode::damage`, scene-coordinate crop translation, crop-before-downscale order, `RenderedFrame::damage`/`commit_seq` passthrough, off-source nodes skipped, target size/format, 0x0 `create_target` error, invalid config. One GL test runs only with `ADESK_TEST_GL=1` (surfaceless EGL + `GlesRenderer`, skips cleanly when EGL is unavailable) and asserts **exact image equality** with the pixman reference — the orientation regression guard (verified under Mesa llvmpipe).
- `src/config.rs` `#[cfg(test)]` (13 tests): `validate`, `target_size`, `output_size` (crop, aspect-preserving `max_dimension`, no upscale, `0` disabled), defaults/formats.
- No test needs a display, GPU, network or installed application. Run: `bash scripts/dev.sh cargo test -p adesk-render` (the wrapper is a bash script; bare `cargo` cannot link outside the dev shell). GL: `ADESK_TEST_GL=1 ./scripts/dev.sh cargo test -p adesk-render --test render_backend`.
- `import_buffer` currently has no caller at all (no production or test reference anywhere in the workspace; a direct test would also need a live wayland connection). End-to-end coverage stays in `adesk-compositor/tests` and `adesk-server/tests` via `adesk-testkit`.

## Status

- Implemented; zero `todo!()` in the crate. `./scripts/dev.sh cargo test -p adesk-render` passes 54 tests, 0 failures, 0 ignored; `cargo check -p adesk-render --all-targets`, `cargo clippy -p adesk-render --all-targets --no-deps -- -D warnings`, `cargo doc -p adesk-render --no-deps --document-private-items` (no warnings) and `cargo fmt -p adesk-render --check` are clean.
- `create_target`, `render_scene` and `import_buffer` are all backed by real implementations; the pixman path is verified headless, the GL path under `ADESK_TEST_GL=1` (Mesa llvmpipe) with exact image equality against the software reference.
- Known gaps: `create_target` error boxing is text-only (signature gap above); `import_buffer` has no direct test without a live wayland connection.

## Known Issues

- `import_buffer` (`src/pipeline.rs`) is unreferenced: no production or test caller anywhere in the workspace (the compositor imports buffers through Smithay's surface-tree walk instead). Its `UnsupportedBuffer`/`ImportFailed` results are therefore never produced by real code.
- `DamageAccumulator` and `coalesce_damage` (`src/damage.rs`) are referenced only by `tests/damage.rs`, never by `adesk-compositor`/`adesk-server`.
- `RenderError::UnsupportedFormat` is never constructed (it is only listed in `code()`); `RenderError::UnsupportedBuffer`/`ImportFailed` are constructed only inside the dead `import_buffer`.
- Unreferenced `pub` accessors: `SceneNode::{element_mut, set_location, set_damage, into_element}`, `Scene::extend`, `OffscreenTarget::{texture, into_inner}`, `RenderConfig::with_clear_color` (tests only). `encode_png` is used by this crate's tests only.
- `adesk-server/src/images.rs` re-implements PNG encoding and stride repacking that duplicate `src/image.rs` (`encode_png` + private `tight_rgba`); it does not call `adesk_render::encode_png`.

## Notes for Agents

- `render_scene` requires `target.core_size() == config.target_size()`; callers should create the target from `config.target_size()`.
- `render_scene` has no "nothing to render" path: an empty `Scene` (zero nodes) or nodes with empty damage still renders `Ok` as a full-size frame of the clear color (test `pixman_empty_scene_renders_clear_color`).
  Only genuine backend/import/readback failures are `Err`; callers that want "no content" to be an error must check `Scene::is_empty()` themselves.
- A validated `RenderConfig` guarantees a non-empty frame: `render_scene` never returns a `0x0` image (`validate` rejects empty `source`/`crop` and crop non-containment); the standalone pure `crop` helper's `0x0` output (disjoint/empty rect) is rejected by `encode_png` as `InvalidImage`.
- `RenderError::UnsupportedFormat` is mapped to `render_failed` but no code path in this crate constructs it.
- `render_scene` returns `RenderedFrame::damage` as raw clipped rects (no coalescing); `adesk-compositor` applies `Region::simplified()` before replying.
  The rects stay in scene/window coordinates even when `crop`/`max_dimension` shrink the image, so a cropped capture's `changed_regions` can reference coordinates outside the returned image.
- GL readback rows are already top-down in scene space; do NOT feed `TextureMapping::flipped()` into `image_from_readback` in this pipeline — it would mirror every GL capture (see Design Decisions).
- `copy_framebuffer`'s `Fourcc` argument is new in 0.7; `RenderElement::draw` lost its `cache` argument in 0.7. Do not copy 0.6-era examples.
- `GlesRenderer` is not `Send` and every renderer call is compositor-thread-only (`docs/architecture.md` §1); this crate holds no global state and never spawns threads.
- `smithay::utils::Size<i32, Buffer>` (buffer coords) and `Size<i32, Physical>` are distinct types; at scale 1.0 with `Transform::Normal` their numeric values are equal, but conversions must be explicit.
- The `image` crate is used with `default-features = false, features = ["png"]`; only PNG encode/decode (and the test-only decoder) are available.
- Integration-test crates do NOT inherit `#![forbid(unsafe_code)]` from `lib.rs`, so `tests/render_backend.rs` may use `unsafe {}` for surfaceless EGL setup; the library itself stays unsafe-free.

## Dependencies

- `adesk-core` — `ImageBuffer`, `Rect`, `Region`, `Size`, `ErrorCode`, `Error` (never fork these types).
- `smithay` 0.7 with `wayland_frontend`, `desktop`, `renderer_pixman`, `renderer_glow` (pulls `wayland-server`, `wayland-protocols`, `pixman`, `glow`, `gl_generator`, `drm-fourcc`).
- `image` 0.25 (png only), `thiserror` 2.
- System libraries (pixman, EGL/GLES, libwayland) come from the Nix dev shell; build through `./scripts/dev.sh`.
r` 2.
- System libraries (pixman, EGL/GLES, libwayland) come from the Nix dev shell; build through `./scripts/dev.sh`.
