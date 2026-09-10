# adesk-render — offscreen render pipeline

## Intent

`adesk-render` owns everything between "a window's surface tree exists" and "here is an `ImageBuffer` plus damage evidence": scene description, offscreen target, read-back, crop/downscale, PNG encoding and damage coalescing.
It implements `docs/architecture.md` §5 and serves `docs/protocol.md` §5.4 (`capture_window`, `capture_region`, `observe(include_image)`, `inspect_capture`).
The concrete renderer (`GlesRenderer` over surfaceless EGL, or `PixmanRenderer`) is **created and owned by `adesk-compositor`**; this crate is generic over Smithay 0.7's renderer traits and must never depend on `adesk-compositor`.
Rendering is on demand only: no frame loop, no continuous composition, no periodic readback.
A configured `crop` reads back only the requested sub-rectangle, so a small crop never pays for a full-frame readback plus a post-hoc image copy.
The crate contains no unsafe code, never panics on a renderer/import/encoding failure, and its pure half (image ops, damage math) works fully headless.

## API Surface

All items are re-exported flat at the crate root; the modules are `pub` as well.

### config (`src/config.rs`)
- `RenderConfig { source: Rect, crop: Option<Rect>, max_dimension: Option<u32>, clear_color: [u8; 4] }` + `new(source)`, `with_crop`, `with_max_dimension`, `target_size() -> Size`, `output_size() -> Size`, `validate() -> Result<()>`.
  - `clear_color` is a public field set directly (there is no builder for it); tests use struct-update syntax.
  - `source` is the scene-coordinate rect rendered into the target (its size is the target size, its origin maps to target pixel `(0,0)`); `crop` is in the same coordinate space and must be contained in `source`.
- `TARGET_FORMAT: Fourcc = Abgr8888`, `READBACK_FORMAT: Fourcc = Abgr8888`, `DEFAULT_CLEAR_COLOR: [u8; 4] = [0,0,0,255]`.

### scene (`src/scene.rs`)
- `SceneNode<E> { element: E, location: Point<i32, Physical>, damage: Region }` + `new`, `with_damage`, `element()`, `location()`, `damage()`.
  - `location` is authoritative placement in scene coordinates; empty `damage` means "unknown → redraw the whole element".
- `Scene<E> { commit_seq: u64, nodes: Vec<SceneNode<E>> }` + `new(commit_seq)`, `push`, `nodes()`, `commit_seq()`, `len`, `is_empty`, `damage() -> Region`.
  - Nodes are bottom-to-top z-order; `E` is the compositor's own render element type (e.g. `WaylandSurfaceRenderElement<R>`); no trait objects, no lifetime plumbing.
  - A scene is assembled once per render request; nodes are never mutated in place (the previously unused `element_mut`/`set_location`/`set_damage`/`into_element`/`Scene::extend` setters were removed as dead).

### output (`src/output.rs`)
- `OffscreenTarget<T> { target: T, size: Size<i32, Buffer>, format: Fourcc }` + `new`, `size()`, `core_size()`, `format()`, `texture_mut()`; `T` is the renderer's own target type.
  - `texture_mut()` exists because `render_scene` must call `renderer.bind(..)`; the immutable accessor and `into_inner()` were removed as dead.
- `RenderedFrame { image: ImageBuffer, commit_seq: u64, damage: Region }` + `new`, `size()`; `damage` stays in scene coordinates, independent of crop/downscale.
- `create_target<R: Offscreen<T>, T>(&mut R, size: Size) -> Result<OffscreenTarget<T>>` — allocates `TARGET_FORMAT`; a zero-sized request fails with a structured error before any backend call.

