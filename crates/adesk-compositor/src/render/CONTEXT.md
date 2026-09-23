# adesk-compositor/src/render — headless renderer + element collection

## Intent

Own the compositor's only contact surface with Smithay's renderer abstraction: construct the
backend renderer (`HeadlessRenderer`), collect a window's surface tree and popups into
`adesk_render::Scene` nodes, and hand the scene to `adesk-render` for (pooled) target
acquisition, readback, crop/downscale and encoding.
Pixel production, damage tracking and image encoding are deliberately **not** here.

## API Surface

- `HeadlessRenderer` (`headless.rs`, `pub(crate)`) — enum over `Gl { renderer: Box<GlesRenderer>, pool: TargetPool<GlTarget> }` / `Pixman { renderer: PixmanRenderer, pool: TargetPool<PixmanTarget> }`:
  `create(RendererKind)`, `name()`, `dmabuf_formats()`, `render_window(surface, …)`,
  `render_output(output_size, windows, region, max_dimension)`.
  Each variant owns its backend's `adesk_render::TargetPool`, so repeated captures of the
  same size reuse the offscreen target instead of reallocating it.
- `OutputWindow` (`mod.rs`, `pub(crate)`) — one window as the output-composition path sees it:
  `geometry: Rect` (output coords), `surface: WlSurface` (root), `active: bool` (the visibility selector).
- `elements.rs` (`pub(crate)`, module-private except `window_elements`/`window_scene`/`output_scene`/
  `popup_surfaces`/`SCENE_SCALE`): the element/scene builder.
- `OutputRenderElements<R>` — `render_elements!` enum wrapping a window `Surface` element.

## Constraints

- Scene coordinates are physical pixels at `SCENE_SCALE == 1.0` (v1 has no fractional scale).
- Renderer selection is runtime, not a Cargo feature; both backends are always compiled in.
- `GlesRenderer` is `!Send`; the renderer never leaves the compositor thread.
- Public API is exactly what `crates/adesk-compositor/CONTEXT.md` documents; everything here is `pub(crate)`.

## Notes for Agents

- `render_elements_from_surface_tree` walks the whole tree (subsurfaces yes, **popups no**); popups
  come from the static `PopupManager::popups_for_surface` (front-to-back, reversed here).
- GL readback is y-flipped and GL rows arrive top-down; do not double-flip.
- `XKB_CONFIG_ROOT` and EGL exist only in the dev shell; build via `./scripts/dev.sh`.
- A buffer the backend cannot import (a DMA-BUF pixman cannot `mmap`, a bad SHM pool, ...) is
  **dropped, never rendered**: at protocol time the `dmabuf` handler logs `dmabuf import failed`
  and calls `notifier.failed()`; at render time Smithay's element walk logs `Failed to import
  surface` and omits the element (`WaylandSurfaceRenderElement::from_surface` returns `Err`
  before a texture is cached). `window_elements`/`output_scene` therefore degrade to a clear or
  partial frame and **no dangling texture reaches `adesk_render::render_scene`** — a failed import
  is a clean, non-fatal outcome of this module.
- This module's own walks are bounded (`wm::MAX_SURFACE_TREE_DEPTH` = 32 in `state.rs`/`wm.rs`).
  Smithay's walk underneath it is not: `render_elements_from_surface_tree` → `with_surface_tree_downward`
  → `PrivateSurfaceData::map` recurses once per subsurface level, and popup collection
  (`PopupNode::iter_popups_relative_to`) recurses once per nested popup — only a deliberately deep
  client tree risks an 8 MiB stack overflow; an ordinary client cannot reach it.
- The workspace's only production `unsafe` is the surfaceless-EGL bootstrap in `headless.rs`
  (`create_gl`: `EGLDisplay::new`, `EGLContext::make_current`, `GlesRenderer::new`); the pixman path
  contains no `unsafe`. A SIGSEGV observed while a client streams DMA-BUFs is **not** producible by
  this module's own code: every failure path is a `Result`, element collection has no unchecked
  indexing, and the walks are bounded. The fault sits in the dependency/backend layer this module
  calls — Smithay's `PixmanRenderer`/`Dmabuf` (raw `mmap` pointer + client stride handed to
  libpixman `composite32`) or, on the GL path, Mesa's EGL import. See the next section and
  `crates/adesk-compositor/CONTEXT.md` "DMA-BUF crash triage".

## DMA-BUF format advertising & crash triage (verified against Smithay 0.7 source)

- **The advertised set is already exactly the active renderer's own set**, so "restrict advertising
  to what the renderer supports" is already satisfied and there is no restriction left to add.
  `HeadlessRenderer::dmabuf_formats` matches on the variant `create` actually built, and
  `State::new` builds the renderer *before* the global (`../state.rs`, `../protocols/dmabuf.rs`).
- GL: `GlesRenderer::dmabuf_formats` is `egl.dmabuf_texture_formats()`, *queried from the EGL display
  itself* (`eglQueryDmaBufFormatsEXT` + `QueryDmaBufModifiersEXT`), so it cannot name a
  fourcc/modifier the display does not claim. It also carries the `Modifier::Invalid` (implicit)
  entry per fourcc and the `external_only` modifiers — correct here, because the compositor only
  *samples* client buffers and never renders into them. `GlesRenderer::has_dmabuf_format` is defined
  as literally `dmabuf_texture_formats().contains(&format)`, so intersecting the advertised set with
  it is a tautology; nothing in the workspace calls `has_dmabuf_format` anyway.
- pixman advertises a static list of **single-plane packed** fourccs with `Modifier::Linear`, and its
  importer requires exactly `num_planes() == 1` + `modifier == Linear` — consistent.
- **The GL import path never mmaps.** `GlesRenderer::import_dmabuf` → `EGLDisplay::create_image_from_dmabuf`
  only builds `EGL_LINUX_DMA_BUF_EXT` attributes and calls `eglCreateImageKHR` into Mesa.
  `Dmabuf::map_plane` has exactly one caller in Smithay: `PixmanRenderer::import_dmabuf`. An
  mmap/`EPERM` failure under the GL renderer therefore originates inside Mesa (llvmpipe must CPU-map
  the buffer; a hardware importer would not), never in this crate or Smithay's GL code — and the
  string is not produced by any ADesk or Smithay code. `--renderer pixman` instead fails *cleanly and
  before* the raw pointer reaches libpixman (it validates plane count/modifier/format and
  `mapping.length() >= stride * height` first), so the EPERM trigger becomes a logged
  `notifier.failed()`.
- The `names[i]` lookup in `create_image_from_dmabuf` cannot go out of bounds: Smithay's
  `MAX_PLANES` is 4, `DmabufBuilder::add_plane` refuses a 5th plane, and the wayland layer rejects
  `plane_idx >= MAX_PLANES` and duplicate indices, so `i <= 3` and `names` has exactly 4 rows.
- The protocol layer validates an incoming buffer's **fourcc** against the advertised set (a
  non-advertised fourcc is a client error) but never the modifier pair, so a modifier restriction
  could not influence a non-conforming client anyway.

## Routing Table

| Area | Owner |
|---|---|
| `HeadlessRenderer` enum, EGL/pixman construction, render entry points | `headless.rs` |
| `OutputWindow`, module doc, re-exports | `mod.rs` |
| Surface/popup element walk, scene construction, output composition | `elements.rs` |