### pool (`src/pool.rs`)
- `TargetPool<T>` + `new()`, `acquire<R: Offscreen<T>>(&mut R, size: Size) -> Result<OffscreenTarget<T>>`, `release(OffscreenTarget<T>)` (`Default` too).
  - A bounded cache of offscreen targets keyed by **exact** `core_size()`: `acquire` returns a cached target of that size or allocates one through `create_target`; `release` returns a target for reuse.
  - Retains at most **one target per distinct size**, up to `MAX_POOLED_TARGETS` (4) sizes total; extra releases are dropped. A target is ~4 MiB at 1280×800, so the pool's footprint is bounded, not proportional to the number of captures.
  - Reuse is byte-identical to a fresh allocation because `render_scene` clears the whole target rectangle before drawing; a target whose render failed must NOT be released (its contents are undefined) — `adesk-compositor` drops it instead.
  - The pool is per-renderer state on the compositor thread (`adesk-compositor`'s `HeadlessRenderer` holds one per backend).

### pipeline (`src/pipeline.rs`)
- `render_scene<R, T, E>(&mut R, &mut OffscreenTarget<T>, &Scene<E>, &RenderConfig) -> Result<RenderedFrame> where R: Renderer + Bind<T> + ExportMem, R::Error: Send + Sync + 'static, E: RenderElement<R>`.
  - Requires `target.core_size() == config.target_size()` (`InvalidConfig` otherwise — readback/crop math assume target == source size); draws every node intersecting the target over its full visible rect (the target is cleared first, so node damage never clips drawing); waits on the `finish()` sync point before readback; consumes the readback as top-down scene order on every backend.
  - When `config.crop` is `Some`, `copy_framebuffer` is called with the crop translated into target/buffer coordinates (`crop.loc - source.loc`), so only that sub-rectangle is transferred; `crop == None` reads the whole target. Width/height come from the returned `TextureMapping` and the row stride from `map_texture`'s slice length — never assumed packed. `downscale` runs only when `max_dimension` is set.
  - Per-node `damage`/`opaque` scratch buffers are hoisted out of the draw loop and reused.

### image (`src/image.rs`) — pure, implemented, tested
- `crop(&ImageBuffer, Rect) -> ImageBuffer` (clamps; disjoint/empty → `0x0`; a whole-image crop of an already-tight buffer returns a plain clone).
- `downscale(&ImageBuffer, max_dimension: u32) -> ImageBuffer` (integer-boundary box filter, half-up rounding; `0`, degenerate or already-fitting → immediate copy).
- `fit_dimensions(Size, max_dimension: u32) -> Size` (aspect-preserving, never upscales).
- `encode_png(&ImageBuffer) -> Result<Vec<u8>>` (bytes, not an `ImageBuffer`; base64 is `adesk-proto`'s job; re-packs padded strides; an already-tight stride is passed to the encoder borrowed, with no copy; empty or truncated images error).
- `image_from_readback(&[u8], width, height, stride, flipped) -> Result<ImageBuffer>` (normalizes stride; tight + unflipped input is one `to_vec` of the payload, padded/flipped input one pre-sized allocation written in output order — never through a zero-filled buffer; `flipped` is the caller's statement about row order — `true` reverses rows. The pipeline passes `false`: both backends' raw readback is already top-down in scene space).

### damage (`src/damage.rs`) — pure, implemented, tested
- `coalesce_damage(&Region, &Rect, min_area: u32) -> Region` (clip → sort by `(y, x, h, w)` → coalesce in place → drop rects below `min_area`; holds two rect buffers, a third only when `min_area` actually drops something).
- `DamageAccumulator { bounds, pending, commits, last_commit_seq }` + `new(bounds)`, `bounds()`, `set_bounds` (re-clips pending), `record_commit(commit_seq, &Region)` (clips straight into `pending`, no throwaway `Region`), `record_rect(Rect)`, `peek()`, `take()` (coalesced + reset pending, keeps lifetime counters), `clear()`, `is_empty()`, `commits()`, `last_commit_seq()`.

### error (`src/error.rs`)
- `RenderError` (10 variants) + `code() -> ErrorCode` + `impl From<RenderError> for adesk_core::Error`; `pub type Result<T>`.
- Mapping: `TargetCreation`/`TargetBind`/`RenderFailed`/`Readback`/`UnsupportedFormat`/`ImportFailed`/`UnsupportedBuffer` → `render_failed`; `InvalidConfig`/`InvalidImage` → `invalid_request`; `Encode` → `capture_failed`.

## Constraints

- Dependencies come only from root `[workspace.dependencies]`: `adesk-core`, `smithay` (features `wayland_frontend,desktop,renderer_pixman,renderer_glow`), `image` (png), `thiserror`.
- Never depend on `adesk-compositor`, `adesk-wm`, `adesk-observer` or any I/O crate; never construct a renderer, an EGL display or a wayland display here.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; no `unwrap`/`expect`/panics on request paths — every failure is a `RenderError`.
- Coordinates: scene coordinates are window-relative pixels; `RenderConfig::source` maps to target `(0,0)`; `RenderedFrame::damage` is scene-relative.
- `crop`, `downscale`, `fit_dimensions`, `encode_png` and `image_from_readback` are consumed by sibling crates (`adesk-inspector/src/post.rs` and its tests, `adesk-server`); their signatures are frozen — optimize the bodies, never the types.
- Files stay well under the ~1000-line threshold; split along module boundaries.
- Public API surface is exactly what this file documents; keep internals private.
- Renderer errors are boxed; the extra `R::Error: Send + Sync + 'static` bound is required because Smithay only guarantees `std::error::Error` — pinned by `tests/renderer_bounds.rs` (absent on `create_target`, see Design Decisions).

## Routing Table

| Area | Owner |
|---|---|
| `RenderConfig`, target/readback formats, clear color | `./src/config.rs` |
| `Scene`, `SceneNode`, scene damage | `./src/scene.rs` |
| `OffscreenTarget`, `RenderedFrame`, `create_target` | `./src/output.rs` |
| `TargetPool` (bounded offscreen-target reuse) | `./src/pool.rs` |
| `render_scene` (renderer-generic entry point, sub-rect readback) | `./src/pipeline.rs` |
| `crop`, `downscale`, `fit_dimensions`, `encode_png`, readback conversion | `./src/image.rs` |
| `DamageAccumulator`, `coalesce_damage` | `./src/damage.rs` |
| `RenderError`, AGP `ErrorCode` mapping | `./src/error.rs` |
| Exact-pixel tests for image ops | `./tests/image_ops.rs` |
| Damage coalescing/accumulator tests | `./tests/damage.rs` |
| Smithay trait-bound pinning | `./tests/renderer_bounds.rs` |
| Pixman/GL integration tests, exact-pixel scene rendering | `./tests/render_backend.rs` |
| Sub-rect-readback equality proof (pixman + `ADESK_TEST_GL=1`) | `./tests/readback_crop.rs` |

## Design Decisions

### Smithay 0.7 facts this crate relies on (vendor: `~/.cargo/registry/src/*/smithay-0.7.0`)

- `RendererSuper { type Error; type TextureId: Texture; type Framebuffer<'buffer>: Texture; type Frame<'frame,'buffer>: Frame }`; `Renderer::render<'frame,'buffer>(&'frame mut self, framebuffer: &'frame mut Self::Framebuffer<'buffer>, output_size: Size<i32, Physical>, dst_transform: Transform) -> Result<Self::Frame<'frame,'buffer>, Self::Error>`.
- `RendererSuper::Error: std::error::Error` only — **not** `Send + Sync + 'static`; boxing renderer errors therefore needs an explicit bound.
- `Bind<Target>::bind<'a>(&mut self, target: &'a mut Target) -> Result<Self::Framebuffer<'a>, Self::Error>` — the framebuffer borrows the **target**, not the renderer (so `render`/`copy_framebuffer` can take `&mut self` afterwards), but it blocks further `target` access: cache `target.size()` before binding.
- `Offscreen<Target>: Renderer + Bind<Target>` with `create_buffer(&mut self, format: Fourcc, size: Size<i32, Buffer>) -> Result<Target, Self::Error>`.
- `ExportMem { type TextureMapping: TextureMapping; fn copy_framebuffer(&mut self, target: &Self::Framebuffer<'_>, region: Rectangle<i32, Buffer>, format: Fourcc) -> Result<Self::TextureMapping, Self::Error>; fn map_texture<'a>(&'a mut self, &'a Self::TextureMapping) -> Result<&'a [u8], Self::Error> }` — **`copy_framebuffer` takes an explicit `Fourcc` in 0.7** (0.6 did not) and accepts a **sub-region**, which is the basis of the cropped readback.
- Sub-region readback empirically yields a freshly copied, region-sized, tightly packed mapping on both backends (pixman composites the sub-rect into a new image, GL reads that region into a region-sized buffer), but the types do not promise packing — `render_scene` therefore takes width/height from `Texture::size(&mapping)` and the stride from `mapped_slice.len() / height`. Pinned by `tests/readback_crop.rs`.
- `Frame::clear(color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error>`, `Frame::draw_solid(dst, damage, Color32F)`, `Frame::finish(self) -> Result<SyncPoint, Self::Error>` (`SyncPoint` is `#[must_use]`); `Color32F: From<[f32; 4]>` only. `Renderer::wait(&SyncPoint)` blocks on the fence (no-op for pixman); required before `copy_framebuffer` because GL readback has no implicit sync.
- `Texture { width, height, size() -> Size<i32, Buffer>, format() -> Option<Fourcc> }`; `TextureMapping::flipped() -> bool` (`true` for `GlesMapping`, `false` for `PixmanMapping`) is renderer-**native**-relative ("flipped compared to the lower left being (0,0)"), not a canonical orientation signal: raw readback rows from both backends are top-down in scene space (GL `render` applies `flip180` to `transform.matrix()`, so scene `y = 0` → framebuffer row 0 → first readback row). Consuming the flag double-flips GL.
- `ImportAll::import_buffer(&mut self, &wl_buffer::WlBuffer, Option<&SurfaceData>, &[Rectangle<i32, Buffer>]) -> Option<Result<Self::TextureId, Self::Error>>` — `None` means "no texture representation" (unknown buffer type / single-pixel buffer). Both renderers satisfy `ImportAll` via the blanket `impl<R: Renderer + ImportMemWl + ImportDmaWl> ImportAll for R` (the `ImportEgl`-requiring variant is gated on `backend_egl` + `use_system_lib`, which this crate's feature set does not enable) — pinned by `tests/renderer_bounds.rs`. This crate no longer wraps it: `adesk-compositor` imports buffers itself through Smithay's surface-tree walk.
- `RenderElement<R: Renderer>: Element { fn draw(&self, frame: &mut R::Frame<'_,'_>, src: Rectangle<f64, Buffer>, dst: Rectangle<i32, Physical>, damage: &[Rectangle<i32, Physical>], opaque_regions: &[Rectangle<i32, Physical>]) -> Result<(), R::Error> }` — **0.7 has no `cache: Option<&CommitCounter>` parameter** (0.6 did).
- `Element` supplies `id()`, `current_commit()`, `location(scale)`, `src()`, `transform()`, `geometry(scale) -> Rectangle<i32, Physical>` (already includes the element location), `damage_since(scale, Option<CommitCounter>) -> DamageSet<i32, Physical>` (rects in element-local coordinates), `opaque_regions(scale)`, `alpha()`, `kind()`.
- Element draw loop as implemented by `OutputDamageTracker::render_output` (`src/backend/renderer/damage/mod.rs`): elements are given **front-to-back (topmost first)** — its own doc comment says so — and drawn with `render_elements.iter().rev()` (back-to-front, painter's algorithm); `dst = element.geometry(scale)`; `src = element.src()`; per-element damage = output damage ∩ geometry minus the opaque regions of elements above, then translated back to element-local (`d.loc -= geometry.loc`); opaque regions translated the same way; `frame.clear(clear_color, &damage_rects)` runs before drawing; elements with empty damage are skipped. A `Scene` is bottom-to-top, so `adesk-compositor` must `.iter().rev()` if it feeds `scene.nodes()` into the tracker.
- `OutputDamageTracker::new(size: impl Into<Size<i32, Physical>>, scale: impl Into<Scale<f64>>, transform: Transform)`; `render_output<E, R>(&mut self, renderer: &mut R, framebuffer: &mut R::Framebuffer<'_>, age: usize, elements: &[E], clear_color: impl Into<Color32F>) -> Result<RenderOutputResult<'_>, damage::Error<R::Error>>` — it tracks per-element commit state and can skip the whole render when nothing changed; `adesk-compositor` may use it, this crate does not (our damage model is window-scoped and renderer-independent; the tracker also ignores `SceneNode::location`).
- Pixman: `PixmanRenderer::new() -> Result<PixmanRenderer, PixmanError>`; `Offscreen<pixman::Image<'static,'static>>` + `Bind<pixman::Image<'static,'static>>` (the type is reachable as `smithay::reexports::pixman::Image`); `Abgr8888` is in its `SUPPORTED_FORMATS`; readback mapping is top-down and tightly packed.
- GL: `EGLDisplay::new(native)` (unsafe) → `EGLContext::new(&display)` → `GlesRenderer::new(context)` (unsafe); headless native display is `EGLSurfacelessDisplay` (EGL_MESA_platform_surfaceless), `EGLDevice` for DRM nodes; offscreen targets are `GlesRenderbuffer` (needs `Capability::Renderbuffer`) or `GlesTexture`; `Bind`/`Offscreen`/`ExportMem` are implemented for `GlesRenderbuffer`; GL readback is bottom-up **in GL window coordinates** — which is exactly scene top-down after the `flip180` projection (see `TextureMapping::flipped` above).
- `GlesRenderer` and `GlesRenderbuffer` are **not** `Send` (raw GL pointers, `Rc`, `mpsc::Receiver`); `PixmanRenderer` is `Send`.
- Import paths: `smithay::backend::allocator::Fourcc` (= `drm_fourcc::DrmFourcc`), `smithay::backend::renderer::{Bind, ExportMem, Frame, ImportAll, Offscreen, Renderer, Texture, TextureFilter, TextureMapping}`, `smithay::backend::renderer::element::{Element, RenderElement, AsRenderElements}`, `smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform}`, `smithay::wayland::compositor::SurfaceData`, `smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer`, `smithay::reexports::pixman`.

### Crate decisions

- `TARGET_FORMAT = READBACK_FORMAT = Fourcc::Abgr8888`: DRM fourcc whose memory byte order is `R,G,B,A` on little-endian, i.e. exactly `PixelFormat::Rgba8`; GLES maps it to `GL_RGBA`/`GL_UNSIGNED_BYTE`, pixman to `A8B8G8R8`. Readback therefore never needs a channel swizzle.
- The scene is generic over the element type (`Scene<E>`) rather than boxing `dyn RenderElement<R>`: no object-safety or lifetime friction, and it matches Smithay's own `render_output<E, R>` style.
- `SceneNode::location` is authoritative for placement (the pipeline ignores the element's own `location(scale)` when computing `dst`), so the compositor can translate a whole scene without rebuilding elements; size still comes from `element.geometry(scale)`.
- Every node that intersects the target is drawn over its **full visible rect** (damage passed to `draw` is element-local). The target is cleared first, so clipping a draw to `SceneNode::damage` would punch clear-color holes into an otherwise complete frame; `SceneNode::damage` is *evidence*, reported via `RenderedFrame::damage` (`scene.damage().clip(source)`), never a draw clip.
- **A `crop` is implemented as a sub-rectangle readback, not as a post-readback image crop**: `copy_framebuffer` takes the crop translated to target coordinates and the image is built directly at crop size. Proof: `crops()` in `tests/readback_crop.rs` renders the same scene twice — `RenderConfig::new(SOURCE).with_crop(c)` (sub-rect path) vs `RenderConfig::new(c)` with no crop (whole-frame path, itself unchanged) — and asserts byte-for-byte image equality, identical `commit_seq`, and that `damage` stays scene-relative in both; the same equality is asserted for GL against the pixman reference. `crop == None` still reads the whole target.
- `RenderedFrame::damage` is the scene-level damage (`Scene::damage()` clipped to `source`) in window/scene coordinates — never crop- or downscale-adjusted, because `changed_regions` is defined window-relative in `docs/protocol.md` §5.4. A crop translates the readback but never the damage evidence.
- Readback is consumed as top-down scene order on every backend: `TextureMapping::flipped()` is deliberately ignored. Both backends' raw rows are byte-identical and top-down in scene space — GL's projection applies `flip180` (scene `y = 0` → framebuffer row 0 → first readback row); `flipped()` is renderer-native-relative (GL origin lower-left), so un-flipping would mirror every GL capture. Smithay's own readback consumers (`examples/buffer_test.rs`, `multigpu`) write the mapped bytes verbatim. Pinned by the `ADESK_TEST_GL=1` tests asserting exact equality with the pixman reference.
- `crop` clamps to the image and returns an empty `0x0` image for disjoint rects (never an error); `RenderConfig::validate` is the strict check for protocol-supplied rects (crop must be contained in source, source non-empty).
- `downscale` uses integer box boundaries `[dx*w/dw, (dx+1)*w/dw)` (every source pixel contributes to exactly one output pixel, no fractional weights); each channel is averaged as `(sum + count/2) / count` (integer division = half-up).
- `fit_dimensions(size, M)` with `longest = max(w, h)` returns `size` unchanged when `M == 0`, a dimension is 0, or `longest <= M`; otherwise each edge is `clamp((value*M + longest/2) / longest, 1, value)` — one shared scale `M/longest`, per-edge half-up integer rounding, never upscales, both edges ≥ 1 (`64x1` with `M = 16` → `16x1`; `M = 1` → `1x1`), so aspect ratio is only approximate for degenerate edges. No `max_dimension` value produces a `RenderError`.
- `crop` and `downscale` short-circuit their no-op cases (whole-image tight crop; `max_dimension` `0`, degenerate image, or already fitting) and return `image.clone()` before any pixel loop — the single unavoidable copy for a `-> ImageBuffer` signature. A padded whole-image crop still goes through the repacking loop, so any result is tight.
- `encode_png` returns raw PNG bytes (`Vec<u8>`): `ImagePayload.data` is base64, which is `adesk-proto`'s concern. The private `tight_rgba` helper returns `Cow<'_, [u8]>`, so an already-tight stride feeds the encoder borrowed and only a padded stride allocates a repacked copy. It also validates `data.len() >= stride * height` (`checked_mul`) and reports `RenderError::InvalidImage` — `ImageBuffer`'s fields are public, so a hand-built buffer that lies about its stride/height must not panic the encode path.
- `image_from_readback` produces a tightly packed `ImageBuffer` (so `ImageBuffer::from_rgba`, which rejects padded strides, always succeeds); its `flipped` parameter is the caller's row-order statement, not a renderer signal. The tight/unflipped case is a single `to_vec` of the payload; padded or flipped input is written into one pre-sized allocation in output order (no zero-filled intermediate).
- `coalesce_damage` sorts the clipped rects by `(y, x, h, w)` *before* coalescing: a merge keeps the earlier rect's `(y, x)` and only grows its extent, so a sorted input coalesces to a sorted, deterministic result — exactly `clip(bounds).simplified()` without the intermediate `Region` or the clone inside `Region::simplified` (pinned by `coalesce_of_unsorted_input_matches_simplified`, which compares against `Region::simplified()` directly). `DamageAccumulator::pending` therefore stays a `Region` and `peek`/`take` need no in-place sort.
- `DamageAccumulator` is a self-contained, per-window clipped damage set: `record_commit` clips each rect straight into `pending` (skipping the throwaway `Region` that `Region::clip` used to build), `take()` resets pending damage but keeps lifetime `commits`/`last_commit_seq` counters. No production caller wires it in yet (see Known Issues).
- `RenderError::code()` is the single place that maps pipeline failures to AGP codes; `docs/architecture.md` §5 requires an unsupported DMA-BUF format on the software path to surface as a structured `render_failed` error, which is what the `ImportFailed`/`UnsupportedBuffer`/`UnsupportedFormat` arm exists for.
- `create_target`'s frozen signature has no `R::Error: Send + Sync + 'static` bound (Smithay only guarantees `std::error::Error`), so `TargetCreation.source` wraps the renderer error text in a private `RendererErrorText` (`Display + Error`); the typed downcast/source chain is lost. API gap: adding the bound would restore typed sources.
- `render_scene`'s step-by-step contract is documented on the function itself.
- **Target reuse is allocation-only, never semantic.** `TargetPool` caches offscreen targets keyed by exact `core_size()`; because `render_scene` clears the entire target rectangle before drawing, a reused target is byte-identical to a freshly allocated one, so no pixel, damage or event can change. A target whose render returned an error is dropped, never returned to the pool.

## Test Strategy

- `./tests/image_ops.rs` (21 tests): exact-pixel `crop` (copy, clamp, disjoint, empty, whole-image tight shortcut and padded whole-image repack), `downscale` (box averages, half-up rounding, no-op and degenerate/padded no-op coverage, aspect ratio, non-integer ratios), `fit_dimensions`, PNG round-trip through the `image` decoder (tight and padded strides, byte-identical for both), truncated-buffer encode errors, `image_from_readback` (tight fast path vs padded path byte equality, flipped, trailing bytes, malformed).
- `./tests/damage.rs` (16 tests): overlap/adjacency merging, disjoint preservation, clipping, `min_area` filtering keeping survivors sorted, sorted output regardless of input order, equivalence with `Region::simplified` for unsorted input, accumulator record/take/peek/clear/set_bounds/record_rect, fully-clipped and empty-bounds commits, counter behavior.
- `./tests/renderer_bounds.rs` (2 tests): compile-time assertions that `GlesRenderer`/`PixmanRenderer` satisfy `Renderer + Bind<T> + ExportMem + Offscreen<T> + ImportAll` for their real target types and that `GlesError`/`PixmanError` are `Send + Sync + 'static`.
- `./tests/render_backend.rs` (14 tests): pixman-backed integration tests using a local recording `Element`/`RenderElement` double — clear color, canonical exact pixels, z-order, `-source.loc` translation, element-local full-rect damage despite partial `SceneNode::damage`, scene-coordinate crop translation, crop-before-downscale order, `RenderedFrame::damage`/`commit_seq` passthrough, off-source nodes skipped, target size/format, 0x0 `create_target` error, invalid config, and the pooled-target stale-pixel guard (`reused_target_does_not_leak_pixels_into_a_node_free_frame`: a same-size target returned to a `TargetPool` and reused for a clear-only pass must come back entirely clear). Two GL tests run only with `ADESK_TEST_GL=1` (surfaceless EGL + `GlesRenderer`, skips cleanly when EGL is unavailable): `gl_renders_canonical_scene` asserts **exact image equality** with the pixman reference — the orientation regression guard (verified under Mesa llvmpipe) — and `gl_reused_target_does_not_leak_pixels_into_a_node_free_frame` is its reuse/clear-frame twin.
- `./tests/readback_crop.rs` (3 tests): the sub-rectangle-readback equality proof. A local recording `Element` double and a 5-node scene (full coverage, layering with element-local opaque regions, nodes hanging off both corners); for five crops (non-zero offset, small crop, two crops slicing partially-visible nodes, `crop == source`) it asserts `RenderConfig::new(SOURCE).with_crop(c)` is byte-for-byte equal to `RenderConfig::new(c)` (the unchanged whole-frame readback) and keeps `commit_seq`/scene-relative damage, plus a `crop == source` boundary test against a plain render and a `ADESK_TEST_GL=1` variant asserting GL equality with the pixman reference.
- `src/config.rs` `#[cfg(test)]` (13 tests): `validate`, `target_size`, `output_size` (crop, aspect-preserving `max_dimension`, no upscale, `0` disabled), defaults/formats, builders.
- `src/pool.rs` `#[cfg(test)]` (5 tests): `TargetPool` reuse proof through a counting `CountingRenderer` that wraps a real `PixmanRenderer` and counts `create_buffer` calls — a same-size `acquire` after `release` allocates nothing, a different size does not reuse, `release`-then-`acquire` serves the same size while other sizes reallocate, at most one target per size is retained, and the pool is bounded across more distinct sizes than `MAX_POOLED_TARGETS`.
- No test needs a display, GPU, network or installed application. Run: `bash scripts/dev.sh cargo test -p adesk-render` (the wrapper is a bash script; bare `cargo` cannot link outside the dev shell). GL: `ADESK_TEST_GL=1 ./scripts/dev.sh cargo test -p adesk-render`.
- The GL gate is skip-by-early-return, not `#[ignore]`: `gl_renderer()` returns `None` while the test still reports `ok` when `ADESK_TEST_GL != 1` or when EGL/GL/`GlesRenderbuffer` setup fails, and the skip reason only goes to `eprintln!` (captured by libtest, shown only with `--nocapture`). A GPU-less run with the gate on therefore shows the test count as passed while exercising no GL, and adesk-render has zero `#[ignore]`d tests.
- No test has a wall-clock sleep, timeout or `Instant`/`Duration`/`thread` use; all five test targets finish in well under a second.
- Integration-test crates do NOT inherit `#![forbid(unsafe_code)]` from `lib.rs`, so the GL tests may use `unsafe {}` for surfaceless EGL setup; the library itself stays unsafe-free.
- End-to-end coverage stays in `adesk-compositor/tests` and `adesk-server/tests` via `adesk-testkit`.

## Status

- Implemented; zero `todo!()` in the crate. `./scripts/dev.sh cargo test -p adesk-render` passes **74 tests** (18 unit — 13 config + 5 pool — 16 damage + 21 image_ops + 3 readback_crop + 14 render_backend + 2 renderer_bounds), 0 failures, 0 ignored; `cargo fmt -p adesk-render --check`, `cargo clippy -p adesk-render --all-targets --no-deps -- -D warnings`, `cargo doc -p adesk-render --no-deps --document-private-items` (zero warnings) and `cargo check -p adesk-render --no-default-features` are clean, and `cargo check --workspace --all-targets` is green after the API cleanup.
- The GL path is verified under `ADESK_TEST_GL=1` (Mesa llvmpipe) with exact image equality against the pixman reference, including the sub-rect readback.
- Known gaps: `create_target` error boxing is text-only (signature gap above); `DamageAccumulator`/`coalesce_damage` are unwired (see Known Issues); `RenderError::{UnsupportedBuffer, UnsupportedFormat}` now have no in-crate constructor.

## Known Issues

- `DamageAccumulator` and `coalesce_damage` (`src/damage.rs`) are referenced only by `tests/damage.rs`. They are not obsolete design: `docs/architecture.md` §5 ("the compositor accumulates per-window damage from `SurfaceCommit` events … coalesced and simplified; it is independent of the renderer") and `docs/protocol.md` §4 specify exactly this capability, and `adesk-observer`'s `waiter.rs::Accumulator::{absorb, resolve}` hand-rolls the same `damage.clip(&geometry)` + `Region::simplified()` — including the per-commit throwaway `Region` this module no longer allocates. Wiring the observer's damage half to this module is a cross-crate change that has not been made, so the API and its tests are kept.
- `RenderError::UnsupportedBuffer` and `RenderError::UnsupportedFormat` are no longer constructed anywhere in the crate (they remain in `code()`'s mapping); `RenderError::ImportFailed` **is** constructed cross-crate (`crates/adesk-server/src/error.rs`), so the variant family cannot be pruned from within this crate. Removing the two unused variants is a cross-crate API decision.
- `encode_png` is used by this crate's tests only, even though it is the intended canonical home for "stride-repack then PNG-encode an `ImageBuffer`". That capability is independently re-implemented in `adesk-server/src/images.rs` (`encode_png` + `tightly_packed`), `adesk-viewer/src/capture.rs` (`tight_rgba8` + `save_buffer_with_format`) and (test-only) `adesk-testkit`'s `ImageAssert::save_png`; `adesk-server` already depends on `adesk-render`, so it could delegate instead of duplicating. Migrating those three call sites is a sibling-crate change.
- adesk-render has no image *comparison*/diff/mean helper: pattern/solid checks, tolerance-aware diffs and region means live only in `adesk-testkit`'s `ImageAssert`; `tests/render_backend.rs`'s local `pixel()` is the only bounds-checked accessor here and is test-local.

## Notes for Agents

- `render_scene` requires `target.core_size() == config.target_size()`; callers should create the target from `config.target_size()`.
- Do not re-introduce a post-readback `crop` in `render_scene`: the crop is applied by reading back the translated sub-rectangle, and `tests/readback_crop.rs` is the equality guard. `damage` must stay in scene coordinates regardless of crop/downscale.
- `render_scene` has no "nothing to render" path: an empty `Scene` (zero nodes) or nodes with empty damage still renders `Ok` as a full-size frame of the clear color (test `pixman_empty_scene_renders_clear_color`).
  Only genuine backend/import/readback failures are `Err`; callers that want "no content" to be an error must check `Scene::is_empty()` themselves.
- A validated `RenderConfig` guarantees a non-empty frame: `render_scene` never returns a `0x0` image (`validate` rejects empty `source`/`crop` and crop non-containment); the standalone pure `crop` helper's `0x0` output (disjoint/empty rect) is rejected by `encode_png` as `InvalidImage`.
- `render_scene` returns `RenderedFrame::damage` as raw clipped rects (no coalescing); `adesk-compositor` applies `Region::simplified()` before replying.
  The rects stay in scene/window coordinates even when `crop`/`max_dimension` shrink the image, so a cropped capture's `changed_regions` can reference coordinates outside the returned image.
- GL readback rows are already top-down in scene space; do NOT feed `TextureMapping::flipped()` into `image_from_readback` in this pipeline — it would mirror every GL capture (see Design Decisions).
- `copy_framebuffer`'s `Fourcc` argument and its sub-region parameter are 0.7-era; `RenderElement::draw` lost its `cache` argument in 0.7. Do not copy 0.6-era examples.
- `GlesRenderer` is not `Send` and every renderer call is compositor-thread-only (`docs/architecture.md` §1); this crate holds no global state and never spawns threads.
- `smithay::utils::Size<i32, Buffer>` (buffer coords) and `Size<i32, Physical>` are distinct types; at scale 1.0 with `Transform::Normal` their numeric values are equal, but conversions must be explicit.
- The `image` crate is used with `default-features = false, features = ["png"]`; only PNG encode/decode (and the test-only decoder) are available.
- `RenderConfig::clear_color` is a public field, not a builder — set it directly (struct-update syntax) in tests and callers.

## Performance Notes

- Rendering is on demand, so per-call cost times request frequency is the metric: one full pass per `capture_window`/`capture_region`/`observe(include_image)`/`inspect_capture` and per VAP frame.
- A crop no longer costs a full-frame readback plus a later image crop: `render_scene` reads back only the crop sub-rectangle and never runs the pure `crop` helper, and `downscale` is only called when `max_dimension` is set. `crop` and `downscale` also short-circuit their own no-op cases.
- `encode_png` borrows tight pixel data (`Cow`) and `image_from_readback` never builds a zero-filled intermediate; `DamageAccumulator::record_commit`/`coalesce_damage` no longer allocate a throwaway clipped `Region` (two rect buffers instead of three, and only when `min_area` drops something). `render_scene` reuses its per-node damage/opaque scratch buffers.
- Still open (deliberately not done in the perf pass): drawing is not damage-scissored (each intersecting node is drawn over its full visible rect); `encode_png` uses the `image` crate's default compression level; `Region::coalesce` is an O(n²) pairwise merge with `Vec::remove` in `adesk-core/src/geometry.rs`, run per observation and again as `frame.damage.simplified()` in the compositor.
- Offscreen targets are **reused, not reallocated per capture**: `TargetPool` (held per backend by `adesk-compositor`'s `HeadlessRenderer`) caches one target per exact source size, so repeated captures of the same window/output skip `create_target`'s ~4 MiB allocation; only the first capture of a size (or one whose pool entry was evicted) allocates. The pool is bounded at `MAX_POOLED_TARGETS` (4) sizes, so its footprint does not grow with the number of distinct sizes seen.
  Per-capture cost is therefore clear + draw every intersecting node + crop-sized readback + optional downscale; the target allocation is amortized away on repeats.
- `copy_framebuffer`'s region is now crop-sized, but the readback slice is still copied once into the `ImageBuffer`; there is no zero-copy path into `adesk-proto`'s base64 encoding.

## Dependencies

- `adesk-core` — `ImageBuffer`, `Rect`, `Region`, `Size`, `ErrorCode`, `Error` (never fork these types).
- `smithay` 0.7 with `wayland_frontend`, `desktop`, `renderer_pixman`, `renderer_glow` (pulls `wayland-server`, `wayland-protocols`, `pixman`, `glow`, `gl_generator`, `drm-fourcc`).
- `image` 0.25 (png only), `thiserror` 2.
- System libraries (pixman, EGL/GLES, libwayland) come from the Nix dev shell; build through `./scripts/dev.sh`.
